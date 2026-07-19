use super::{FileCursor, RawEvent, UsageEvent, UsageStore, UsageSummary, add_event};
use anyhow::Result;
use chrono::{DateTime, Utc};
use postgres::{Client, NoTls};

pub struct PostgresStore {
    client: Client,
}

impl PostgresStore {
    pub fn connect(url: &str) -> Result<Self> {
        let mut store = Self {
            client: Client::connect(url, NoTls)?,
        };
        store.init()?;
        Ok(store)
    }

    fn init(&mut self) -> Result<()> {
        self.client.batch_execute("CREATE TABLE IF NOT EXISTS agentusage_usage_raw_events (event_id TEXT PRIMARY KEY, source_system TEXT NOT NULL, source_channel TEXT NOT NULL, occurred_at TIMESTAMPTZ NOT NULL, payload JSONB NOT NULL, payload_hash TEXT NOT NULL); CREATE TABLE IF NOT EXISTS agentusage_usage_events (event_id TEXT PRIMARY KEY, occurred_at TIMESTAMPTZ NOT NULL, provider_id TEXT NOT NULL, agent_name TEXT NOT NULL, account_id TEXT, session_id TEXT, model TEXT, client TEXT, input_tokens BIGINT NOT NULL, output_tokens BIGINT NOT NULL, reasoning_tokens BIGINT NOT NULL, cache_read_tokens BIGINT NOT NULL, cache_write_tokens BIGINT NOT NULL, total_tokens BIGINT NOT NULL, cost_usd DOUBLE PRECISION NOT NULL, ai_units_nano BIGINT NOT NULL DEFAULT 0, request_multiplier DOUBLE PRECISION NOT NULL DEFAULT 0, ai_credits DOUBLE PRECISION NOT NULL DEFAULT 0, requests BIGINT NOT NULL, prompts BIGINT NOT NULL, lines_added BIGINT NOT NULL, lines_removed BIGINT NOT NULL, dedup_key TEXT NOT NULL UNIQUE, raw_event_id TEXT NOT NULL REFERENCES agentusage_usage_raw_events(event_id)); ALTER TABLE agentusage_usage_events ADD COLUMN IF NOT EXISTS ai_units_nano BIGINT NOT NULL DEFAULT 0; ALTER TABLE agentusage_usage_events ADD COLUMN IF NOT EXISTS request_multiplier DOUBLE PRECISION NOT NULL DEFAULT 0; ALTER TABLE agentusage_usage_events ADD COLUMN IF NOT EXISTS ai_credits DOUBLE PRECISION NOT NULL DEFAULT 0; CREATE INDEX IF NOT EXISTS agentusage_usage_events_occurred_at ON agentusage_usage_events(occurred_at); CREATE TABLE IF NOT EXISTS agentusage_ingest_cursors (path TEXT PRIMARY KEY, byte_offset BIGINT NOT NULL, file_size BIGINT NOT NULL, last_event_hash TEXT, updated_at TIMESTAMPTZ NOT NULL);")?;
        Ok(())
    }
}

