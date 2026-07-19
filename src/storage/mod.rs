use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env,
    io::{self, IsTerminal},
};

pub mod postgres;
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
pub struct UsageEvent {
    pub event_id: String,
    pub occurred_at: DateTime<Utc>,
    pub provider_id: String,
    pub agent_name: String,
    pub account_id: Option<String>,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub client: Option<String>,
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
    pub clients: BTreeMap<String, UsageBucket>,
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

impl UsageSummary {
    pub fn cache_hit_rate(&self) -> Option<f64> {
        let denominator = self.input_tokens + self.cache_read_tokens + self.cache_write_tokens;
        (denominator > 0 && self.cache_read_tokens > 0)
            .then(|| self.cache_read_tokens as f64 / denominator as f64 * 100.0)
    }
}

pub trait UsageStore {
    fn append_raw_event(&mut self, event: &RawEvent) -> Result<bool>;
    fn append_usage_event(&mut self, event: &UsageEvent) -> Result<bool>;
    fn cursor(&mut self, path: &str) -> Result<Option<FileCursor>>;
    fn save_cursor(&mut self, cursor: &FileCursor) -> Result<()>;
    fn summary(&mut self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<UsageSummary>;
    fn summary_for_agent(
        &mut self,
        agent_name: Option<&str>,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<UsageSummary>;
}

pub enum Backend {
    Sqlite(sqlite::SqliteStore),
    Postgres(postgres::PostgresStore),
}

impl Backend {
    pub fn open(mode: BackendMode) -> Result<Self> {
        Self::open_for_agent(mode, "codex")
    }

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
            BackendMode::Jsonl => anyhow::bail!("JSONL fallback has no storage backend"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendMode {
    Sqlite,
    Postgres,
    Jsonl,
}

pub fn prepare_backend(interactive: bool) -> Result<BackendMode> {
    prepare_backend_for_agent(interactive, "codex")
}

pub fn prepare_backend_for_agent(interactive: bool, agent: &str) -> Result<BackendMode> {
    let sqlite_path = crate::config::agent_db_path(agent)?;
    if sqlite_path.exists() && sqlite::SqliteStore::open(&sqlite_path).is_ok() {
        eprintln!(
            "[agentusage] storage backend=sqlite path={}",
            sqlite_path.display()
        );
        return Ok(BackendMode::Sqlite);
    }
    let postgres_url = env::var("AGENTUSAGE_POSTGRES_URL")
        .ok()
        .filter(|value| !value.trim().is_empty());
    if let Some(url) = postgres_url.as_deref()
        && postgres::PostgresStore::connect(url).is_ok()
    {
        eprintln!("[agentusage] storage backend=postgres status=connected");
        return Ok(BackendMode::Postgres);
    }
    if !interactive || !io::stdin().is_terminal() {
        eprintln!("[agentusage] storage backend=jsonl reason=no_initialized_database");
        return Ok(BackendMode::Jsonl);
    }
    println!("No initialized usage storage backend was found.");
    println!("Choose the preferred backend:");
    println!("[s] Initialize SQLite at {}", sqlite_path.display());
    if postgres_url.is_some() {
        println!("[p] Initialize PostgreSQL from AGENTUSAGE_POSTGRES_URL");
    }
    println!("[n] Use JSONL fallback without initializing a database");
    println!("Enter your choice [s/p/n]:");
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    match answer.trim().to_ascii_lowercase().as_str() {
        "s" | "sqlite" => {
            sqlite::SqliteStore::open(&sqlite_path)?;
            eprintln!(
                "[agentusage] storage backend=sqlite initialized path={}",
                sqlite_path.display()
            );
            Ok(BackendMode::Sqlite)
        }
        "p" | "postgres" if postgres_url.is_some() => {
            postgres::PostgresStore::connect(postgres_url.as_deref().unwrap())?;
            eprintln!("[agentusage] storage backend=postgres initialized");
            Ok(BackendMode::Postgres)
        }
        _ => {
            println!("Using JSONL fallback; no storage backend was initialized.");
            eprintln!("[agentusage] storage backend=jsonl reason=user_selected_fallback");
            Ok(BackendMode::Jsonl)
        }
    }
}

impl UsageStore for Backend {
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
    fn summary(&mut self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<UsageSummary> {
        match self {
            Self::Sqlite(store) => store.summary(from, to),
            Self::Postgres(store) => store.summary(from, to),
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
    if let Some(client) = &event.client {
        add_bucket(summary.clients.entry(client.clone()).or_default(), &bucket);
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
