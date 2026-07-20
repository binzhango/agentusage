use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDate, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
};

use crate::storage::{IngestRecord, RawEvent, UsageEvent, UsageMetric, UsageStore};

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct TokenBreakdown {
    pub requests: i64,
    pub input: i64,
    pub output: i64,
    pub reasoning: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub total: i64,
    pub cost_usd: f64,
    pub ai_units_nano: i64,
    pub request_multiplier: f64,
    pub ai_credits: f64,
}

#[derive(Debug, Default, PartialEq)]
pub struct DailyUsage {
    pub provider: String,
    pub date: NaiveDate,
    pub end_date: NaiveDate,
    pub sessions: usize,
    pub requests: usize,
    pub prompts: usize,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: f64,
    pub ai_units_nano: i64,
    pub ai_credits: f64,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub models: BTreeMap<String, TokenBreakdown>,
    pub clients: BTreeMap<String, TokenBreakdown>,
    pub projects: BTreeMap<String, TokenBreakdown>,
    pub tools: BTreeMap<String, usize>,
    pub languages: BTreeMap<String, usize>,
    pub files_scanned: usize,
    pub files_with_usage: usize,
    pub token_records: usize,
    pub malformed_lines: usize,
    pub desktop_usage: Option<DesktopUsage>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DesktopUsage {
    pub samples: usize,
    pub latest_timestamp_ms: i64,
    pub five_hour_signal: i64,
    pub seven_day_signal: i64,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct IngestStats {
    pub files_scanned: usize,
    pub files_with_usage: usize,
    pub token_records: usize,
    pub malformed_lines: usize,
    pub events_inserted: usize,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Counters {
    input: i64,
    output: i64,
    reasoning: i64,
    cached: i64,
    cache_write: i64,
    total: i64,
}

pub fn today_usage(date: Option<&str>, sessions_dir: Option<&str>) -> Result<DailyUsage> {
    let target = match date {
        Some(value) => NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .with_context(|| format!("invalid date {value:?}; expected YYYY-MM-DD"))?,
        None => Local::now().date_naive(),
    };
    usage_between(target, target, sessions_dir)
}

pub fn usage_between(
    start: NaiveDate,
    end: NaiveDate,
    sessions_dir: Option<&str>,
) -> Result<DailyUsage> {
    if end < start {
        anyhow::bail!("end date must be on or after start date");
    }
    let root = sessions_dir
        .map(PathBuf::from)
        .unwrap_or_else(default_sessions_dir);
    let mut report = DailyUsage {
        date: start,
        end_date: end,
        provider: "codex".into(),
        ..Default::default()
    };
    for path in jsonl_files(&root)? {
        report.files_scanned += 1;
        let requests_before = report.requests;
        if parse_session(&path, start, end, &mut report)? {
            report.sessions += 1;
        }
        if report.requests > requests_before {
            report.files_with_usage += 1;
        }
    }
    Ok(report)
}

pub fn ingest_into_store<S: UsageStore>(
    sessions_dir: Option<&str>,
    store: &mut S,
) -> Result<IngestStats> {
    let root = sessions_dir
        .map(PathBuf::from)
        .unwrap_or_else(default_sessions_dir);
    let mut stats = IngestStats::default();
    for path in jsonl_files(&root)? {
        stats.files_scanned += 1;
        let file_size = fs::metadata(&path)?.len() as i64;
        if let Some(cursor) = store.cursor(&path.to_string_lossy())?
            && cursor.file_size == file_size
            && cursor.last_event_hash.as_deref() == Some("project-v5")
        {
            continue;
        }
        let token_records_before = stats.token_records;
        let before = stats.events_inserted;
        ingest_session(&path, store, &mut stats)?;
        if stats.token_records > token_records_before || stats.events_inserted > before {
            stats.files_with_usage += 1;
        }
        store.save_cursor(&crate::storage::FileCursor {
            path: path.to_string_lossy().into_owned(),
            byte_offset: file_size,
            file_size,
            last_event_hash: Some("project-v5".into()),
            updated_at: Utc::now(),
        })?;
    }
    Ok(stats)
}

fn ingest_session<S: UsageStore>(
    path: &Path,
    store: &mut S,
    stats: &mut IngestStats,
) -> Result<()> {
    let content = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let session_id = stable_id(&format!("session:{}", path.display()));
    let mut previous = Counters::default();
    let mut model = String::from("unknown");
    let mut client = String::from("Other");
    let mut project = String::new();
    for (line_number, line) in content.lines().enumerate() {
        let Ok(root) = serde_json::from_str::<Value>(line) else {
            stats.malformed_lines += 1;
            continue;
        };
        let kind = root.get("type").and_then(Value::as_str).unwrap_or_default();
        let payload = root.get("payload").unwrap_or(&Value::Null);
        let occurred_at = root
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc));
        let line_id = format!("{}:{line_number}", path.display());
        if kind == "session_meta" {
            model = first_string(payload, &["model", "model_id"]).unwrap_or(model);
            client = classify_client(
                first_string(payload, &["source"]).as_deref(),
                first_string(payload, &["originator"]).as_deref(),
            );
            project = project_from_value(payload).unwrap_or(project);
        }
        if kind == "turn_context" {
            model = first_string(payload, &["model", "model_id"]).unwrap_or(model);
            project = project_from_value(payload).unwrap_or(project);
        }
        store.append_record(&IngestRecord {
            record_id: stable_id(&format!("record:{line_id}")),
            source_path: path.to_string_lossy().into_owned(),
            line_number: line_number as i64,
            occurred_at,
            provider_id: "codex".into(),
            agent_name: "codex".into(),
            session_id: Some(session_id.clone()),
            event_type: kind.into(),
            payload_type: payload
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_owned),
            model: Some(model.clone()),
            client: Some(client.clone()),
            project: (!project.is_empty()).then(|| project.clone()),
            tool_name: payload
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned),
            payload: root.clone(),
            dedup_key: format!("record:{line_id}"),
        })?;
        if kind == "session_meta" || kind == "turn_context" {
            continue;
        }
        let Some(occurred_at) = occurred_at else {
            continue;
        };
        if kind == "response_item"
            && matches!(
                payload.get("type").and_then(Value::as_str),
                Some("custom_tool_call") | Some("function_call")
            )
        {
            if let Some(name) = payload.get("name").and_then(Value::as_str) {
                append_metric(
                    store,
                    &session_id,
                    occurred_at,
                    "tool",
                    name,
                    &format!("{line_id}:tool:{name}"),
                )?;
            }
            if let Some(input) = payload
                .get("input")
                .or_else(|| payload.get("arguments"))
                .and_then(Value::as_str)
            {
                let mut languages = BTreeMap::new();
                if payload.get("name").and_then(Value::as_str) == Some("apply_patch") {
                    count_patch_languages(input, &mut languages);
                } else {
                    count_tool_languages(input, &mut languages);
                }
                for language in languages.keys() {
                    append_metric(
                        store,
                        &session_id,
                        occurred_at,
                        "language_v2",
                        language,
                        &format!("{line_id}:language-v2:{language}"),
                    )?;
                }
            }
            continue;
        }
        if kind == "event_msg"
            && payload.get("type").and_then(Value::as_str) == Some("user_message")
        {
            let usage = UsageEvent {
                event_id: stable_id(&format!("prompt:{line_id}")),
                occurred_at,
                provider_id: "codex".into(),
                agent_name: "codex".into(),
                session_id: Some(session_id.clone()),
                model: Some(model.clone()),
                client: Some(client.clone()),
                project: (!project.is_empty()).then(|| project.clone()),
                prompts: 1,
                dedup_key: stable_id(&format!("prompt:{line_id}")),
                raw_event_id: stable_id(&format!("raw:prompt:{line_id}")),
                ..Default::default()
            };
            if append_event(store, &root, &usage)? {
                stats.events_inserted += 1;
            }
            continue;
        }
        if kind == "event_msg" && payload.get("type").and_then(Value::as_str) == Some("token_count")
        {
            let current = counters(payload.get("info"));
            stats.token_records += 1;
            if current == Counters::default() {
                continue;
            }
            let delta = current.saturating_sub(previous);
            previous = current;
            if delta.total <= 0 {
                continue;
            }
            let event_model =
                first_string(payload, &["model", "model_id"]).unwrap_or_else(|| model.clone());
            let event_id = stable_id(&format!("token:{line_id}"));
            let usage = UsageEvent {
                event_id: event_id.clone(),
                occurred_at,
                provider_id: "codex".into(),
                agent_name: "codex".into(),
                session_id: Some(session_id.clone()),
                model: Some(event_model.clone()),
                client: Some(client.clone()),
                project: (!project.is_empty()).then(|| project.clone()),
                input_tokens: delta.input,
                output_tokens: delta.output,
                reasoning_tokens: delta.reasoning,
                cache_read_tokens: delta.cached,
                cache_write_tokens: delta.cache_write,
                total_tokens: delta.total,
                cost_usd: estimate_cost(&event_model, delta),
                requests: 1,
                dedup_key: event_id,
                raw_event_id: stable_id(&format!("raw:token:{line_id}")),
                ..Default::default()
            };
            if append_event(store, &root, &usage)? {
                stats.events_inserted += 1;
            }
            continue;
        }
        if kind == "response_item"
            && payload.get("type").and_then(Value::as_str) == Some("custom_tool_call")
            && payload.get("name").and_then(Value::as_str) == Some("apply_patch")
            && let Some(input) = payload.get("input").and_then(Value::as_str)
        {
            let (added, removed) = patch_counts(input);
            if added == 0 && removed == 0 {
                continue;
            }
            let event_id = stable_id(&format!("patch:{line_id}"));
            let usage = UsageEvent {
                event_id: event_id.clone(),
                occurred_at,
                provider_id: "codex".into(),
                agent_name: "codex".into(),
                session_id: Some(session_id.clone()),
                model: Some(model.clone()),
                client: Some(client.clone()),
                project: (!project.is_empty()).then(|| project.clone()),
                lines_added: added,
                lines_removed: removed,
                dedup_key: event_id,
                raw_event_id: stable_id(&format!("raw:patch:{line_id}")),
                ..Default::default()
            };
            if append_event(store, &root, &usage)? {
                stats.events_inserted += 1;
            }
        }
    }
    Ok(())
}

