use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use crate::storage::{FileCursor, IngestRecord, RawEvent, UsageEvent, UsageMetric, UsageStore};

const CURSOR_VERSION: &str = "pi-v2";

#[derive(Debug, Default, Clone, Copy)]
pub struct IngestStats {
    pub files_scanned: usize,
    pub files_with_usage: usize,
    pub token_records: usize,
    pub malformed_lines: usize,
    pub events_inserted: usize,
}

#[derive(Debug, Default, Clone, Copy)]
struct UsageValues {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    total: i64,
    cost: f64,
}

pub fn ingest_into_store<S: UsageStore>(
    sessions_dir: Option<&str>,
    store: &mut S,
) -> Result<IngestStats> {
    let root = sessions_dir
        .map(PathBuf::from)
        .or_else(|| env::var_os("PI_CODING_AGENT_SESSION_DIR").map(PathBuf::from))
        .unwrap_or_else(default_sessions_dir);
    let mut stats = IngestStats::default();
    store.begin_batch()?;
    for path in jsonl_files(&root)? {
        stats.files_scanned += 1;
        let file_size = fs::metadata(&path)?.len() as i64;
        let key = path.to_string_lossy().into_owned();
        let cursor = store.cursor(&key)?;
        if cursor.as_ref().is_some_and(|value| {
            value.file_size == file_size
                && value.byte_offset == file_size
                && value.last_event_hash.as_deref() == Some(CURSOR_VERSION)
        }) {
            continue;
        }
        let start_offset = cursor
            .filter(|value| value.byte_offset >= 0 && value.byte_offset <= file_size)
            .map(|value| value.byte_offset as u64)
            .unwrap_or(0);
        let before = stats.events_inserted + stats.token_records;
        let processed_offset = ingest_file(&path, start_offset, store, &mut stats)?;
        if stats.events_inserted + stats.token_records > before {
            stats.files_with_usage += 1;
        }
        store.save_cursor(&FileCursor {
            path: key,
            byte_offset: processed_offset as i64,
            file_size,
            last_event_hash: Some(CURSOR_VERSION.into()),
            updated_at: Utc::now(),
        })?;
    }
    store.end_batch()?;
    Ok(stats)
}