impl UsageStore for PostgresStore {
    fn append_raw_event(&mut self, event: &RawEvent) -> Result<bool> {
        let n = self.client.execute(
            "INSERT INTO agentusage_usage_raw_events VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING",
            &[
                &event.event_id,
                &event.source_system,
                &event.source_channel,
                &event.occurred_at,
                &event.payload,
                &event.payload_hash,
            ],
        )?;
        Ok(n > 0)
    }
    fn append_usage_event(&mut self, event: &UsageEvent) -> Result<bool> {
        let n = self.client.execute("INSERT INTO agentusage_usage_events (event_id,occurred_at,provider_id,agent_name,account_id,session_id,model,client,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cache_write_tokens,total_tokens,cost_usd,ai_units_nano,request_multiplier,ai_credits,requests,prompts,lines_added,lines_removed,dedup_key,raw_event_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24) ON CONFLICT (dedup_key) DO NOTHING", &[&event.event_id, &event.occurred_at, &event.provider_id, &event.agent_name, &event.account_id, &event.session_id, &event.model, &event.client, &event.input_tokens, &event.output_tokens, &event.reasoning_tokens, &event.cache_read_tokens, &event.cache_write_tokens, &event.total_tokens, &event.cost_usd, &event.ai_units_nano, &event.request_multiplier, &event.ai_credits, &event.requests, &event.prompts, &event.lines_added, &event.lines_removed, &event.dedup_key, &event.raw_event_id])?;
        Ok(n > 0)
    }
    fn cursor(&mut self, path: &str) -> Result<Option<FileCursor>> {
        Ok(self.client.query_opt("SELECT path,byte_offset,file_size,last_event_hash,updated_at FROM agentusage_ingest_cursors WHERE path=$1", &[&path])?.map(|row| FileCursor { path: row.get(0), byte_offset: row.get(1), file_size: row.get(2), last_event_hash: row.get(3), updated_at: row.get(4) }))
    }
    fn save_cursor(&mut self, cursor: &FileCursor) -> Result<()> {
        self.client.execute("INSERT INTO agentusage_ingest_cursors VALUES ($1,$2,$3,$4,$5) ON CONFLICT(path) DO UPDATE SET byte_offset=EXCLUDED.byte_offset,file_size=EXCLUDED.file_size,last_event_hash=EXCLUDED.last_event_hash,updated_at=EXCLUDED.updated_at", &[&cursor.path, &cursor.byte_offset, &cursor.file_size, &cursor.last_event_hash, &cursor.updated_at])?;
        Ok(())
    }
    fn summary(&mut self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<UsageSummary> {
        self.summary_for_agent(None, from, to)
    }

    fn summary_for_agent(
        &mut self,
        agent_name: Option<&str>,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<UsageSummary> {
        let mut summary = UsageSummary {
            from,
            to,
            ..Default::default()
        };
        let rows = self.client.query("SELECT e.event_id,e.occurred_at,e.provider_id,e.agent_name,e.account_id,e.session_id,e.model,e.client,e.input_tokens,e.output_tokens,e.reasoning_tokens,e.cache_read_tokens,e.cache_write_tokens,e.total_tokens,e.cost_usd,e.ai_units_nano,e.request_multiplier,e.ai_credits,e.requests,e.prompts,e.lines_added,e.lines_removed,e.dedup_key,e.raw_event_id FROM agentusage_usage_events e JOIN agentusage_usage_raw_events raw ON raw.event_id = e.raw_event_id WHERE e.occurred_at >= $1 AND e.occurred_at < $2 AND ($3::text IS NULL OR e.agent_name = $3) AND NOT (e.client = 'IDE' AND e.total_tokens = 0 AND e.ai_credits = 0 AND EXISTS (SELECT 1 FROM agentusage_usage_events richer WHERE richer.model = e.model AND richer.client = 'IDE' AND richer.ai_credits > 0 AND richer.occurred_at >= $1 AND richer.occurred_at < $2)) AND NOT EXISTS (SELECT 1 FROM agentusage_usage_events duplicate JOIN agentusage_usage_raw_events duplicate_raw ON duplicate_raw.event_id = duplicate.raw_event_id WHERE duplicate_raw.payload->>'assistant_usage_event_id' IS NOT NULL AND duplicate_raw.payload->>'assistant_usage_event_id' = raw.payload->>'assistant_usage_event_id' AND duplicate.event_id < e.event_id)", &[&from, &to, &agent_name])?;
        for row in rows {
            add_event(
                &mut summary,
                &UsageEvent {
                    event_id: row.get(0),
                    occurred_at: row.get(1),
                    provider_id: row.get(2),
                    agent_name: row.get(3),
                    account_id: row.get(4),
                    session_id: row.get(5),
                    model: row.get(6),
                    client: row.get(7),
                    input_tokens: row.get(8),
                    output_tokens: row.get(9),
                    reasoning_tokens: row.get(10),
                    cache_read_tokens: row.get(11),
                    cache_write_tokens: row.get(12),
                    total_tokens: row.get(13),
                    cost_usd: row.get(14),
                    ai_units_nano: row.get(15),
                    request_multiplier: row.get(16),
                    ai_credits: row.get(17),
                    requests: row.get(18),
                    prompts: row.get(19),
                    lines_added: row.get(20),
                    lines_removed: row.get(21),
                    dedup_key: row.get(22),
                    raw_event_id: row.get(23),
                },
            );
        }
        summary.sessions = self.client.query_one("SELECT COUNT(DISTINCT session_id) FROM agentusage_usage_events WHERE occurred_at >= $1 AND occurred_at < $2 AND ($3::text IS NULL OR agent_name = $3)", &[&from, &to, &agent_name])?.get(0);
        Ok(summary)
    }
}