fn append_event<S: UsageStore>(store: &mut S, payload: &Value, event: &UsageEvent) -> Result<bool> {
    let raw = RawEvent {
        event_id: event.raw_event_id.clone(),
        source_system: "codex".into(),
        source_channel: "jsonl".into(),
        occurred_at: event.occurred_at,
        payload: payload.clone(),
        payload_hash: stable_id(&serde_json::to_string(payload)?),
    };
    store.append_raw_event(&raw)?;
    store.append_usage_event(event)
}

fn append_metric<S: UsageStore>(
    store: &mut S,
    session_id: &str,
    occurred_at: DateTime<Utc>,
    dimension: &str,
    name: &str,
    dedup_key: &str,
) -> Result<()> {
    store.append_metric(&UsageMetric {
        metric_id: stable_id(&format!("metric:{dedup_key}")),
        occurred_at,
        provider_id: "codex".into(),
        agent_name: "codex".into(),
        session_id: Some(session_id.to_owned()),
        dimension: dimension.into(),
        name: name.into(),
        dedup_key: dedup_key.into(),
    })?;
    Ok(())
}

fn stable_id(value: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(value.as_bytes());
    hex::encode(hash.finalize())
}

fn default_sessions_dir() -> PathBuf {
    let home = env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .unwrap_or_default();
    PathBuf::from(home).join(".codex").join("sessions")
}

