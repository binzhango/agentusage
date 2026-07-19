use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

use crate::core::TokenUsage;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventType {
    #[serde(rename = "turn_completed")]
    TurnCompleted,
    #[default]
    #[serde(rename = "message_usage")]
    MessageUsage,
    #[serde(rename = "tool_usage")]
    ToolUsage,
    #[serde(rename = "raw_envelope")]
    RawEnvelope,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventStatus {
    #[default]
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "aborted")]
    Aborted,
    #[serde(rename = "unknown")]
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IngestRequest {
    pub source_system: String,
    pub source_channel: String,
    pub source_schema_version: String,
    pub occurred_at: Option<DateTime<Utc>>,
    pub workspace_id: Option<String>,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub message_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub provider_id: Option<String>,
    pub account_id: Option<String>,
    pub agent_name: Option<String>,
    pub event_type: EventType,
    pub model_raw: Option<String>,
    pub model_canonical: Option<String>,
    pub model_lineage_id: Option<String>,
    #[serde(flatten)]
    pub usage: TokenUsage,
    pub tool_name: Option<String>,
    pub status: EventStatus,
    pub normalization_version: Option<String>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct IngestResult {
    pub status: String,
    pub deduped: bool,
    pub event_id: String,
    pub raw_event_id: String,
}

pub struct Store {
    connection: Connection,
    path: PathBuf,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let connection = Connection::open(path)
            .with_context(|| format!("open telemetry DB {}", path.display()))?;
        let store = Self {
            connection,
            path: path.to_path_buf(),
        };
        store.init()?;
        Ok(store)
    }

    fn init(&self) -> Result<()> {
        self.connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; CREATE TABLE IF NOT EXISTS usage_raw_events (raw_event_id TEXT PRIMARY KEY, ingested_at TEXT NOT NULL, source_system TEXT NOT NULL, source_channel TEXT NOT NULL, source_schema_version TEXT NOT NULL, source_payload TEXT NOT NULL, source_payload_hash TEXT NOT NULL, workspace_id TEXT, agent_session_id TEXT); CREATE INDEX IF NOT EXISTS idx_usage_raw_events_ingested_at ON usage_raw_events(ingested_at); CREATE TABLE IF NOT EXISTS usage_events (event_id TEXT PRIMARY KEY, occurred_at TEXT NOT NULL, provider_id TEXT, agent_name TEXT NOT NULL, account_id TEXT, workspace_id TEXT, session_id TEXT, turn_id TEXT, message_id TEXT, tool_call_id TEXT, event_type TEXT NOT NULL, model_raw TEXT, model_canonical TEXT, model_lineage_id TEXT, input_tokens INTEGER, output_tokens INTEGER, reasoning_tokens INTEGER, cache_read_tokens INTEGER, cache_write_tokens INTEGER, total_tokens INTEGER, cost_usd REAL, requests INTEGER, tool_name TEXT, status TEXT NOT NULL, dedup_key TEXT NOT NULL UNIQUE, raw_event_id TEXT NOT NULL, normalization_version TEXT NOT NULL, FOREIGN KEY(raw_event_id) REFERENCES usage_raw_events(raw_event_id)); CREATE INDEX IF NOT EXISTS idx_usage_events_occurred_at ON usage_events(occurred_at); CREATE INDEX IF NOT EXISTS idx_usage_events_provider_window ON usage_events(provider_id, account_id, occurred_at);")?;
        Ok(())
    }

    pub fn ingest(&self, mut request: IngestRequest) -> Result<IngestResult> {
        let now = Utc::now();
        request.occurred_at.get_or_insert(now);
        request.source_schema_version = if request.source_schema_version.trim().is_empty() {
            "v1".into()
        } else {
            request.source_schema_version.trim().into()
        };
        request.normalization_version.get_or_insert("v1".into());
        request
            .agent_name
            .get_or_insert_with(|| request.source_system.clone());
        request.usage.sum_total_tokens();
        let payload = if request.payload.is_null() {
            serde_json::json!({})
        } else {
            request.payload.clone()
        };
        let payload_text = serde_json::to_string(&payload)?;
        let raw_id = Uuid::new_v4().to_string();
        let event_id = Uuid::new_v4().to_string();
        let dedup_key = dedup_key(&request)?;
        let mut hash = Sha256::new();
        hash.update(payload_text.as_bytes());
        let payload_hash = hex::encode(hash.finalize());
        let tx = self.connection.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO usage_raw_events VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                raw_id,
                now.to_rfc3339(),
                request.source_system,
                request.source_channel,
                request.source_schema_version,
                payload_text,
                payload_hash,
                request.workspace_id,
                request.session_id
            ],
        )?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT event_id FROM usage_events WHERE dedup_key=?1",
                [&dedup_key],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            tx.commit()?;
            return Ok(IngestResult {
                status: "accepted".into(),
                deduped: true,
                event_id: existing,
                raw_event_id: raw_id,
            });
        }
        tx.execute("INSERT INTO usage_events (event_id,occurred_at,provider_id,agent_name,account_id,workspace_id,session_id,turn_id,message_id,tool_call_id,event_type,model_raw,model_canonical,model_lineage_id,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cache_write_tokens,total_tokens,cost_usd,requests,tool_name,status,dedup_key,raw_event_id,normalization_version) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27)", params![event_id, request.occurred_at.unwrap().to_rfc3339(), request.provider_id, request.agent_name.unwrap(), request.account_id, request.workspace_id, request.session_id, request.turn_id, request.message_id, request.tool_call_id, serde_json::to_string(&request.event_type)?.trim_matches('"'), request.model_raw, request.model_canonical, request.model_lineage_id, request.usage.input_tokens, request.usage.output_tokens, request.usage.reasoning_tokens, request.usage.cache_read_tokens, request.usage.cache_write_tokens, request.usage.total_tokens, request.usage.cost_usd, request.usage.requests, request.tool_name, serde_json::to_string(&request.status)?.trim_matches('"'), dedup_key, raw_id, request.normalization_version.unwrap()])?;
        tx.commit()?;
        Ok(IngestResult {
            status: "accepted".into(),
            deduped: false,
            event_id,
            raw_event_id: raw_id,
        })
    }

    pub fn write_spool(&self, request: &IngestRequest) -> Result<IngestResult> {
        let spool_dir = self
            .path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("hook-spool");
        fs::create_dir_all(&spool_dir)?;
        let event_id = Uuid::new_v4().to_string();
        let spool_path = spool_dir.join(format!("{event_id}.json"));
        fs::write(&spool_path, serde_json::to_vec_pretty(request)?)?;
        Ok(IngestResult {
            status: "spooled".into(),
            deduped: false,
            event_id,
            raw_event_id: String::new(),
        })
    }
    pub fn keep_alive(&self) {
        let _ = &self.path;
    }
}

