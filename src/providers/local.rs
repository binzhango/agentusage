use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
};

use crate::core::TokenSemantics;
use crate::storage::{FileCursor, RawEvent, UsageEvent, UsageStore};

#[derive(Debug, Clone, Copy)]
pub enum Agent {
    ClaudeCode,
    OpenCode,
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
            && cursor.last_event_hash.as_deref() == Some("prompts-v3")
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
            last_event_hash: Some("prompts-v3".into()),
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
        let Ok(payload) = serde_json::from_str::<Value>(raw) else {
            *malformed += 1;
            continue;
        };
        let Ok(entry) = serde_json::from_value::<ClaudeEntry>(payload.clone()) else {
            *malformed += 1;
            continue;
        };
        if entry.entry_type == "user"
            && payload.get("isMeta").and_then(Value::as_bool) != Some(true)
        {
            let Some(message) = entry.message.as_ref() else {
                continue;
            };
            if !message.content.as_ref().is_some_and(has_prompt_text) {
                continue;
            }
            let Some(at) = parse_time(entry.timestamp.as_deref()) else {
                *malformed += 1;
                continue;
            };
            let key = entry
                .uuid
                .clone()
                .or_else(|| message.id.clone())
                .unwrap_or_else(|| line.to_string());
            let id = stable(&format!("claude-prompt:{path:?}:{key}"));
            let event = UsageEvent {
                event_id: id.clone(),
                occurred_at: at,
                provider_id: "anthropic".into(),
                agent_name: "claude_code".into(),
                session_id: entry.session_id.clone(),
                model: message.model.clone(),
                client: Some("CLI".into()),
                project: entry
                    .cwd
                    .as_deref()
                    .and_then(project_name)
                    .or_else(|| project_from_path(path)),
                prompts: 1,
                dedup_key: id,
                raw_event_id: stable(&format!("raw:claude-prompt:{path:?}:{key}")),
                ..Default::default()
            };
            append(store, &event, payload)?;
            continue;
        }
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
            *malformed += 1;
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
        let total = TokenSemantics::Anthropic.total(
            input,
            output,
            usage.reasoning_tokens,
            cache_read,
            cache_write,
        );
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
        append(store, &event, payload)?;
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
    let mut prompts = BTreeMap::<String, OpenCodePrompt>::new();
    let mut prompt_parts = BTreeMap::<String, BTreeMap<String, Value>>::new();
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
        if kind == "message.part.updated" {
            let part = root
                .pointer("/properties/part")
                .or_else(|| root.pointer("/payload/part"));
            if let Some(part) = part
                && part.get("type").and_then(Value::as_str) == Some("text")
                && part
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty())
                && let Some(message_id) = string(part, "messageID")
            {
                let part_id = string(part, "id").unwrap_or_else(|| line.to_string());
                prompt_parts
                    .entry(message_id)
                    .or_default()
                    .insert(part_id, part.clone());
            }
            continue;
        }
        if kind != "message.updated" {
            continue;
        }
        let info = root
            .pointer("/properties/info")
            .or_else(|| root.pointer("/payload/info"));
        let Some(info) = info else {
            continue;
        };
        let role = info.get("role").and_then(Value::as_str).unwrap_or_default();
        if role == "user" {
            let Some(message_id) = string(info, "id") else {
                continue;
            };
            let Some(raw_time) = info.pointer("/time/created").and_then(Value::as_i64) else {
                *malformed += 1;
                continue;
            };
            let Some(occurred_at) = unix_time(raw_time) else {
                *malformed += 1;
                continue;
            };
            prompts.insert(
                message_id,
                OpenCodePrompt {
                    occurred_at,
                    session_id: string(info, "sessionID"),
                    provider_id: info
                        .pointer("/model/providerID")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| "opencode".into()),
                    model: info
                        .pointer("/model/modelID")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    raw_message: info.clone(),
                    line_number: line + 1,
                },
            );
            continue;
        }
        if role != "assistant" {
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
        let Some(raw_time) = info
            .pointer("/time/completed")
            .and_then(Value::as_i64)
            .or_else(|| info.pointer("/time/created").and_then(Value::as_i64))
        else {
            *malformed += 1;
            continue;
        };
        let Some(at) = unix_time(raw_time) else {
            *malformed += 1;
            continue;
        };
        let total =
            TokenSemantics::Additive.total(input, output, reasoning, cache_read, cache_write);
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
        upsert_snapshot(store, &event, root)?;
        *records += 1;
    }
    for (message_id, prompt) in prompts {
        let Some(parts) = prompt_parts.remove(&message_id) else {
            continue;
        };
        let parts = parts.into_values().collect::<Vec<_>>();
        if parts.is_empty() {
            continue;
        }
        let id = stable(&format!("opencode-prompt:{message_id}"));
        let event = UsageEvent {
            event_id: id.clone(),
            occurred_at: prompt.occurred_at,
            provider_id: prompt.provider_id,
            agent_name: "opencode".into(),
            session_id: prompt.session_id,
            model: prompt.model,
            client: Some("OpenCode".into()),
            project: project_from_path(path),
            prompts: 1,
            dedup_key: id,
            raw_event_id: stable(&format!("raw:opencode-prompt:{message_id}")),
            ..Default::default()
        };
        upsert_snapshot(
            store,
            &event,
            serde_json::json!({
                "message": prompt.raw_message,
                "parts": parts,
                "source_path": path.to_string_lossy(),
                "line_number": prompt.line_number,
            }),
        )?;
    }
    Ok(())
}