fn project_from_value(value: &Value) -> Option<String> {
    let raw = ["cwd", "project", "workspace", "workspace_id", "workdir"]
        .iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))?
        .trim();
    if raw.is_empty() {
        return None;
    }
    let path = Path::new(raw);
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .or_else(|| Some(raw.to_owned()))
}

fn jsonl_files(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(jsonl_files(&path)?);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
    Ok(files)
}

fn parse_session(
    path: &Path,
    start: NaiveDate,
    end: NaiveDate,
    report: &mut DailyUsage,
) -> Result<bool> {
    let content = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut previous = Counters::default();
    let mut model = String::from("unknown");
    let mut client = String::from("Other");
    let mut project = String::new();
    let mut counted = false;
    for line in content.lines() {
        let Ok(root) = serde_json::from_str::<Value>(line) else {
            report.malformed_lines += 1;
            continue;
        };
        let kind = root.get("type").and_then(Value::as_str).unwrap_or_default();
        let payload = root.get("payload").unwrap_or(&Value::Null);
        if kind == "session_meta" {
            model = first_string(payload, &["model", "model_id"]).unwrap_or(model);
            client = classify_client(
                first_string(payload, &["source"]).as_deref(),
                first_string(payload, &["originator"]).as_deref(),
            );
            project = project_from_value(payload).unwrap_or(project);
            continue;
        }
        if kind == "turn_context" {
            model = first_string(payload, &["model", "model_id"]).unwrap_or(model);
            project = project_from_value(payload).unwrap_or(project);
            continue;
        }
        let timestamp = root
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok());
        let on_day = timestamp
            .map(|value| {
                let date = value.with_timezone(&Local).date_naive();
                date >= start && date <= end
            })
            .unwrap_or(false);
        if kind == "event_msg"
            && payload.get("type").and_then(Value::as_str) == Some("user_message")
        {
            if on_day {
                report.prompts += 1;
            }
            continue;
        }
        if kind == "event_msg" && payload.get("type").and_then(Value::as_str) == Some("token_count")
        {
            let current = counters(payload.get("info"));
            report.token_records += 1;
            if current == Counters::default() {
                continue;
            }
            let delta = current.saturating_sub(previous);
            previous = current;
            if !on_day || delta.total <= 0 {
                continue;
            }
            let event_model =
                first_string(payload, &["model", "model_id"]).unwrap_or_else(|| model.clone());
            let breakdown = TokenBreakdown {
                input: delta.input,
                output: delta.output,
                reasoning: delta.reasoning,
                cache_read: delta.cached,
                cache_write: delta.cache_write,
                total: delta.total,
                cost_usd: estimate_cost(&event_model, delta),
                ..Default::default()
            };
            report.requests += 1;
            report.input_tokens += delta.input;
            report.output_tokens += delta.output;
            report.reasoning_tokens += delta.reasoning;
            report.cached_input_tokens += delta.cached;
            report.cache_write_tokens += delta.cache_write;
            report.total_tokens += delta.total;
            report.cost_usd += breakdown.cost_usd;
            add_breakdown(&mut report.models, event_model, breakdown);
            add_breakdown(&mut report.clients, client.clone(), breakdown);
            if !project.is_empty() {
                add_breakdown(&mut report.projects, project.clone(), breakdown);
            }
            counted = true;
            continue;
        }
        if kind == "response_item"
            && on_day
            && payload.get("type").and_then(Value::as_str) == Some("custom_tool_call")
        {
            if let Some(name) = payload.get("name").and_then(Value::as_str) {
                *report.tools.entry(name.to_owned()).or_default() += 1;
            }
            if let Some(input) = payload.get("input").and_then(Value::as_str) {
                if payload.get("name").and_then(Value::as_str) == Some("apply_patch") {
                    count_patch(input, report);
                    count_patch_languages(input, &mut report.languages);
                } else {
                    count_tool_languages(input, &mut report.languages);
                }
            }
        }
    }
    Ok(counted)
}

