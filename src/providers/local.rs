use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

use crate::storage::{FileCursor, RawEvent, UsageEvent, UsageStore};

#[derive(Debug, Deserialize)]
struct DesktopHistory {
    samples: Vec<DesktopSample>,
}

#[derive(Debug, Deserialize)]
struct DesktopSample {
    t: i64,
    u: DesktopSignals,
}

#[derive(Debug, Deserialize)]
struct DesktopSignals {
    #[serde(rename = "fh")]
    five_hour: i64,
    #[serde(rename = "sd")]
    seven_day: i64,
}

#[derive(Debug, Clone, Copy)]
pub enum Agent {
    ClaudeCode,
    OpenCode,
}

impl Agent {
    pub fn id(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude_code",
            Self::OpenCode => "opencode",
        }
    }
}

pub fn default_dir(agent: Agent) -> PathBuf {
    let home = env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .unwrap_or_default();
    match agent {
        Agent::ClaudeCode => PathBuf::from(home).join(".claude").join("projects"),
        Agent::OpenCode => {
            let preferred = PathBuf::from(&home).join(".opencode").join("events");
            if preferred.exists() {
                preferred
            } else {
                PathBuf::from(home)
                    .join(".local")
                    .join("state")
                    .join("opencode")
                    .join("events")
            }
        }
    }
}

pub fn desktop_usage() -> Option<crate::providers::codex::DesktopUsage> {
    let home = env::var("HOME").or_else(|_| env::var("USERPROFILE")).ok()?;
    let path = if cfg!(target_os = "macos") {
        PathBuf::from(&home).join("Library/Application Support/Claude/plan-usage-history.json")
    } else if cfg!(target_os = "windows") {
        PathBuf::from(&home).join("AppData/Roaming/Claude/plan-usage-history.json")
    } else {
        PathBuf::from(&home).join(".config/Claude/plan-usage-history.json")
    };
    let history = serde_json::from_str::<DesktopHistory>(&fs::read_to_string(path).ok()?).ok()?;
    let latest = history.samples.iter().max_by_key(|sample| sample.t)?;
    Some(crate::providers::codex::DesktopUsage {
        samples: history.samples.len(),
        latest_timestamp_ms: latest.t,
        five_hour_signal: latest.u.five_hour,
        seven_day_signal: latest.u.seven_day,
    })
}

pub fn ingest_into_store<S: UsageStore>(
    agent: Agent,
    dir: Option<&str>,
    store: &mut S,
) -> Result<(usize, usize, usize, usize)> {
    let root = dir.map(PathBuf::from).unwrap_or_else(|| default_dir(agent));
    let mut files = 0;
    let mut active = 0;
    let mut records = 0;
    let mut malformed = 0;
    for path in jsonl_files(&root)? {
        files += 1;
        let size = fs::metadata(&path)?.len() as i64;
        let key = path.to_string_lossy().into_owned();
        if let Some(cursor) = store.cursor(&key)?
            && cursor.file_size == size
            && cursor.last_event_hash.as_deref() == Some("project-v2")
        {
            continue;
        }
        let before = records;
        match agent {
            Agent::ClaudeCode => ingest_claude_file(&path, store, &mut records, &mut malformed)?,
            Agent::OpenCode => ingest_opencode_file(&path, store, &mut records, &mut malformed)?,
        }
        if records > before {
            active += 1;
        }
        store.save_cursor(&FileCursor {
            path: key,
            byte_offset: size,
            file_size: size,
            last_event_hash: Some("project-v2".into()),
            updated_at: Utc::now(),
        })?;
    }
    Ok((files, active, records, malformed))
}

fn ingest_claude_file<S: UsageStore>(
    path: &Path,
    store: &mut S,
    records: &mut usize,
    malformed: &mut usize,
) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut seen = HashSet::new();
    for (line, raw) in text.lines().enumerate() {
        let Ok(entry) = serde_json::from_str::<ClaudeEntry>(raw) else {
            *malformed += 1;
            continue;
        };
        if entry.entry_type != "assistant" {
            continue;
        }
        let Some(message) = entry.message else {
            continue;
        };
        let Some(usage) = message.usage else {
            continue;
        };
        let key = entry
            .request_id
            .clone()
            .or(message.id.clone())
            .unwrap_or_else(|| format!("{line}"));
        if !seen.insert(key.clone()) {
            continue;
        }
        let Some(at) = parse_time(entry.timestamp.as_deref()) else {
            continue;
        };
        let model = message.model.unwrap_or_else(|| "unknown".into());
        let project = entry
            .cwd
            .as_deref()
            .and_then(project_name)
            .or_else(|| project_from_path(path));
        let input = usage.input_tokens;
        let output = usage.output_tokens;
        let cache_read = usage.cache_read_input_tokens;
        let cache_write = usage.cache_creation_input_tokens;
        let total = input + output + usage.reasoning_tokens + cache_read + cache_write;
        let id = stable(&format!("claude:{path:?}:{key}"));
        let event = UsageEvent {
            event_id: id.clone(),
            occurred_at: at,
            provider_id: "anthropic".into(),
            agent_name: "claude_code".into(),
            session_id: entry.session_id,
            model: Some(model.clone()),
            client: Some("CLI".into()),
            project,
            input_tokens: input,
            output_tokens: output,
            reasoning_tokens: usage.reasoning_tokens,
            cache_read_tokens: cache_read,
            cache_write_tokens: cache_write,
            total_tokens: total,
            cost_usd: claude_cost(&model, input, output, cache_read, cache_write),
            requests: 1,
            dedup_key: id,
            raw_event_id: stable(&format!("raw:claude:{path:?}:{key}")),
            ..Default::default()
        };
        append(store, &event, serde_json::from_str(raw)?)?;
        *records += 1;
    }
    Ok(())
}

