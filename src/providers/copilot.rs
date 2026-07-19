use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

use crate::storage::{RawEvent, UsageEvent, UsageStore};

#[derive(Debug, Default, Clone, Copy)]
pub struct IngestStats {
    pub files_scanned: usize,
    pub files_with_usage: usize,
    pub token_records: usize,
    pub malformed_lines: usize,
}

type VscodeChatRequest = (String, String, DateTime<Utc>, f64, i64, i64, Value);

pub fn default_db_path() -> PathBuf {
    if let Ok(home) = env::var("COPILOT_HOME")
        && !home.trim().is_empty()
    {
        return PathBuf::from(home).join("session-store.db");
    }
    let home = env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .unwrap_or_default();
    PathBuf::from(home)
        .join(".copilot")
        .join("session-store.db")
}

pub fn default_db_paths() -> Vec<PathBuf> {
    let mut paths = vec![default_db_path()];
    if let Ok(home) = env::var("HOME").or_else(|_| env::var("USERPROFILE")) {
        #[cfg(target_os = "macos")]
        paths.push(PathBuf::from(&home).join("Library/Application Support/Code/User/globalStorage/github.copilot-chat/session-store.db"));
        #[cfg(target_os = "windows")]
        paths.push(
            PathBuf::from(&home).join(
                "AppData/Roaming/Code/User/globalStorage/github.copilot-chat/session-store.db",
            ),
        );
        #[cfg(target_os = "linux")]
        paths.push(
            PathBuf::from(&home)
                .join(".config/Code/User/globalStorage/github.copilot-chat/session-store.db"),
        );
    }
    paths.sort();
    paths.dedup();
    paths
}

pub fn ingest_into_store<S: UsageStore>(
    source: Option<&str>,
    store: &mut S,
) -> Result<IngestStats> {
    let paths = source
        .map(|path| vec![PathBuf::from(path)])
        .unwrap_or_else(default_db_paths);
    let mut total = IngestStats::default();
    for path in paths {
        if !path.exists() {
            continue;
        }
        let stats = ingest_database(&path, store)?;
        total.files_scanned += stats.files_scanned;
        total.files_with_usage += stats.files_with_usage;
        total.token_records += stats.token_records;
        total.malformed_lines += stats.malformed_lines;
    }
    let vscode_stats = ingest_vscode_chat_sessions(store)?;
    total.files_scanned += vscode_stats.files_scanned;
    total.files_with_usage += vscode_stats.files_with_usage;
    total.token_records += vscode_stats.token_records;
    let vscode_log_stats = ingest_vscode_logs(store)?;
    total.files_scanned += vscode_log_stats.files_scanned;
    total.files_with_usage += vscode_log_stats.files_with_usage;
    total.token_records += vscode_log_stats.token_records;
    Ok(total)
}

fn ingest_vscode_chat_sessions<S: UsageStore>(store: &mut S) -> Result<IngestStats> {
    let Ok(home) = env::var("HOME").or_else(|_| env::var("USERPROFILE")) else {
        return Ok(IngestStats::default());
    };
    let root = if cfg!(target_os = "macos") {
        PathBuf::from(&home).join("Library/Application Support/Code/User/workspaceStorage")
    } else if cfg!(target_os = "windows") {
        PathBuf::from(&home).join("AppData/Roaming/Code/User/workspaceStorage")
    } else {
        PathBuf::from(&home).join(".config/Code/User/workspaceStorage")
    };
    let mut stats = IngestStats::default();
    for path in chat_session_files(&root)? {
        stats.files_scanned += 1;
        let text = fs::read_to_string(&path)?;
        for (line_number, line) in text.lines().enumerate() {
            let Some((request_id, model, occurred_at, credits, input, output, payload)) =
                parse_vscode_chat_request(line)
            else {
                continue;
            };
            let event_id = stable(&format!("copilot:vscode:chat:{request_id}"));
            let raw_event_id = stable(&format!("raw:copilot:vscode:chat:{request_id}"));
            let event = UsageEvent {
                event_id: event_id.clone(),
                occurred_at,
                provider_id: "copilot".into(),
                agent_name: "copilot".into(),
                model: Some(normalize_model(&model)),
                client: Some("IDE".into()),
                input_tokens: input,
                output_tokens: output,
                total_tokens: input + output,
                ai_credits: credits,
                requests: 1,
                dedup_key: event_id,
                raw_event_id: raw_event_id.clone(),
                ..Default::default()
            };
            store.append_raw_event(&RawEvent {
                event_id: raw_event_id,
                source_system: "copilot".into(),
                source_channel: "vscode_chat_session".into(),
                occurred_at,
                payload: json!({"source": path, "line": line_number + 1, "request": payload}),
                payload_hash: stable(line),
            })?;
            if store.append_usage_event(&event)? {
                stats.token_records += 1;
            }
        }
    }
    stats.files_with_usage = usize::from(stats.token_records > 0);
    Ok(stats)
}