pub fn parse_hook(source: &str, raw: &[u8], account_id: String) -> Result<IngestRequest> {
    let payload: serde_json::Value = serde_json::from_slice(raw).context("parse hook JSON")?;
    let normalized = source.trim().to_ascii_lowercase();
    let provider = match normalized.as_str() {
        "claude" | "claude_code" => "claude_code",
        "codex" => "codex",
        "opencode" | "open_code" => "opencode",
        other => anyhow::bail!(
            "unknown telemetry source {other:?}; expected claude_code, codex, or opencode"
        ),
    };
    let mut request = IngestRequest {
        source_system: provider.into(),
        source_channel: "hook".into(),
        source_schema_version: "v1".into(),
        account_id: (!account_id.trim().is_empty()).then_some(account_id),
        provider_id: Some(provider.into()),
        agent_name: Some(provider.into()),
        event_type: EventType::TurnCompleted,
        payload,
        ..Default::default()
    };
    request.session_id = string_field(&request.payload, &["session_id", "sessionId"]);
    request.turn_id = string_field(&request.payload, &["turn_id", "turnId", "id"]);
    request.model_raw = string_field(&request.payload, &["model", "model_name", "modelName"]);
    request.workspace_id = string_field(&request.payload, &["cwd", "workspace", "workspace_id"]);
    request.occurred_at = string_field(&request.payload, &["timestamp", "time", "occurred_at"])
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(&v).ok())
        .map(|v| v.with_timezone(&Utc));
    request.usage.input_tokens = int_field(
        &request.payload,
        &["input_tokens", "inputTokens", "usage.input_tokens"],
    );
    request.usage.output_tokens = int_field(
        &request.payload,
        &["output_tokens", "outputTokens", "usage.output_tokens"],
    );
    request.usage.reasoning_tokens =
        int_field(&request.payload, &["reasoning_tokens", "reasoningTokens"]);
    request.usage.cache_read_tokens =
        int_field(&request.payload, &["cache_read_tokens", "cacheReadTokens"]);
    request.usage.cache_write_tokens = int_field(
        &request.payload,
        &["cache_write_tokens", "cacheWriteTokens"],
    );
    request.usage.cost_usd =
        float_field(&request.payload, &["cost_usd", "costUSD", "usage.cost_usd"]);
    request.usage.requests = int_field(&request.payload, &["requests"]);
    Ok(request)
}

fn string_field(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .pointer(&format!("/{}", key.replace('.', "/")))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    })
}
fn int_field(value: &serde_json::Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        value
            .pointer(&format!("/{}", key.replace('.', "/")))
            .and_then(|v| v.as_i64())
            .or_else(|| {
                value
                    .pointer(&format!("/{}", key.replace('.', "/")))
                    .and_then(|v| v.as_f64())
                    .map(|v| v as i64)
            })
    })
}
fn float_field(value: &serde_json::Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        value
            .pointer(&format!("/{}", key.replace('.', "/")))
            .and_then(|v| v.as_f64())
    })
}
fn dedup_key(req: &IngestRequest) -> Result<String> {
    let stable = req
        .tool_call_id
        .as_ref()
        .or(req.message_id.as_ref())
        .or(req.turn_id.as_ref())
        .map(|v| format!("id:{v}"));
    let basis = stable.unwrap_or_else(|| serde_json::to_string(req).unwrap_or_default());
    let mut hash = Sha256::new();
    hash.update(format!(
        "{}|{}|{}",
        req.source_system,
        req.event_type_string(),
        basis
    ));
    Ok(hex::encode(hash.finalize()))
}
impl IngestRequest {
    fn event_type_string(&self) -> String {
        serde_json::to_string(&self.event_type).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hook_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("telemetry.db")).unwrap();
        let req = parse_hook(
            "codex",
            br#"{"turn_id":"t1","input_tokens":2,"output_tokens":3}"#,
            String::new(),
        )
        .unwrap();
        let first = store.ingest(req.clone()).unwrap();
        let second = store.ingest(req).unwrap();
        assert!(!first.deduped);
        assert!(second.deduped);
        assert_eq!(first.event_id, second.event_id);
    }
}
