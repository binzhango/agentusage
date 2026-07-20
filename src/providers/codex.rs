use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    collections::BTreeSet,
    env, fs,
    io::{BufRead, BufReader},
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

pub fn ingest_into_store<S: UsageStore>(
    sessions_dir: Option<&str>,
    store: &mut S,
) -> Result<IngestStats> {
    let root = sessions_dir
        .map(PathBuf::from)
        .unwrap_or_else(default_sessions_dir);
    let mut stats = IngestStats::default();
    store.begin_batch()?;
    for path in jsonl_files(&root)? {
        stats.files_scanned += 1;
        let file_size = fs::metadata(&path)?.len() as i64;
        let cursor = store.cursor(&path.to_string_lossy())?;
        if let Some(cursor) = cursor.as_ref()
            && cursor.file_size == file_size
            && cursor.byte_offset == file_size
            && cursor.last_event_hash.as_deref() == Some("project-v5")
        {
            continue;
        }
        let start_offset = cursor
            .filter(|value| value.byte_offset <= file_size)
            .map(|value| value.byte_offset.max(0) as u64)
            .unwrap_or(0);
        let token_records_before = stats.token_records;
        let before = stats.events_inserted;
        let processed_offset = ingest_session(&path, start_offset, store, &mut stats)?;
        if stats.token_records > token_records_before || stats.events_inserted > before {
            stats.files_with_usage += 1;
        }
        store.save_cursor(&crate::storage::FileCursor {
            path: path.to_string_lossy().into_owned(),
            byte_offset: processed_offset as i64,
            file_size,
            last_event_hash: Some("project-v5".into()),
            updated_at: Utc::now(),
        })?;
    }
    store.end_batch()?;
    Ok(stats)
}