fn chat_session_files(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(chat_session_files(&path)?);
        } else if path.extension().and_then(|value| value.to_str()) == Some("jsonl")
            && path
                .parent()
                .and_then(Path::file_name)
                .and_then(|value| value.to_str())
                == Some("chatSessions")
        {
            files.push(path);
        }
    }
    Ok(files)
}

fn parse_vscode_chat_request(line: &str) -> Option<VscodeChatRequest> {
    let root: Value = serde_json::from_str(line).ok()?;
    if root.get("k")?.get(0)?.as_str()? != "requests" {
        return None;
    }
    let request = root.get("v")?.as_array()?.first()?.clone();
    let request_id = request.get("requestId")?.as_str()?.to_owned();
    let timestamp = request.get("timestamp")?.as_i64()?;
    let occurred_at = Utc.timestamp_millis_opt(timestamp).single()?;
    let metadata = request
        .get("result")
        .and_then(|value| value.get("metadata"));
    let result_details = request
        .get("result")
        .and_then(|value| value.get("details"))
        .and_then(Value::as_str);
    let model = metadata
        .and_then(|value| {
            value
                .get("resolvedModelName")
                .or_else(|| value.get("resolvedModel"))
        })
        .and_then(Value::as_str)
        .or_else(|| result_details.and_then(|value| value.split(" • ").next()))?
        .to_owned();
    let credits = request
        .get("copilotCredits")
        .and_then(Value::as_f64)
        .or_else(|| {
            request
                .get("result")
                .and_then(|value| value.get("details"))
                .and_then(Value::as_str)
                .and_then(|value| value.split(" • ").nth(1))
                .and_then(|value| value.strip_suffix(" credits"))
                .and_then(|value| value.parse().ok())
        })?;
    let input = request
        .get("promptTokens")
        .and_then(Value::as_i64)
        .or_else(|| {
            metadata
                .and_then(|value| value.get("promptTokens"))
                .and_then(Value::as_i64)
        })
        .unwrap_or_default();
    let output = request
        .get("completionTokens")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    Some((
        request_id,
        model,
        occurred_at,
        credits,
        input,
        output,
        request,
    ))
}

fn ingest_vscode_logs<S: UsageStore>(store: &mut S) -> Result<IngestStats> {
    let Ok(home) = env::var("HOME").or_else(|_| env::var("USERPROFILE")) else {
        return Ok(IngestStats::default());
    };
    let root = if cfg!(target_os = "macos") {
        PathBuf::from(&home).join("Library/Application Support/Code/logs")
    } else if cfg!(target_os = "windows") {
        PathBuf::from(&home).join("AppData/Roaming/Code/logs")
    } else {
        PathBuf::from(&home).join(".config/Code/logs")
    };
    let mut stats = IngestStats::default();
    for path in log_files(&root)? {
        stats.files_scanned += 1;
        let text = fs::read_to_string(&path)?;
        for (line_number, line) in text.lines().enumerate() {
            let Some((request_id, model, occurred_at)) = parse_vscode_request(line) else {
                continue;
            };
            let event_id = stable(&format!("copilot:vscode:{request_id}"));
            let event = UsageEvent {
                event_id: event_id.clone(),
                occurred_at,
                provider_id: "copilot".into(),
                agent_name: "copilot".into(),
                model: Some(normalize_model(&model)),
                client: Some("IDE".into()),
                requests: 1,
                dedup_key: event_id,
                raw_event_id: stable(&format!("raw:copilot:vscode:{request_id}")),
                ..Default::default()
            };
            let payload = json!({"source": path, "line": line_number + 1, "request_id": request_id, "model": model});
            store.append_raw_event(&RawEvent {
                event_id: event.raw_event_id.clone(),
                source_system: "copilot".into(),
                source_channel: "vscode_log".into(),
                occurred_at,
                payload,
                payload_hash: stable(line),
            })?;
            if store.append_usage_event(&event)? {
                stats.token_records += 1;
            }
        }
    }
    stats.files_with_usage = usize::from(stats.token_records > 0);
    Ok(stats)
}

