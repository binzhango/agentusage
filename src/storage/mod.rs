use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env,
    io::{self, IsTerminal},
};

pub mod postgres;
pub mod schema;
pub mod sqlite;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvent {
    pub event_id: String,
    pub source_system: String,
    pub source_channel: String,
    pub occurred_at: DateTime<Utc>,
    pub payload: serde_json::Value,
    pub payload_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageMetric {
    pub metric_id: String,
    pub occurred_at: DateTime<Utc>,
    pub provider_id: String,
    pub agent_name: String,
    pub session_id: Option<String>,
    pub dimension: String,
    pub name: String,
    pub dedup_key: String,
}

/// One imported provider event. The source JSONL is retained here for audit,
/// while the common columns let the dashboard query without reopening files.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IngestRecord {
    pub record_id: String,
    pub source_path: String,
    pub line_number: i64,
    pub occurred_at: Option<DateTime<Utc>>,
    pub provider_id: String,
    pub agent_name: String,
    pub session_id: Option<String>,
    pub event_type: String,
    pub payload_type: Option<String>,
    pub model: Option<String>,
    pub client: Option<String>,
    pub project: Option<String>,
    pub tool_name: Option<String>,
    pub payload: serde_json::Value,
    pub dedup_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageEvent {
    pub event_id: String,
    pub occurred_at: DateTime<Utc>,
    pub provider_id: String,
    pub agent_name: String,
    pub account_id: Option<String>,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub client: Option<String>,
    pub project: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: f64,
    pub ai_units_nano: i64,
    pub request_multiplier: f64,
    pub ai_credits: f64,
    pub requests: i64,
    pub prompts: i64,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub dedup_key: String,
    pub raw_event_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileCursor {
    pub path: String,
    pub byte_offset: i64,
    pub file_size: i64,
    pub last_event_hash: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageSummary {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub sessions: i64,
    pub requests: i64,
    pub prompts: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: f64,
    pub ai_units_nano: i64,
    pub request_multiplier: f64,
    pub ai_credits: f64,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub models: BTreeMap<String, UsageBucket>,
    pub providers: BTreeMap<String, UsageBucket>,
    pub clients: BTreeMap<String, UsageBucket>,
    pub projects: BTreeMap<String, UsageBucket>,
    pub tools: BTreeMap<String, i64>,
    pub languages: BTreeMap<String, i64>,
    pub primary_used_percent: Option<f64>,
    pub primary_window_minutes: Option<i64>,
    pub primary_resets_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DailyUsagePoint {
    pub date: NaiveDate,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub total_tokens: i64,
    pub models: BTreeMap<String, i64>,
}

#[derive(Debug, Clone)]
pub struct UsageEventQuery {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub before: Option<UsageEventCursor>,
    pub limit: usize,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PromptQuery {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub before: Option<UsageEventCursor>,
    pub limit: usize,
    pub session_id: Option<String>,
    pub search: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UsageEventCursor {
    pub occurred_at: DateTime<Utc>,
    pub event_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageEventDetail {
    #[serde(flatten)]
    pub usage: UsageEvent,
    pub source_system: String,
    pub source_channel: String,
    pub source_request_id: Option<String>,
    pub status: String,
    pub duration_ms: Option<i64>,
    pub source_locator: Option<String>,
    pub total_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PromptDetail {
    #[serde(flatten)]
    pub usage: UsageEvent,
    pub text: String,
    pub source_system: String,
    pub source_channel: String,
    pub source_locator: Option<String>,
}

pub fn prompt_detail(
    usage: UsageEvent,
    source_system: String,
    source_channel: String,
    payload: serde_json::Value,
) -> Option<PromptDetail> {
    let text = prompt_text(&usage.agent_name, &payload)?;
    let event = usage_event_detail(
        usage,
        source_system.clone(),
        source_channel.clone(),
        payload,
        false,
    );
    Some(PromptDetail {
        usage: event.usage,
        text,
        source_system,
        source_channel,
        source_locator: event.source_locator,
    })
}

fn prompt_text(agent_name: &str, payload: &serde_json::Value) -> Option<String> {
    let candidate = match agent_name {
        "codex" => {
            let body = payload
                .get("payload")
                .or_else(|| payload.get("data"))
                .unwrap_or(payload);
            ["message", "text", "content", "prompt"]
                .into_iter()
                .find_map(|key| body.get(key))
        }
        "claude_code" | "pi" => payload
            .get("message")
            .and_then(|message| message.get("content")),
        "opencode" => payload
            .get("parts")
            .or_else(|| payload.pointer("/properties/part"))
            .or_else(|| payload.pointer("/payload/part")),
        _ => None,
    }?;
    content_text(candidate)
}

fn content_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => non_empty_text(text),
        serde_json::Value::Array(values) => {
            let text = values
                .iter()
                .filter_map(content_text)
                .collect::<Vec<_>>()
                .join("\n");
            non_empty_text(&text)
        }
        serde_json::Value::Object(object) => {
            let content_type = object.get("type").and_then(serde_json::Value::as_str);
            if content_type.is_some_and(|kind| !matches!(kind, "text" | "input_text")) {
                return None;
            }
            object
                .get("text")
                .and_then(serde_json::Value::as_str)
                .and_then(non_empty_text)
                .or_else(|| object.get("content").and_then(content_text))
        }
        _ => None,
    }
}

fn non_empty_text(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

pub fn usage_event_detail(
    usage: UsageEvent,
    source_system: String,
    source_channel: String,
    payload: serde_json::Value,
    include_raw: bool,
) -> UsageEventDetail {
    fn find_value<'a>(
        value: &'a serde_json::Value,
        keys: &[&str],
    ) -> Option<&'a serde_json::Value> {
        match value {
            serde_json::Value::Object(object) => {
                for key in keys {
                    if let Some(found) = object.get(*key) {
                        return Some(found);
                    }
                }
                object.values().find_map(|child| find_value(child, keys))
            }
            serde_json::Value::Array(values) => {
                values.iter().find_map(|child| find_value(child, keys))
            }
            _ => None,
        }
    }
    let source_request_id = find_value(
        &payload,
        &["request_id", "requestId", "turn_id", "turnId", "message_id"],
    )
    .and_then(serde_json::Value::as_str)
    .map(str::to_owned);
    let status = find_value(&payload, &["status", "stop_reason", "stopReason"])
        .and_then(serde_json::Value::as_str)
        .unwrap_or("ok")
        .to_owned();
    let duration_ms = find_value(
        &payload,
        &["duration_ms", "durationMs", "latency_ms", "latencyMs"],
    )
    .and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_f64().map(|value| value as i64))
    });
    let source_path = find_value(&payload, &["source_path", "sourcePath", "source"])
        .and_then(serde_json::Value::as_str);
    let source_line = find_value(&payload, &["line_number", "lineNumber", "line"])
        .and_then(serde_json::Value::as_i64);
    let source_locator = source_path.map(|path| match source_line {
        Some(line) => format!("{path}:{line}"),
        None => path.to_owned(),
    });
    let has_reported_total = find_value(
        &payload,
        &["total_tokens", "totalTokens", "total_token_usage"],
    )
    .is_some();
    let total_source = if has_reported_total {
        "provider_reported"
    } else {
        "computed_provider_policy"
    }
    .to_owned();
    UsageEventDetail {
        usage,
        source_system,
        source_channel,
        source_request_id,
        status,
        duration_ms,
        source_locator,
        total_source,
        raw_payload: include_raw.then_some(payload),
    }
}

/// Extract quota from one provider payload. The caller is responsible for
/// selecting the latest raw event before calling this function.
pub fn quota_from_payload(value: &serde_json::Value) -> Option<(f64, Option<i64>, Option<i64>)> {
    fn walk(value: &serde_json::Value) -> Option<(f64, Option<i64>, Option<i64>)> {
        if let serde_json::Value::Object(object) = value {
            let number = |keys: &[&str]| {
                keys.iter().find_map(|key| {
                    object.get(*key).and_then(|value| {
                        value
                            .as_f64()
                            .or_else(|| value.as_i64().map(|value| value as f64))
                            .or_else(|| value.as_str()?.parse::<f64>().ok())
                    })
                })
            };
            if let Some(used) =
                number(&["used_percent", "usedPercent", "percent_used", "percentUsed"])
            {
                return Some((
                    used,
                    number(&["window_minutes", "windowMinutes", "window"])
                        .map(|value| value as i64),
                    number(&["resets_at", "resetsAt", "reset_at", "resetAt"])
                        .map(|value| value as i64),
                ));
            }
            for child in object.values() {
                if let Some(result) = walk(child) {
                    return Some(result);
                }
            }
        } else if let serde_json::Value::Array(values) = value {
            for child in values {
                if let Some(result) = walk(child) {
                    return Some(result);
                }
            }
        }
        None
    }
    walk(value)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageBucket {
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: f64,
    pub ai_units_nano: i64,
    pub request_multiplier: f64,
    pub ai_credits: f64,
}

pub trait UsageStore {
    fn begin_batch(&mut self) -> Result<()> {
        Ok(())
    }
    fn end_batch(&mut self) -> Result<()> {
        Ok(())
    }
    fn append_record(&mut self, record: &IngestRecord) -> Result<bool> {
        let _ = record;
        Ok(false)
    }
    fn append_raw_event(&mut self, event: &RawEvent) -> Result<bool>;
    fn append_usage_event(&mut self, event: &UsageEvent) -> Result<bool>;
    fn upsert_raw_event(&mut self, event: &RawEvent) -> Result<bool> {
        self.append_raw_event(event)
    }
    fn upsert_usage_event(&mut self, event: &UsageEvent) -> Result<bool> {
        self.append_usage_event(event)
    }
    fn append_metric(&mut self, metric: &UsageMetric) -> Result<bool> {
        let _ = metric;
        Ok(false)
    }
    fn cursor(&mut self, path: &str) -> Result<Option<FileCursor>>;
    fn save_cursor(&mut self, cursor: &FileCursor) -> Result<()>;
    fn summary_for_agent(
        &mut self,
        agent_name: Option<&str>,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<UsageSummary>;
    fn usage_events(
        &mut self,
        agent_name: &str,
        query: &UsageEventQuery,
    ) -> Result<Vec<UsageEventDetail>>;
    fn usage_event(&mut self, event_id: &str) -> Result<Option<UsageEventDetail>>;
    fn prompts(&mut self, agent_name: &str, query: &PromptQuery) -> Result<Vec<PromptDetail>>;
    fn prompt(&mut self, event_id: &str) -> Result<Option<PromptDetail>> {
        Ok(self.usage_event(event_id)?.and_then(|event| {
            let payload = event.raw_payload?;
            prompt_detail(
                event.usage,
                event.source_system,
                event.source_channel,
                payload,
            )
        }))
    }
}

pub enum Backend {
    Sqlite(sqlite::SqliteStore),
    Postgres(postgres::PostgresStore),
}

impl Backend {
    pub fn open_for_agent(mode: BackendMode, agent: &str) -> Result<Self> {
        match mode {
            BackendMode::Sqlite => Ok(Self::Sqlite(sqlite::SqliteStore::open(
                &crate::config::agent_db_path(agent)?,
            )?)),
            BackendMode::Postgres => {
                let url = env::var("AGENTUSAGE_POSTGRES_URL")
                    .map_err(|_| anyhow::anyhow!("AGENTUSAGE_POSTGRES_URL is not set"))?;
                Ok(Self::Postgres(postgres::PostgresStore::connect(&url)?))
            }
        }
    }

    pub fn open_read_only_for_agent(mode: BackendMode, agent: &str) -> Result<Self> {
        match mode {
            BackendMode::Sqlite => Ok(Self::Sqlite(sqlite::SqliteStore::open_read_only(
                &crate::config::agent_db_path(agent)?,
            )?)),
            BackendMode::Postgres => {
                let url = env::var("AGENTUSAGE_POSTGRES_URL")
                    .map_err(|_| anyhow::anyhow!("AGENTUSAGE_POSTGRES_URL is not set"))?;
                Ok(Self::Postgres(postgres::PostgresStore::connect_read_only(
                    &url,
                )?))
            }
        }
    }

    pub fn agent_summary(
        &mut self,
        agent_name: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<UsageSummary> {
        match self {
            Self::Sqlite(store) => store.summary_for_agent(Some(agent_name), from, to),
            Self::Postgres(store) => store.summary_for_agent(Some(agent_name), from, to),
        }
    }

    pub fn daily_trend_for_agent(
        &mut self,
        agent_name: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<DailyUsagePoint>> {
        match self {
            Self::Sqlite(store) => store.daily_trend_for_agent(agent_name, from, to),
            Self::Postgres(store) => store.daily_trend_for_agent(agent_name, from, to),
        }
    }

    pub fn usage_events(
        &mut self,
        agent_name: &str,
        query: &UsageEventQuery,
    ) -> Result<Vec<UsageEventDetail>> {
        match self {
            Self::Sqlite(store) => store.usage_events(agent_name, query),
            Self::Postgres(store) => store.usage_events(agent_name, query),
        }
    }

    pub fn usage_event(&mut self, event_id: &str) -> Result<Option<UsageEventDetail>> {
        match self {
            Self::Sqlite(store) => store.usage_event(event_id),
            Self::Postgres(store) => store.usage_event(event_id),
        }
    }

    pub fn prompts(&mut self, agent_name: &str, query: &PromptQuery) -> Result<Vec<PromptDetail>> {
        match self {
            Self::Sqlite(store) => store.prompts(agent_name, query),
            Self::Postgres(store) => store.prompts(agent_name, query),
        }
    }

    pub fn prompt(&mut self, event_id: &str) -> Result<Option<PromptDetail>> {
        match self {
            Self::Sqlite(store) => store.prompt(event_id),
            Self::Postgres(store) => store.prompt(event_id),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendMode {
    Sqlite,
    Postgres,
}

pub fn prepare_backend_for_agent(interactive: bool, agent: &str) -> Result<BackendMode> {
    let sqlite_path = crate::config::agent_db_path(agent)?;
    let sqlite_problem = if sqlite_path.exists() {
        match sqlite::SqliteStore::open_read_only(&sqlite_path) {
            Ok(_) => return Ok(BackendMode::Sqlite),
            Err(error) => Some(error.to_string()),
        }
    } else {
        None
    };
    let postgres_url = env::var("AGENTUSAGE_POSTGRES_URL")
        .ok()
        .filter(|value| !value.trim().is_empty());
    if let Some(url) = postgres_url.as_deref()
        && postgres::PostgresStore::connect_read_only(url).is_ok()
    {
        return Ok(BackendMode::Postgres);
    }
    if !interactive || !io::stdin().is_terminal() {
        if let Some(problem) = sqlite_problem {
            anyhow::bail!(
                "SQLite usage storage at {} is unavailable: {problem}; run `agentusage sync {agent}` in a terminal to rebuild it",
                sqlite_path.display()
            );
        }
        anyhow::bail!(
            "no initialized SQLite or PostgreSQL usage storage found; run `agentusage sync {agent}` after selecting a database backend"
        );
    }
    println!("No initialized usage storage backend was found.");
    println!("Choose the preferred backend:");
    if let Some(problem) = sqlite_problem.as_deref() {
        println!("[s] Rebuild derived SQLite at {}", sqlite_path.display());
        println!("    Existing database: {problem}");
    } else {
        println!("[s] Initialize SQLite at {}", sqlite_path.display());
    }
    if postgres_url.is_some() {
        println!("[p] Initialize PostgreSQL from AGENTUSAGE_POSTGRES_URL");
    }
    if postgres_url.is_some() {
        println!("Enter your choice [s/p]:");
    } else {
        println!("Enter your choice [s]:");
    }
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    match answer.trim().to_ascii_lowercase().as_str() {
        "s" | "sqlite" => {
            if sqlite_problem.is_some() {
                sqlite::SqliteStore::rebuild(&sqlite_path)?;
                eprintln!("Storage rebuilt · provider={agent} · backend=SQLite");
            } else {
                sqlite::SqliteStore::open(&sqlite_path)?;
                eprintln!("Storage initialized · provider={agent} · backend=SQLite");
            }
            Ok(BackendMode::Sqlite)
        }
        "p" | "postgres" if postgres_url.is_some() => {
            postgres::PostgresStore::connect(postgres_url.as_deref().unwrap())?;
            eprintln!("Storage initialized · provider={agent} · backend=PostgreSQL");
            Ok(BackendMode::Postgres)
        }
        _ => anyhow::bail!("no storage backend selected; choose SQLite or PostgreSQL"),
    }
}

impl UsageStore for Backend {
    fn begin_batch(&mut self) -> Result<()> {
        match self {
            Self::Sqlite(store) => store.begin_batch(),
            Self::Postgres(store) => store.begin_batch(),
        }
    }

    fn end_batch(&mut self) -> Result<()> {
        match self {
            Self::Sqlite(store) => store.end_batch(),
            Self::Postgres(store) => store.end_batch(),
        }
    }

    fn append_record(&mut self, record: &IngestRecord) -> Result<bool> {
        match self {
            Self::Sqlite(store) => store.append_record(record),
            Self::Postgres(store) => store.append_record(record),
        }
    }

    fn append_raw_event(&mut self, event: &RawEvent) -> Result<bool> {
        match self {
            Self::Sqlite(store) => store.append_raw_event(event),
            Self::Postgres(store) => store.append_raw_event(event),
        }
    }
    fn append_usage_event(&mut self, event: &UsageEvent) -> Result<bool> {
        match self {
            Self::Sqlite(store) => store.append_usage_event(event),
            Self::Postgres(store) => store.append_usage_event(event),
        }
    }

    fn upsert_raw_event(&mut self, event: &RawEvent) -> Result<bool> {
        match self {
            Self::Sqlite(store) => store.upsert_raw_event(event),
            Self::Postgres(store) => store.upsert_raw_event(event),
        }
    }

    fn upsert_usage_event(&mut self, event: &UsageEvent) -> Result<bool> {
        match self {
            Self::Sqlite(store) => store.upsert_usage_event(event),
            Self::Postgres(store) => store.upsert_usage_event(event),
        }
    }

    fn append_metric(&mut self, metric: &UsageMetric) -> Result<bool> {
        match self {
            Self::Sqlite(store) => store.append_metric(metric),
            Self::Postgres(store) => store.append_metric(metric),
        }
    }
    fn cursor(&mut self, path: &str) -> Result<Option<FileCursor>> {
        match self {
            Self::Sqlite(store) => store.cursor(path),
            Self::Postgres(store) => store.cursor(path),
        }
    }
    fn save_cursor(&mut self, cursor: &FileCursor) -> Result<()> {
        match self {
            Self::Sqlite(store) => store.save_cursor(cursor),
            Self::Postgres(store) => store.save_cursor(cursor),
        }
    }
    fn summary_for_agent(
        &mut self,
        agent_name: Option<&str>,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<UsageSummary> {
        match self {
            Self::Sqlite(store) => store.summary_for_agent(agent_name, from, to),
            Self::Postgres(store) => store.summary_for_agent(agent_name, from, to),
        }
    }

    fn usage_events(
        &mut self,
        agent_name: &str,
        query: &UsageEventQuery,
    ) -> Result<Vec<UsageEventDetail>> {
        self.usage_events(agent_name, query)
    }

    fn usage_event(&mut self, event_id: &str) -> Result<Option<UsageEventDetail>> {
        self.usage_event(event_id)
    }

    fn prompts(&mut self, agent_name: &str, query: &PromptQuery) -> Result<Vec<PromptDetail>> {
        self.prompts(agent_name, query)
    }

    fn prompt(&mut self, event_id: &str) -> Result<Option<PromptDetail>> {
        self.prompt(event_id)
    }
}

pub fn add_event(summary: &mut UsageSummary, event: &UsageEvent) {
    summary.requests += event.requests;
    summary.prompts += event.prompts;
    summary.input_tokens += event.input_tokens;
    summary.output_tokens += event.output_tokens;
    summary.reasoning_tokens += event.reasoning_tokens;
    summary.cache_read_tokens += event.cache_read_tokens;
    summary.cache_write_tokens += event.cache_write_tokens;
    summary.total_tokens += event.total_tokens;
    summary.cost_usd += event.cost_usd;
    summary.ai_units_nano += event.ai_units_nano;
    summary.request_multiplier += event.request_multiplier;
    summary.ai_credits += event.ai_credits;
    summary.lines_added += event.lines_added;
    summary.lines_removed += event.lines_removed;
    let bucket = UsageBucket {
        requests: event.requests,
        input_tokens: event.input_tokens,
        output_tokens: event.output_tokens,
        reasoning_tokens: event.reasoning_tokens,
        cache_read_tokens: event.cache_read_tokens,
        cache_write_tokens: event.cache_write_tokens,
        total_tokens: event.total_tokens,
        cost_usd: event.cost_usd,
        ai_units_nano: event.ai_units_nano,
        request_multiplier: event.request_multiplier,
        ai_credits: event.ai_credits,
    };
    if let Some(model) = &event.model {
        add_bucket(summary.models.entry(model.clone()).or_default(), &bucket);
    }
    if !event.provider_id.is_empty() {
        add_bucket(
            summary
                .providers
                .entry(event.provider_id.clone())
                .or_default(),
            &bucket,
        );
    }
    if let Some(client) = &event.client {
        add_bucket(summary.clients.entry(client.clone()).or_default(), &bucket);
    }
    if let Some(project) = &event.project {
        add_bucket(
            summary.projects.entry(project.clone()).or_default(),
            &bucket,
        );
    }
}

fn add_bucket(target: &mut UsageBucket, value: &UsageBucket) {
    target.requests += value.requests;
    target.input_tokens += value.input_tokens;
    target.output_tokens += value.output_tokens;
    target.reasoning_tokens += value.reasoning_tokens;
    target.cache_read_tokens += value.cache_read_tokens;
    target.cache_write_tokens += value.cache_write_tokens;
    target.total_tokens += value.total_tokens;
    target.cost_usd += value.cost_usd;
    target.ai_units_nano += value.ai_units_nano;
    target.request_multiplier += value.request_multiplier;
    target.ai_credits += value.ai_credits;
}

#[cfg(test)]
mod tests {
    use super::{prompt_text, quota_from_payload};

    #[test]
    fn extracts_quota_from_latest_codex_payload_shape() {
        let payload = serde_json::json!({
            "payload": {
                "rate_limits": {
                    "primary": {
                        "used_percent": 26.0,
                        "window_minutes": 10080,
                        "resets_at": 1785091968
                    }
                }
            }
        });
        assert_eq!(
            quota_from_payload(&payload),
            Some((26.0, Some(10080), Some(1785091968)))
        );
    }

    #[test]
    fn extracts_only_user_text_from_supported_prompt_shapes() {
        assert_eq!(
            prompt_text(
                "codex",
                &serde_json::json!({"payload":{"type":"user_message","message":"Fix the parser"}}),
            )
            .as_deref(),
            Some("Fix the parser")
        );
        assert_eq!(
            prompt_text(
                "claude_code",
                &serde_json::json!({"message":{"content":[{"type":"text","text":"Add tests"},{"type":"tool_result","content":"ignored"}]}}),
            )
            .as_deref(),
            Some("Add tests")
        );
        assert_eq!(
            prompt_text(
                "opencode",
                &serde_json::json!({"parts":[{"type":"text","text":"Review this"},{"type":"file","text":"ignored"}]}),
            )
            .as_deref(),
            Some("Review this")
        );
        assert_eq!(
            prompt_text(
                "pi",
                &serde_json::json!({"message":{"content":[{"type":"text","text":"Explain it"}]}}),
            )
            .as_deref(),
            Some("Explain it")
        );
    }
}
