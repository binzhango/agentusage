//! Canonical persistence schema for both supported storage backends.
//!
//! SQLite stores JSON and timestamps as TEXT, while PostgreSQL uses JSONB and
//! timestamptz. The table and column names are intentionally identical so the
//! ingestion and reporting layers have one portable data model.

pub const SQLITE: &str = r#"
CREATE TABLE IF NOT EXISTS agentusage_ingest_records (
    record_id TEXT PRIMARY KEY,
    source_path TEXT NOT NULL,
    line_number INTEGER NOT NULL,
    occurred_at TEXT,
    provider_id TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    session_id TEXT,
    event_type TEXT NOT NULL,
    payload_type TEXT,
    model TEXT,
    client TEXT,
    project TEXT,
    tool_name TEXT,
    payload TEXT NOT NULL,
    dedup_key TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS agentusage_usage_raw_events (
    event_id TEXT PRIMARY KEY,
    source_system TEXT NOT NULL,
    source_channel TEXT NOT NULL,
    occurred_at TEXT NOT NULL,
    payload TEXT NOT NULL,
    payload_hash TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS agentusage_usage_events (
    event_id TEXT PRIMARY KEY,
    occurred_at TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    account_id TEXT,
    session_id TEXT,
    model TEXT,
    client TEXT,
    project TEXT,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd REAL NOT NULL DEFAULT 0,
    ai_units_nano INTEGER NOT NULL DEFAULT 0,
    request_multiplier REAL NOT NULL DEFAULT 0,
    ai_credits REAL NOT NULL DEFAULT 0,
    requests INTEGER NOT NULL DEFAULT 0,
    prompts INTEGER NOT NULL DEFAULT 0,
    lines_added INTEGER NOT NULL DEFAULT 0,
    lines_removed INTEGER NOT NULL DEFAULT 0,
    dedup_key TEXT NOT NULL UNIQUE,
    raw_event_id TEXT NOT NULL REFERENCES agentusage_usage_raw_events(event_id)
);
CREATE TABLE IF NOT EXISTS agentusage_usage_metrics (
    metric_id TEXT PRIMARY KEY,
    occurred_at TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    session_id TEXT,
    dimension TEXT NOT NULL,
    name TEXT NOT NULL,
    dedup_key TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS agentusage_ingest_cursors (
    path TEXT PRIMARY KEY,
    byte_offset INTEGER NOT NULL,
    file_size INTEGER NOT NULL,
    last_event_hash TEXT,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS agentusage_ingest_records_lookup
    ON agentusage_ingest_records(occurred_at, agent_name, event_type);
CREATE INDEX IF NOT EXISTS agentusage_usage_events_occurred_at
    ON agentusage_usage_events(occurred_at);
CREATE INDEX IF NOT EXISTS agentusage_usage_events_agent_time
    ON agentusage_usage_events(agent_name, occurred_at);
CREATE INDEX IF NOT EXISTS agentusage_usage_events_model_client_time
    ON agentusage_usage_events(model, client, occurred_at);
CREATE INDEX IF NOT EXISTS agentusage_usage_metrics_lookup
    ON agentusage_usage_metrics(occurred_at, agent_name, dimension);
CREATE INDEX IF NOT EXISTS agentusage_usage_raw_events_assistant_usage_id
    ON agentusage_usage_raw_events(json_extract(payload, '$.assistant_usage_event_id'));
"#;

pub const POSTGRES: &str = r#"
CREATE TABLE IF NOT EXISTS agentusage_ingest_records (
    record_id TEXT PRIMARY KEY,
    source_path TEXT NOT NULL,
    line_number BIGINT NOT NULL,
    occurred_at TIMESTAMPTZ,
    provider_id TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    session_id TEXT,
    event_type TEXT NOT NULL,
    payload_type TEXT,
    model TEXT,
    client TEXT,
    project TEXT,
    tool_name TEXT,
    payload JSONB NOT NULL,
    dedup_key TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS agentusage_usage_raw_events (
    event_id TEXT PRIMARY KEY,
    source_system TEXT NOT NULL,
    source_channel TEXT NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL,
    payload JSONB NOT NULL,
    payload_hash TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS agentusage_usage_events (
    event_id TEXT PRIMARY KEY,
    occurred_at TIMESTAMPTZ NOT NULL,
    provider_id TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    account_id TEXT,
    session_id TEXT,
    model TEXT,
    client TEXT,
    project TEXT,
    input_tokens BIGINT NOT NULL DEFAULT 0,
    output_tokens BIGINT NOT NULL DEFAULT 0,
    reasoning_tokens BIGINT NOT NULL DEFAULT 0,
    cache_read_tokens BIGINT NOT NULL DEFAULT 0,
    cache_write_tokens BIGINT NOT NULL DEFAULT 0,
    total_tokens BIGINT NOT NULL DEFAULT 0,
    cost_usd DOUBLE PRECISION NOT NULL DEFAULT 0,
    ai_units_nano BIGINT NOT NULL DEFAULT 0,
    request_multiplier DOUBLE PRECISION NOT NULL DEFAULT 0,
    ai_credits DOUBLE PRECISION NOT NULL DEFAULT 0,
    requests BIGINT NOT NULL DEFAULT 0,
    prompts BIGINT NOT NULL DEFAULT 0,
    lines_added BIGINT NOT NULL DEFAULT 0,
    lines_removed BIGINT NOT NULL DEFAULT 0,
    dedup_key TEXT NOT NULL UNIQUE,
    raw_event_id TEXT NOT NULL REFERENCES agentusage_usage_raw_events(event_id)
);
CREATE TABLE IF NOT EXISTS agentusage_usage_metrics (
    metric_id TEXT PRIMARY KEY,
    occurred_at TIMESTAMPTZ NOT NULL,
    provider_id TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    session_id TEXT,
    dimension TEXT NOT NULL,
    name TEXT NOT NULL,
    dedup_key TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS agentusage_ingest_cursors (
    path TEXT PRIMARY KEY,
    byte_offset BIGINT NOT NULL,
    file_size BIGINT NOT NULL,
    last_event_hash TEXT,
    updated_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX IF NOT EXISTS agentusage_ingest_records_lookup
    ON agentusage_ingest_records(occurred_at, agent_name, event_type);
CREATE INDEX IF NOT EXISTS agentusage_usage_events_occurred_at
    ON agentusage_usage_events(occurred_at);
CREATE INDEX IF NOT EXISTS agentusage_usage_events_agent_time
    ON agentusage_usage_events(agent_name, occurred_at);
CREATE INDEX IF NOT EXISTS agentusage_usage_events_model_client_time
    ON agentusage_usage_events(model, client, occurred_at);
CREATE INDEX IF NOT EXISTS agentusage_usage_metrics_lookup
    ON agentusage_usage_metrics(occurred_at, agent_name, dimension);
CREATE INDEX IF NOT EXISTS agentusage_usage_raw_events_assistant_usage_id
    ON agentusage_usage_raw_events((payload->>'assistant_usage_event_id'));
"#;