fn ingest_opencode_file<S: UsageStore>(
    path: &Path,
    store: &mut S,
    records: &mut usize,
    malformed: &mut usize,
) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    for (line, raw) in text.lines().enumerate() {
        let Ok(root) = serde_json::from_str::<Value>(raw) else {
            *malformed += 1;
            continue;
        };
        let kind = root
            .get("type")
            .or_else(|| root.get("event"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if kind != "message.updated" {
            continue;
        }
        let info = root
            .pointer("/properties/info")
            .or_else(|| root.pointer("/payload/info"));
        let Some(info) = info else {
            continue;
        };
        if info.get("role").and_then(Value::as_str).unwrap_or_default() != "assistant" {
            continue;
        }
        let session = string(info, "sessionID");
        let message = string(info, "id").unwrap_or_else(|| format!("{path:?}:{line}"));
        let id = stable(&format!("opencode:{message}"));
        let input = number(info.pointer("/tokens/input"));
        let output = number(info.pointer("/tokens/output"));
        let reasoning = number(info.pointer("/tokens/reasoning"));
        let cache_read = number(info.pointer("/tokens/cache/read"));
        let cache_write = number(info.pointer("/tokens/cache/write"));
        let total = input + output + reasoning + cache_read + cache_write;
        let at = info
            .pointer("/time/completed")
            .and_then(Value::as_i64)
            .or_else(|| info.pointer("/time/created").and_then(Value::as_i64))
            .map(unix_time)
            .unwrap_or_else(Utc::now);
        let event = UsageEvent {
            event_id: id.clone(),
            occurred_at: at,
            provider_id: string(info, "providerID").unwrap_or_else(|| "opencode".into()),
            agent_name: "opencode".into(),
            session_id: session,
            model: string(info, "modelID"),
            client: Some("OpenCode".into()),
            project: string(info, "cwd")
                .or_else(|| string(info, "workspace"))
                .as_deref()
                .and_then(project_name),
            input_tokens: input,
            output_tokens: output,
            reasoning_tokens: reasoning,
            cache_read_tokens: cache_read,
            cache_write_tokens: cache_write,
            total_tokens: total,
            cost_usd: number_f64(info, "cost"),
            requests: 1,
            dedup_key: id,
            raw_event_id: stable(&format!("raw:opencode:{message}")),
            ..Default::default()
        };
        append(store, &event, root)?;
        *records += 1;
    }
    Ok(())
}

#[derive(Deserialize)]
struct ClaudeEntry {
    #[serde(rename = "type")]
    entry_type: String,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    message: Option<ClaudeMessage>,
    #[serde(default)]
    cwd: Option<String>,
}
#[derive(Deserialize)]
struct ClaudeMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<ClaudeUsage>,
}
#[derive(Deserialize)]
struct ClaudeUsage {
    input_tokens: i64,
    output_tokens: i64,
    #[serde(default)]
    reasoning_tokens: i64,
    #[serde(default)]
    cache_read_input_tokens: i64,
    #[serde(default)]
    cache_creation_input_tokens: i64,
}

fn append<S: UsageStore>(store: &mut S, event: &UsageEvent, payload: Value) -> Result<()> {
    store.append_raw_event(&RawEvent {
        event_id: event.raw_event_id.clone(),
        source_system: event.agent_name.clone(),
        source_channel: "jsonl".into(),
        occurred_at: event.occurred_at,
        payload: payload.clone(),
        payload_hash: stable(&serde_json::to_string(&payload)?),
    })?;
    store.append_usage_event(event)?;
    Ok(())
}
fn jsonl_files(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(root)? {
        let p = entry?.path();
        if p.is_dir() {
            out.extend(jsonl_files(&p)?);
        } else if matches!(
            p.extension().and_then(|v| v.to_str()),
            Some("jsonl" | "ndjson")
        ) {
            out.push(p);
        }
    }
    Ok(out)
}
fn string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

fn project_name(value: &str) -> Option<String> {
    let path = Path::new(value.trim());
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

fn project_from_path(path: &Path) -> Option<String> {
    path.parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}
fn number(value: Option<&Value>) -> i64 {
    value.and_then(Value::as_i64).unwrap_or_default()
}
fn number_f64(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or_default()
}
fn parse_time(value: Option<&str>) -> Option<DateTime<Utc>> {
    value
        .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
        .map(|v| v.with_timezone(&Utc))
}
fn unix_time(value: i64) -> DateTime<Utc> {
    if value > 10_000_000_000 {
        DateTime::from_timestamp_millis(value).unwrap_or_else(Utc::now)
    } else {
        DateTime::from_timestamp(value, 0).unwrap_or_else(Utc::now)
    }
}
fn stable(value: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(value.as_bytes());
    hex::encode(hash.finalize())
}
fn claude_cost(model: &str, input: i64, output: i64, cache_read: i64, cache_write: i64) -> f64 {
    let m = model.to_ascii_lowercase();
    let (i, o, r, w) = if m.contains("opus") {
        (15.0, 75.0, 1.5, 18.75)
    } else if m.contains("haiku") {
        (0.8, 4.0, 0.08, 1.0)
    } else {
        (3.0, 15.0, 0.3, 3.75)
    };
    (input as f64 * i + output as f64 * o + cache_read as f64 * r + cache_write as f64 * w)
        / 1_000_000.0
}