fn ingest_file<S: UsageStore>(
    path: &Path,
    start_offset: u64,
    store: &mut S,
    stats: &mut IngestStats,
) -> Result<u64> {
    let file = fs::File::open(path).with_context(|| format!("read {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let fallback_session = stable_id(&format!("pi-session:{}", path.display()));
    let mut session_id = fallback_session.clone();
    let mut project = String::new();
    let mut model = String::from("unknown");
    let mut provider = String::from("pi");
    let mut byte_offset = 0u64;
    let mut last_complete_offset = 0u64;
    let mut line_number = 0usize;
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
        let current_line = line_number;
        line_number += 1;
        let line = buffer.trim_end_matches(['\r', '\n']);
        let Ok(root) = serde_json::from_str::<Value>(line) else {
            if persist {
                stats.malformed_lines += 1;
            }
            continue;
        };

        if root.get("type").and_then(Value::as_str) == Some("session") {
            session_id = string(&root, &["id"]).unwrap_or(session_id);
            project = project_from_value(&root).unwrap_or(project);
        }
        if root.get("type").and_then(Value::as_str) == Some("model_change") {
            provider = string(&root, &["provider"]).unwrap_or(provider);
            model = string(&root, &["modelId", "model"]).unwrap_or(model);
        }

        let entry_id = string(&root, &["id"]).unwrap_or_else(|| current_line.to_string());
        let entry_time = timestamp(&root);
        let message = root.get("message");
        if let Some(message) = message {
            model = string(message, &["model"]).unwrap_or(model);
            provider = string(message, &["provider"]).unwrap_or(provider);
            if project.is_empty() {
                project = project_from_value(&root).unwrap_or_default();
            }
        }
        if persist {
            store.append_record(&IngestRecord {
                record_id: stable_id(&format!("pi-record:{}:{}", path.display(), entry_id)),
                source_path: path.to_string_lossy().into_owned(),
                line_number: (current_line + 1) as i64,
                occurred_at: entry_time,
                provider_id: provider.clone(),
                agent_name: "pi".into(),
                session_id: Some(session_id.clone()),
                event_type: string(&root, &["type"]).unwrap_or_else(|| "unknown".into()),
                payload_type: message.and_then(|value| string(value, &["role"])),
                model: Some(model.clone()),
                client: Some("CLI".into()),
                project: (!project.is_empty()).then(|| project.clone()),
                tool_name: None,
                payload: root.clone(),
                dedup_key: format!("pi-record:{}:{}", path.display(), entry_id),
            })?;
        }

        if let Some(message) = message {
            let role = string(message, &["role"]).unwrap_or_default();
            match role.as_str() {
                "user" => {
                    if let Some(at) = entry_time.or_else(|| timestamp(message))
                        && persist
                    {
                        let usage = UsageEvent {
                            event_id: stable_id(&format!("pi-prompt:{session_id}:{entry_id}")),
                            occurred_at: at,
                            provider_id: provider.clone(),
                            agent_name: "pi".into(),
                            session_id: Some(session_id.clone()),
                            model: Some(display_model(&provider, &model)),
                            client: Some("CLI".into()),
                            project: (!project.is_empty()).then(|| project.clone()),
                            prompts: 1,
                            dedup_key: format!("pi-prompt:{session_id}:{entry_id}"),
                            raw_event_id: stable_id(&format!(
                                "pi-raw-prompt:{session_id}:{entry_id}"
                            )),
                            ..Default::default()
                        };
                        if append_event(store, &root, &usage)? {
                            stats.events_inserted += 1;
                        }
                    }
                }
                "assistant" => {
                    append_message_usage(
                        store,
                        &root,
                        message,
                        &session_id,
                        &entry_id,
                        entry_time,
                        &provider,
                        &model,
                        &project,
                        persist,
                        stats,
                    )?;
                    if persist {
                        append_tool_metrics(store, message, &session_id, &entry_id, entry_time)?;
                    }
                }
                "toolResult" => {
                    if let Some(usage) = message.get("usage") {
                        append_usage(
                            store,
                            &root,
                            usage,
                            &session_id,
                            &format!("{entry_id}:tool"),
                            entry_time,
                            &provider,
                            &model,
                            &project,
                            persist,
                            stats,
                        )?;
                    }
                }
                _ => {}
            }
        }

        if matches!(
            root.get("type").and_then(Value::as_str),
            Some("compaction" | "branch_summary")
        ) && let Some(usage) = root.get("usage")
        {
            append_usage(
                store,
                &root,
                usage,
                &session_id,
                &entry_id,
                entry_time,
                &provider,
                &model,
                &project,
                persist,
                stats,
            )?;
        }
    }
    Ok(last_complete_offset)
}

#[allow(clippy::too_many_arguments)]
fn append_message_usage<S: UsageStore>(
    store: &mut S,
    root: &Value,
    message: &Value,
    session_id: &str,
    entry_id: &str,
    entry_time: Option<DateTime<Utc>>,
    provider: &str,
    model: &str,
    project: &str,
    persist: bool,
    stats: &mut IngestStats,
) -> Result<()> {
    let Some(usage) = message.get("usage") else {
        return Ok(());
    };
    append_usage(
        store,
        root,
        usage,
        session_id,
        entry_id,
        entry_time.or_else(|| timestamp(message)),
        provider,
        &string(message, &["model"]).unwrap_or_else(|| model.to_owned()),
        project,
        persist,
        stats,
    )
}

#[allow(clippy::too_many_arguments)]
fn append_usage<S: UsageStore>(
    store: &mut S,
    root: &Value,
    usage: &Value,
    session_id: &str,
    usage_id: &str,
    occurred_at: Option<DateTime<Utc>>,
    provider: &str,
    model: &str,
    project: &str,
    persist: bool,
    stats: &mut IngestStats,
) -> Result<()> {
    let Some(at) = occurred_at else {
        return Ok(());
    };
    let values = usage_values(usage);
    if values.total == 0 && values.input == 0 && values.output == 0 {
        return Ok(());
    }
    if persist {
        stats.token_records += 1;
    }
    if !persist {
        return Ok(());
    }
    let dedup = format!("pi-usage:{session_id}:{usage_id}");
    let event = UsageEvent {
        event_id: stable_id(&dedup),
        occurred_at: at,
        provider_id: provider.to_owned(),
        agent_name: "pi".into(),
        session_id: Some(session_id.to_owned()),
        model: Some(display_model(provider, model)),
        client: Some("CLI".into()),
        project: (!project.is_empty()).then(|| project.to_owned()),
        input_tokens: values.input,
        output_tokens: values.output,
        cache_read_tokens: values.cache_read,
        cache_write_tokens: values.cache_write,
        total_tokens: if values.total > 0 {
            values.total
        } else {
            values.input + values.output + values.cache_read + values.cache_write
        },
        cost_usd: values.cost,
        requests: 1,
        dedup_key: dedup,
        raw_event_id: stable_id(&format!("pi-raw:{session_id}:{usage_id}")),
        ..Default::default()
    };
    if append_event(store, root, &event)? {
        stats.events_inserted += 1;
    }
    Ok(())
}

fn append_tool_metrics<S: UsageStore>(
    store: &mut S,
    message: &Value,
    session_id: &str,
    entry_id: &str,
    occurred_at: Option<DateTime<Utc>>,
) -> Result<()> {
    let Some(at) = occurred_at else {
        return Ok(());
    };
    let Some(content) = message.get("content").and_then(Value::as_array) else {
        return Ok(());
    };
    for (index, block) in content.iter().enumerate() {
        if block.get("type").and_then(Value::as_str) != Some("toolCall") {
            continue;
        }
        let Some(name) = string(block, &["name"]) else {
            continue;
        };
        let dedup = format!("pi-tool:{session_id}:{entry_id}:{index}");
        store.append_metric(&UsageMetric {
            metric_id: stable_id(&dedup),
            occurred_at: at,
            provider_id: "pi".into(),
            agent_name: "pi".into(),
            session_id: Some(session_id.to_owned()),
            dimension: "tool".into(),
            name,
            dedup_key: dedup,
        })?;
    }
    Ok(())
}

fn append_event<S: UsageStore>(store: &mut S, payload: &Value, event: &UsageEvent) -> Result<bool> {
    store.append_raw_event(&RawEvent {
        event_id: event.raw_event_id.clone(),
        source_system: "pi".into(),
        source_channel: "jsonl".into(),
        occurred_at: event.occurred_at,
        payload: payload.clone(),
        payload_hash: stable_id(&serde_json::to_string(payload)?),
    })?;
    store.append_usage_event(event)
}

fn usage_values(value: &Value) -> UsageValues {
    let input = number(value, &["input"]);
    let output = number(value, &["output"]);
    let cache_read = number(value, &["cacheRead"]);
    let cache_write = number(value, &["cacheWrite"]);
    let total = number(value, &["totalTokens"]);
    let cost = value
        .get("cost")
        .and_then(|cost| number_f64(cost, &["total"]))
        .unwrap_or_default();
    UsageValues {
        input,
        output,
        cache_read,
        cache_write,
        total,
        cost,
    }
}

fn display_model(provider: &str, model: &str) -> String {
    if provider.is_empty() || provider == "pi" {
        model.to_owned()
    } else {
        format!("{provider}:{model}")
    }
}

fn number(value: &Value, keys: &[&str]) -> i64 {
    keys.iter()
        .find_map(|key| {
            value
                .get(*key)
                .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|n| n as i64)))
        })
        .unwrap_or_default()
}