fn log_files(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(log_files(&path)?);
        } else if path.file_name().and_then(|name| name.to_str()) == Some("GitHub Copilot Chat.log")
        {
            files.push(path);
        }
    }
    Ok(files)
}

fn parse_vscode_request(line: &str) -> Option<(String, String, DateTime<Utc>)> {
    let parts: Vec<&str> = line.split(" | ").collect();
    if parts.len() < 4
        || parts[1] != "success"
        || parts[0].len() < 23
        || !parts[0].contains("[info] ccreq:")
    {
        return None;
    }
    let request_id = parts[0]
        .split("ccreq:")
        .nth(1)?
        .split_whitespace()
        .next()?
        .to_owned();
    let model = parts[2].trim().to_owned();
    let timestamp = NaiveDateTime::parse_from_str(&parts[0][..23], "%Y-%m-%d %H:%M:%S%.3f").ok()?;
    let occurred_at = Local
        .from_local_datetime(&timestamp)
        .single()?
        .with_timezone(&Utc);
    Some((request_id, model, occurred_at))
}

fn normalize_model(model: &str) -> String {
    if model.to_ascii_lowercase().starts_with("mai-code-1-flash") {
        "MAI-Code-1-Flash".into()
    } else {
        model.into()
    }
}

fn ingest_database<S: UsageStore>(path: &PathBuf, store: &mut S) -> Result<IngestStats> {
    let source_db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open Copilot session store {}", path.display()))?;
    let has_usage_table: bool = source_db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='assistant_usage_events')",
        [],
        |row| row.get(0),
    )?;
    if !has_usage_table {
        return Ok(IngestStats {
            files_scanned: 1,
            ..Default::default()
        });
    }
    let mut statement = source_db.prepare("SELECT id,session_id,turn_index,model,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,reasoning_tokens,total_nano_aiu,request_multiplier,initiator,created_at FROM assistant_usage_events ORDER BY id")?;
    let mut rows = statement.query([])?;
    let mut stats = IngestStats {
        files_scanned: 1,
        ..Default::default()
    };
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let session_id: String = row.get(1)?;
        let model: String = row.get(3)?;
        let input: i64 = row.get::<_, Option<i64>>(4)?.unwrap_or_default();
        let output: i64 = row.get::<_, Option<i64>>(5)?.unwrap_or_default();
        let cache_read: i64 = row.get::<_, Option<i64>>(6)?.unwrap_or_default();
        let cache_write: i64 = row.get::<_, Option<i64>>(7)?.unwrap_or_default();
        let reasoning: i64 = row.get::<_, Option<i64>>(8)?.unwrap_or_default();
        let ai_units: i64 = row.get::<_, Option<i64>>(9)?.unwrap_or_default();
        let multiplier: f64 = row.get::<_, Option<f64>>(10)?.unwrap_or_default();
        let initiator = row
            .get::<_, Option<String>>(11)?
            .unwrap_or_else(|| "CLI".into());
        let created_at: String = row.get(12)?;
        let occurred_at = parse_timestamp(&created_at).unwrap_or_else(Utc::now);
        let total = input + output + cache_read + cache_write + reasoning;
        let ai_credits = ai_units as f64 / 1_000_000_000.0 * multiplier;
        let event_id = stable(&format!("copilot:assistant_usage_event:{id}"));
        let payload = json!({
            "source": path,
            "assistant_usage_event_id": id,
            "ai_units_nano": ai_units,
            "request_multiplier": multiplier,
        });
        let event = UsageEvent {
            event_id: event_id.clone(),
            occurred_at,
            provider_id: "copilot".into(),
            agent_name: "copilot".into(),
            session_id: Some(session_id),
            model: Some(model),
            client: Some(classify_client(path, &initiator)),
            input_tokens: input,
            output_tokens: output,
            reasoning_tokens: reasoning,
            cache_read_tokens: cache_read,
            cache_write_tokens: cache_write,
            total_tokens: total,
            ai_units_nano: ai_units,
            request_multiplier: multiplier,
            ai_credits,
            requests: 1,
            dedup_key: event_id,
            raw_event_id: stable(&format!("raw:copilot:assistant_usage_event:{id}")),
            ..Default::default()
        };
        store.append_raw_event(&RawEvent {
            event_id: event.raw_event_id.clone(),
            source_system: "copilot".into(),
            source_channel: "session_store".into(),
            occurred_at,
            payload,
            payload_hash: stable(&format!("copilot-payload:assistant_usage_event:{id}")),
        })?;
        store.append_usage_event(&event)?;
        stats.token_records += 1;
    }
    stats.files_with_usage = usize::from(stats.token_records > 0);
    Ok(stats)
}