fn ingest_session<S: UsageStore>(
    path: &Path,
    start_offset: u64,
    store: &mut S,
    stats: &mut IngestStats,
) -> Result<u64> {
    let file = fs::File::open(path).with_context(|| format!("read {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let session_id = stable_id(&format!("session:{}", path.display()));
    let mut previous = Counters::default();
    let mut model = String::from("unknown");
    let mut client = String::from("Other");
    let mut project = String::new();
    let mut line_number = 0usize;
    let mut byte_offset = 0u64;
    let mut last_complete_offset = 0u64;
    let mut buffer = String::new();
    loop {
        buffer.clear();
        let bytes_read = reader.read_line(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        byte_offset += bytes_read as u64;
        let complete_line = buffer.ends_with('\n');
        if complete_line {
            last_complete_offset = byte_offset;
        }
        let persist = complete_line && byte_offset > start_offset;
        let line = buffer.trim_end_matches(['\r', '\n']);
        let current_line_number = line_number;
        line_number += 1;
        let Ok(root) = serde_json::from_str::<Value>(line) else {
            if persist {
                stats.malformed_lines += 1;
            }
            continue;
        };
        let kind = event_kind(&root);
        let payload = event_payload(&root);
        let occurred_at = event_timestamp(&root);
        let line_id = format!("{}:{current_line_number}", path.display());
        if kind == "session_meta" {
            model = first_string(payload, &["model", "model_id", "modelId"]).unwrap_or(model);
            client = classify_client(
                first_string(payload, &["source"]).as_deref(),
                first_string(payload, &["originator"]).as_deref(),
            );
            project = project_from_value(payload).unwrap_or(project);
        }
        if kind == "turn_context" {
            model = first_string(payload, &["model", "model_id", "modelId"]).unwrap_or(model);
            project = project_from_value(payload).unwrap_or(project);
        }
        if persist {
            store.append_record(&IngestRecord {
                record_id: stable_id(&format!("record:{line_id}")),
                source_path: path.to_string_lossy().into_owned(),
                line_number: line_number as i64,
                occurred_at,
                provider_id: "codex".into(),
                agent_name: "codex".into(),
                session_id: Some(session_id.clone()),
                event_type: kind.clone(),
                payload_type: first_string(payload, &["type", "event_type", "eventType", "kind"]),
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
        }
        if matches!(
            kind.as_str(),
            "session_meta" | "turn_context" | "session.started" | "turn.started"
        ) {
            continue;
        }
        let Some(occurred_at) = occurred_at else {
            continue;
        };
        if matches!(
            kind.as_str(),
            "response_item" | "response.item" | "tool_call"
        ) && matches!(
            payload.get("type").and_then(Value::as_str),
            Some("custom_tool_call") | Some("function_call")
        ) {
            if let Some(name) = payload.get("name").and_then(Value::as_str) {
                if persist {
                    append_metric(
                        store,
                        &session_id,
                        occurred_at,
                        "tool",
                        name,
                        &format!("{line_id}:tool:{name}"),
                    )?;
                }
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
                    if persist {
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
            }
            continue;
        }
        if kind == "event_msg"
            && matches!(
                first_string(payload, &["type", "event_type", "eventType", "kind"]).as_deref(),
                Some("user_message" | "user.message" | "prompt")
            )
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
            if persist && append_event(store, &root, &usage)? {
                stats.events_inserted += 1;
            }
            continue;
        }
        if kind == "event_msg"
            && matches!(
                first_string(payload, &["type", "event_type", "eventType", "kind"]).as_deref(),
                Some("token_count" | "token.count" | "usage" | "token_usage")
            )
        {
            let current = counters(payload);
            if persist {
                stats.token_records += 1;
            }
            if current == Counters::default() {
                continue;
            }
            let delta = current.saturating_sub(previous);
            previous = current;
            if delta.total <= 0 {
                continue;
            }
            let event_model = first_string(payload, &["model", "model_id", "modelId"])
                .unwrap_or_else(|| model.clone());
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
            if persist && append_event(store, &root, &usage)? {
                stats.events_inserted += 1;
            }
            continue;
        }
        if matches!(
            kind.as_str(),
            "response_item" | "response.item" | "tool_call"
        ) && matches!(
            first_string(payload, &["type", "event_type", "eventType", "kind"]).as_deref(),
            Some("custom_tool_call" | "function_call" | "tool_call")
        ) && payload.get("name").and_then(Value::as_str) == Some("apply_patch")
            && let Some(input) = payload.get("input").and_then(Value::as_str)
        {
            let (added, removed) = patch_counts(input);
            if !persist || (added == 0 && removed == 0) {
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
    Ok(last_complete_offset)
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
    let raw = [
        "cwd",
        "project",
        "workspace",
        "workspace_id",
        "workspaceId",
        "workdir",
        "working_directory",
    ]
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

fn event_kind(root: &Value) -> String {
    first_string(root, &["type", "event_type", "eventType", "kind"])
        .or_else(|| {
            first_string(
                event_payload(root),
                &["type", "event_type", "eventType", "kind"],
            )
        })
        .unwrap_or_else(|| "unknown".into())
}

fn event_payload(root: &Value) -> &Value {
    ["payload", "data", "event", "details"]
        .iter()
        .find_map(|key| root.get(*key).filter(|value| value.is_object()))
        .unwrap_or(root)
}

fn event_timestamp(root: &Value) -> Option<DateTime<Utc>> {
    let value = [root, event_payload(root)]
        .into_iter()
        .flat_map(|value| {
            [
                "timestamp",
                "time",
                "occurred_at",
                "occurredAt",
                "created_at",
                "createdAt",
            ]
            .into_iter()
            .filter_map(move |key| value.get(key))
        })
        .next()?;
    if let Some(value) = value.as_str() {
        return DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|value| value.with_timezone(&Utc));
    }
    value
        .as_i64()
        .and_then(|millis| DateTime::from_timestamp_millis(millis))
}

fn counters(value: &Value) -> Counters {
    let usage = [
        value.pointer("/info/total_token_usage"),
        value.pointer("/info/usage"),
        value.pointer("/total_token_usage"),
        value.pointer("/token_usage"),
        value.pointer("/usage"),
        Some(value),
    ]
    .into_iter()
    .flatten()
    .find(|candidate| {
        [
            "input_tokens",
            "inputTokens",
            "prompt_tokens",
            "promptTokens",
        ]
        .iter()
        .any(|key| candidate.get(*key).is_some())
    });
    let Some(usage) = usage else {
        return Counters::default();
    };
    Counters {
        input: number_any(
            usage,
            &[
                "input_tokens",
                "inputTokens",
                "prompt_tokens",
                "promptTokens",
            ],
        ),
        output: number_any(
            usage,
            &[
                "output_tokens",
                "outputTokens",
                "completion_tokens",
                "completionTokens",
            ],
        ),
        reasoning: number_any(
            usage,
            &[
                "reasoning_output_tokens",
                "reasoning_tokens",
                "reasoningTokens",
            ],
        ),
        cached: number_any(
            usage,
            &[
                "cached_input_tokens",
                "cache_read_tokens",
                "cachedInputTokens",
                "cacheReadTokens",
            ],
        ),
        cache_write: number_any(usage, &["cache_write_tokens", "cacheWriteTokens"]),
        total: number_any(usage, &["total_tokens", "totalTokens", "total"]),
    }
}

fn number_any(value: &Value, keys: &[&str]) -> i64 {
    keys.iter()
        .find_map(|key| {
            value.get(*key).and_then(|value| {
                value
                    .as_i64()
                    .or_else(|| value.as_f64().map(|value| value as i64))
            })
        })
        .unwrap_or_default()
}

fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value_at(value, key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    })
}

fn value_at<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    key.split('.')
        .try_fold(value, |current, part| current.get(part))
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

    use crate::storage::{UsageStore, sqlite::SqliteStore};

    #[test]
    fn ingests_only_appended_lines_and_keeps_token_deltas() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(file, r#"{{"timestamp":"2026-07-19T05:00:00Z","type":"session_meta","payload":{{"model":"gpt-5.5"}}}}"#).unwrap();
        writeln!(file, r#"{{"timestamp":"2026-07-19T05:01:00Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}}}}}}"#).unwrap();

        let mut store = SqliteStore::open_in_memory().unwrap();
        let first = ingest_into_store(Some(dir.path().to_str().unwrap()), &mut store).unwrap();
        assert_eq!(first.token_records, 1);

        writeln!(file, r#"{{"timestamp":"2026-07-19T05:02:00Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":20,"output_tokens":10,"total_tokens":30}}}}}}}}"#).unwrap();
        file.flush().unwrap();

        let second = ingest_into_store(Some(dir.path().to_str().unwrap()), &mut store).unwrap();
        assert_eq!(second.token_records, 1);
        let summary = store
            .summary_for_agent(
                Some("codex"),
                "2026-07-19T00:00:00Z".parse().unwrap(),
                "2026-07-20T00:00:00Z".parse().unwrap(),
            )
            .unwrap();
        assert_eq!(summary.total_tokens, 30);
        assert_eq!(summary.requests, 2);
    }

    #[test]
    fn accepts_alternate_codex_event_attributes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("alternate.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(
            file,
            r#"{{"eventType":"session_meta","time":"2026-07-19T05:00:00Z","data":{{"modelId":"gpt-5.5","workspace":"/tmp/flexible-project"}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"eventType":"event_msg","time":"2026-07-19T05:01:00Z","data":{{"eventType":"token.count","usage":{{"inputTokens":10,"outputTokens":5,"totalTokens":15}},"rateLimits":{{"primary":{{"usedPercent":42.5,"windowMinutes":300,"resetsAt":1784476800}}}}}}}}"#
        )
        .unwrap();

        let mut store = SqliteStore::open_in_memory().unwrap();
        let stats = ingest_into_store(Some(dir.path().to_str().unwrap()), &mut store).unwrap();
        assert_eq!(stats.token_records, 1);
        let summary = store
            .summary_for_agent(
                Some("codex"),
                "2026-07-19T00:00:00Z".parse().unwrap(),
                "2026-07-20T00:00:00Z".parse().unwrap(),
            )
            .unwrap();
        assert_eq!(summary.total_tokens, 15);
        assert!(summary.models.contains_key("gpt-5.5"));
        assert!(summary.projects.contains_key("flexible-project"));
        assert_eq!(summary.primary_used_percent, Some(42.5));
        assert_eq!(summary.primary_window_minutes, Some(300));

        let live_status = store
            .summary_for_agent(
                Some("codex"),
                "2026-07-20T00:00:00Z".parse().unwrap(),
                "2026-07-21T00:00:00Z".parse().unwrap(),
            )
            .unwrap();
        assert_eq!(live_status.total_tokens, 0);
        assert_eq!(live_status.primary_used_percent, Some(42.5));
    }
}