fn number_f64(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)))
    })
}

fn string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    })
}

fn timestamp(value: &Value) -> Option<DateTime<Utc>> {
    let raw = value.get("timestamp")?;
    if let Some(value) = raw.as_str() {
        return DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|value| value.with_timezone(&Utc));
    }
    raw.as_i64().and_then(DateTime::from_timestamp_millis)
}

fn project_from_value(value: &Value) -> Option<String> {
    let raw = string(value, &["cwd", "project", "workspace"])?;
    Path::new(&raw)
        .file_name()
        .and_then(|v| v.to_str())
        .map(str::to_owned)
        .or(Some(raw))
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
    PathBuf::from(home)
        .join(".pi")
        .join("agent")
        .join("sessions")
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
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    use crate::storage::{UsageStore, sqlite::SqliteStore};

    #[test]
    fn ingests_pi_usage_prompts_tools_and_provider_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(file, r#"{{"type":"session","version":3,"id":"session-1","timestamp":"2026-07-19T05:00:00Z","cwd":"/tmp/pi-project"}}"#).unwrap();
        writeln!(file, r#"{{"type":"model_change","id":"m1","timestamp":"2026-07-19T05:00:01Z","provider":"openai-codex","modelId":"gpt-5.6-luna"}}"#).unwrap();
        writeln!(file, r#"{{"type":"message","id":"u1","timestamp":"2026-07-19T05:01:00Z","message":{{"role":"user","content":"Fix it"}}}}"#).unwrap();
        writeln!(file, r#"{{"type":"message","id":"a1","timestamp":"2026-07-19T05:02:00Z","message":{{"role":"assistant","provider":"anthropic","model":"claude-sonnet","content":[{{"type":"toolCall","id":"t1","name":"bash","arguments":{{}}}}],"usage":{{"input":10,"output":4,"cacheRead":2,"cacheWrite":1,"totalTokens":17,"cost":{{"total":0.123}}}}}}}}"#).unwrap();

        let mut store = SqliteStore::open_in_memory().unwrap();
        let stats = ingest_into_store(Some(dir.path().to_str().unwrap()), &mut store).unwrap();
        assert_eq!(stats.token_records, 1);
        let summary = store
            .summary_for_agent(
                Some("pi"),
                "2026-07-19T00:00:00Z".parse().unwrap(),
                "2026-07-20T00:00:00Z".parse().unwrap(),
            )
            .unwrap();
        assert_eq!(summary.prompts, 1);
        assert_eq!(summary.requests, 1);
        assert_eq!(summary.total_tokens, 17);
        assert_eq!(summary.cost_usd, 0.123);
        assert!(summary.models.contains_key("anthropic:claude-sonnet"));
        assert!(!summary.models.contains_key("unknown"));
        assert!(!summary.providers.contains_key("pi"));
        assert!(summary.providers.contains_key("openai-codex"));
        assert!(summary.projects.contains_key("pi-project"));
        assert_eq!(summary.tools.get("bash"), Some(&1));
    }

    #[test]
    fn is_incremental_idempotent_and_counts_nested_usage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(file, r#"{{"type":"session","id":"session-1","timestamp":"2026-07-19T05:00:00Z","cwd":"/tmp/pi-project"}}"#).unwrap();
        writeln!(file, r#"{{"type":"message","id":"t1","timestamp":"2026-07-19T05:02:00Z","message":{{"role":"toolResult","usage":{{"input":3,"output":2,"totalTokens":5,"cost":{{"total":0.01}}}}}}}}"#).unwrap();
        writeln!(file, "not json").unwrap();
        let mut store = SqliteStore::open_in_memory().unwrap();
        let first = ingest_into_store(Some(dir.path().to_str().unwrap()), &mut store).unwrap();
        let second = ingest_into_store(Some(dir.path().to_str().unwrap()), &mut store).unwrap();
        assert_eq!(first.token_records, 1);
        assert_eq!(first.malformed_lines, 1);
        assert_eq!(second.token_records, 0);
        assert_eq!(
            store
                .summary_for_agent(
                    Some("pi"),
                    "2026-07-19T00:00:00Z".parse().unwrap(),
                    "2026-07-20T00:00:00Z".parse().unwrap()
                )
                .unwrap()
                .total_tokens,
            5
        );
    }
}