struct OpenCodePrompt {
    occurred_at: DateTime<Utc>,
    session_id: Option<String>,
    provider_id: String,
    model: Option<String>,
    raw_message: Value,
    line_number: usize,
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
    uuid: Option<String>,
    message: Option<ClaudeMessage>,
    #[serde(default)]
    cwd: Option<String>,
}
#[derive(Deserialize)]
struct ClaudeMessage {
    id: Option<String>,
    model: Option<String>,
    content: Option<Value>,
    usage: Option<ClaudeUsage>,
}

fn has_prompt_text(value: &Value) -> bool {
    match value {
        Value::String(text) => !text.trim().is_empty(),
        Value::Array(values) => values.iter().any(has_prompt_text),
        Value::Object(object) => {
            object.get("type").and_then(Value::as_str) == Some("text")
                && object
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty())
        }
        _ => false,
    }
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

fn upsert_snapshot<S: UsageStore>(store: &mut S, event: &UsageEvent, payload: Value) -> Result<()> {
    store.upsert_raw_event(&RawEvent {
        event_id: event.raw_event_id.clone(),
        source_system: event.agent_name.clone(),
        source_channel: "jsonl".into(),
        occurred_at: event.occurred_at,
        payload: payload.clone(),
        payload_hash: stable(&serde_json::to_string(&payload)?),
    })?;
    store.upsert_usage_event(event)?;
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
fn unix_time(value: i64) -> Option<DateTime<Utc>> {
    if value > 10_000_000_000 {
        DateTime::from_timestamp_millis(value)
    } else {
        DateTime::from_timestamp(value, 0)
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
    } else if m.contains("sonnet") {
        (3.0, 15.0, 0.3, 3.75)
    } else {
        return 0.0;
    };
    (input as f64 * i + output as f64 * o + cache_read as f64 * r + cache_write as f64 * w)
        / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    use crate::storage::{PromptQuery, UsageStore, sqlite::SqliteStore};

    #[test]
    fn opencode_updates_replace_partial_message_usage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        for output in [1, 9] {
            writeln!(
                file,
                r#"{{"type":"message.updated","properties":{{"info":{{"role":"assistant","id":"message-1","sessionID":"session-1","providerID":"openai","modelID":"gpt-5","tokens":{{"input":10,"output":{output},"reasoning":0,"cache":{{"read":2,"write":0}}}},"time":{{"completed":1784437200000}}}}}}}}"#
            )
            .unwrap();
        }

        let mut store = SqliteStore::open_in_memory().unwrap();
        ingest_into_store(
            Agent::OpenCode,
            Some(dir.path().to_str().unwrap()),
            &mut store,
        )
        .unwrap();
        let summary = store
            .summary_for_agent(
                Some("opencode"),
                "2026-07-19T00:00:00Z".parse().unwrap(),
                "2026-07-20T00:00:00Z".parse().unwrap(),
            )
            .unwrap();
        assert_eq!(summary.requests, 1);
        assert_eq!(summary.total_tokens, 21);
    }

    #[test]
    fn opencode_rejects_usage_without_a_source_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        fs::write(
            &path,
            r#"{"type":"message.updated","properties":{"info":{"role":"assistant","id":"message-1","tokens":{"input":10,"output":1}}}}"#,
        )
        .unwrap();
        let mut store = SqliteStore::open_in_memory().unwrap();
        let (_, _, records, malformed) = ingest_into_store(
            Agent::OpenCode,
            Some(dir.path().to_str().unwrap()),
            &mut store,
        )
        .unwrap();
        assert_eq!(records, 0);
        assert_eq!(malformed, 1);
    }

    #[test]
    fn claude_rejects_missing_timestamps_and_does_not_double_count_reasoning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::write(
            &path,
            concat!(
                r#"{"type":"assistant","sessionId":"session-1","message":{"id":"missing-time","model":"claude-sonnet","usage":{"input_tokens":10,"output_tokens":4}}}"#,
                "\n",
                r#"{"type":"assistant","sessionId":"session-1","timestamp":"2026-07-19T12:00:00Z","message":{"id":"valid","model":"claude-sonnet","usage":{"input_tokens":10,"output_tokens":4,"reasoning_tokens":2,"cache_read_input_tokens":6,"cache_creation_input_tokens":1}}}"#
            ),
        )
        .unwrap();
        let mut store = SqliteStore::open_in_memory().unwrap();
        let (_, _, records, malformed) = ingest_into_store(
            Agent::ClaudeCode,
            Some(dir.path().to_str().unwrap()),
            &mut store,
        )
        .unwrap();
        assert_eq!(records, 1);
        assert_eq!(malformed, 1);
        let summary = store
            .summary_for_agent(
                Some("claude_code"),
                "2026-07-19T00:00:00Z".parse().unwrap(),
                "2026-07-20T00:00:00Z".parse().unwrap(),
            )
            .unwrap();
        assert_eq!(summary.reasoning_tokens, 2);
        assert_eq!(summary.total_tokens, 21);
    }

    #[test]
    fn claude_indexes_user_text_without_tool_results() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::write(
            &path,
            concat!(
                r#"{"type":"user","uuid":"prompt-1","sessionId":"session-1","timestamp":"2026-07-19T12:00:00Z","cwd":"/tmp/agentusage","message":{"role":"user","content":[{"type":"text","text":"Add prompt browsing"}]}}"#,
                "\n",
                r#"{"type":"user","uuid":"tool-1","sessionId":"session-1","timestamp":"2026-07-19T12:00:01Z","message":{"role":"user","content":[{"type":"tool_result","content":"not a prompt"}]}}"#,
            ),
        )
        .unwrap();
        let mut store = SqliteStore::open_in_memory().unwrap();
        ingest_into_store(
            Agent::ClaudeCode,
            Some(dir.path().to_str().unwrap()),
            &mut store,
        )
        .unwrap();
        let prompts = store
            .prompts(
                "claude_code",
                &PromptQuery {
                    from: "2026-07-19T00:00:00Z".parse().unwrap(),
                    to: "2026-07-20T00:00:00Z".parse().unwrap(),
                    before: None,
                    limit: 10,
                    session_id: None,
                    search: None,
                },
            )
            .unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].text, "Add prompt browsing");
        assert_eq!(prompts[0].usage.project.as_deref(), Some("agentusage"));
    }

    #[test]
    fn opencode_combines_user_text_parts_into_one_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        fs::write(
            &path,
            concat!(
                r#"{"type":"message.updated","properties":{"info":{"role":"user","id":"message-1","sessionID":"session-1","model":{"providerID":"openai","modelID":"gpt-5"},"time":{"created":1784437200000}}}}"#,
                "\n",
                r#"{"type":"message.part.updated","properties":{"part":{"type":"text","id":"part-1","messageID":"message-1","sessionID":"session-1","text":"Build the prompt API"}}}"#,
                "\n",
                r#"{"type":"message.part.updated","properties":{"part":{"type":"text","id":"part-2","messageID":"message-1","sessionID":"session-1","text":"and add pagination"}}}"#,
            ),
        )
        .unwrap();
        let mut store = SqliteStore::open_in_memory().unwrap();
        ingest_into_store(
            Agent::OpenCode,
            Some(dir.path().to_str().unwrap()),
            &mut store,
        )
        .unwrap();
        let prompts = store
            .prompts(
                "opencode",
                &PromptQuery {
                    from: "2026-07-19T00:00:00Z".parse().unwrap(),
                    to: "2026-07-20T00:00:00Z".parse().unwrap(),
                    before: None,
                    limit: 10,
                    session_id: None,
                    search: Some("pagination".into()),
                },
            )
            .unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].text, "Build the prompt API\nand add pagination");
        assert_eq!(prompts[0].usage.provider_id, "openai");
    }
}