fn classify_client(path: &Path, initiator: &str) -> String {
    let path_text = path.to_string_lossy().to_ascii_lowercase();
    if path_text.contains("github.copilot-chat") || path_text.contains("visual studio code") {
        "IDE".into()
    } else if path_text.contains(".copilot") || initiator.to_ascii_lowercase().contains("cli") {
        "CLI".into()
    } else {
        initiator.to_owned()
    }
}

fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .or_else(|_| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").map(|value| value.and_utc())
        })
        .ok()
}

fn stable(value: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(value.as_bytes());
    hex::encode(hash.finalize())
}

#[cfg(test)]
mod tests {
    use chrono::{Local, NaiveDateTime, TimeZone, Utc};

    use super::{normalize_model, parse_vscode_chat_request, parse_vscode_request};

    #[test]
    fn parses_vscode_success_log_with_model() {
        let line = "2026-07-19 15:01:31.522 [info] ccreq:603573d1.copilotmd | success | mai-code-1-flash-secondary | 3280ms | [panel/editAgent]";
        let (request_id, model, occurred_at) = parse_vscode_request(line).expect("request");
        assert_eq!(request_id, "603573d1.copilotmd");
        assert_eq!(normalize_model(&model), "MAI-Code-1-Flash");
        let local_time =
            NaiveDateTime::parse_from_str("2026-07-19 15:01:31.522", "%Y-%m-%d %H:%M:%S%.3f")
                .unwrap();
        let expected = Local
            .from_local_datetime(&local_time)
            .single()
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(occurred_at, expected);
    }

    #[test]
    fn ignores_failed_vscode_request() {
        let line = "2026-07-19 15:01:31.522 [info] ccreq:failed.copilotmd | error | mai-code-1-flash | 100ms | [panel/editAgent]";
        assert!(parse_vscode_request(line).is_none());
    }

    #[test]
    fn parses_vscode_chat_session_credits() {
        let line = r#"{"kind":2,"k":["requests"],"v":[{"requestId":"request-1","timestamp":1784487686171,"result":{"metadata":{"resolvedModelName":"MAI-Code-1-Flash"}},"copilotCredits":1.62339,"promptTokens":31832,"completionTokens":11,"details":"MAI-Code-1-Flash • 1.6 credits"}]}"#;
        let (_, model, _, credits, input, output, _) =
            parse_vscode_chat_request(line).expect("chat request");
        assert_eq!(model, "MAI-Code-1-Flash");
        assert_eq!(credits, 1.62339);
        assert_eq!((input, output), (31832, 11));
    }
}