fn counters(info: Option<&Value>) -> Counters {
    let Some(usage) = info.and_then(|value| value.get("total_token_usage")) else {
        return Counters::default();
    };
    Counters {
        input: number(usage, "input_tokens"),
        output: number(usage, "output_tokens"),
        reasoning: number(usage, "reasoning_output_tokens"),
        cached: number(usage, "cached_input_tokens"),
        cache_write: number(usage, "cache_write_tokens"),
        total: number(usage, "total_tokens"),
    }
}

fn number(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or_default()
}

fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    })
}

fn classify_client(source: Option<&str>, originator: Option<&str>) -> String {
    let value = format!(
        "{} {}",
        source.unwrap_or_default(),
        originator.unwrap_or_default()
    )
    .to_ascii_lowercase();
    if value.contains("desktop") || value.contains("app") {
        "Desktop".into()
    } else if value.contains("vscode") || value.contains("ide") {
        "IDE".into()
    } else if value.contains("cli") {
        "CLI".into()
    } else {
        "Other".into()
    }
}

fn add_breakdown(map: &mut BTreeMap<String, TokenBreakdown>, name: String, value: TokenBreakdown) {
    let entry = map.entry(name).or_default();
    entry.input += value.input;
    entry.output += value.output;
    entry.reasoning += value.reasoning;
    entry.cache_read += value.cache_read;
    entry.cache_write += value.cache_write;
    entry.total += value.total;
    entry.cost_usd += value.cost_usd;
}

fn estimate_cost(model: &str, usage: Counters) -> f64 {
    // Codex subscription usage is not an invoice. This is an API-equivalent
    // estimate using the current GPT-5 family rate assumption.
    if !model.to_ascii_lowercase().contains("gpt-5")
        && !model.to_ascii_lowercase().contains("codex")
    {
        return 0.0;
    }
    (usage.input as f64 * 1.25 + usage.cached as f64 * 0.125 + usage.output as f64 * 10.0)
        / 1_000_000.0
}

fn count_patch(input: &str, report: &mut DailyUsage) {
    let (added, removed) = patch_counts(input);
    report.lines_added += added;
    report.lines_removed += removed;
}

fn count_patch_languages(input: &str, languages: &mut BTreeMap<String, usize>) {
    let mut seen = BTreeSet::new();
    for line in input.lines() {
        let path = line
            .strip_prefix("+++ b/")
            .or_else(|| line.strip_prefix("*** Update File: "))
            .or_else(|| line.strip_prefix("*** Add File: "))
            .or_else(|| line.strip_prefix("*** Delete File: "));
        let Some(path) = path else { continue };
        add_language(path, &mut seen, languages);
    }
}

fn count_tool_languages(input: &str, languages: &mut BTreeMap<String, usize>) {
    let mut seen = BTreeSet::new();
    for token in input.split_whitespace() {
        let token = token.trim_matches(|character: char| {
            matches!(character, '"' | '\'' | '`' | ',' | ';' | ')' | ']' | '}')
        });
        if token.contains('/') || token.starts_with('.') {
            add_language(token, &mut seen, languages);
        }
    }
}

fn add_language(path: &str, seen: &mut BTreeSet<String>, languages: &mut BTreeMap<String, usize>) {
    let path = path
        .trim()
        .split("\\n")
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .unwrap_or_default();
    let Some(extension) = Path::new(path).extension().and_then(|value| value.to_str()) else {
        return;
    };
    let extension = extension.to_ascii_lowercase();
    let language = match extension.as_str() {
        "rs" => "rust",
        "py" => "python",
        "js" | "jsx" => "javascript",
        "ts" | "tsx" => "typescript",
        "md" | "mdx" => "markdown",
        "json" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "sh" | "bash" => "shell",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cpp" | "cc" | "hpp" => "cpp",
        _ => return,
    };
    if seen.insert(language.to_owned()) {
        *languages.entry(language.to_owned()).or_default() += 1;
    }
}

fn patch_counts(input: &str) -> (i64, i64) {
    let mut added = 0;
    let mut removed = 0;
    for line in input.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            added += 1;
        }
        if line.starts_with('-') && !line.starts_with("---") {
            removed += 1;
        }
    }
    (added, removed)
}

impl Counters {
    fn saturating_sub(self, previous: Self) -> Self {
        Self {
            input: self.input.saturating_sub(previous.input),
            output: self.output.saturating_sub(previous.output),
            reasoning: self.reasoning.saturating_sub(previous.reasoning),
            cached: self.cached.saturating_sub(previous.cached),
            cache_write: self.cache_write.saturating_sub(previous.cache_write),
            total: self.total.saturating_sub(previous.total),
        }
    }
}

pub fn cache_hit_rate(report: &DailyUsage) -> Option<f64> {
    let denominator = report.input_tokens + report.cached_input_tokens + report.cache_write_tokens;
    (denominator > 0 && report.cached_input_tokens > 0)
        .then(|| report.cached_input_tokens as f64 / denominator as f64 * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn aggregates_breakdowns_and_cache_rate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(file, r#"{{"timestamp":"2026-07-18T23:59:00Z","type":"session_meta","payload":{{"model":"gpt-5.5","source":"vscode","originator":"Codex Desktop"}}}}"#).unwrap();
        writeln!(file, r#"{{"timestamp":"2026-07-19T05:00:00Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":15,"cached_input_tokens":5,"output_tokens":4,"total_tokens":19}}}}}}}}"#).unwrap();
        let report = today_usage(Some("2026-07-19"), Some(dir.path().to_str().unwrap())).unwrap();
        assert_eq!(report.requests, 1);
        assert!(report.models.contains_key("gpt-5.5"));
        assert!(report.clients.contains_key("Desktop"));
        assert_eq!(cache_hit_rate(&report), Some(25.0));
    }
}
