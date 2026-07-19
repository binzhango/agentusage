use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

use super::{FileCursor, RawEvent, UsageEvent, UsageStore, UsageSummary, add_event};

pub struct SqliteStore {
    connection: Connection,
}

impl SqliteStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let store = Self {
            connection: Connection::open(path)?,
        };
        store.init()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let store = Self {
            connection: Connection::open_in_memory()?,
        };
        store.init()?;
        Ok(store)
    }

    fn init(&self) -> Result<()> {
        self.connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS agentusage_usage_raw_events (event_id TEXT PRIMARY KEY, source_system TEXT NOT NULL, source_channel TEXT NOT NULL, occurred_at TEXT NOT NULL, payload TEXT NOT NULL, payload_hash TEXT NOT NULL); CREATE TABLE IF NOT EXISTS agentusage_usage_events (event_id TEXT PRIMARY KEY, occurred_at TEXT NOT NULL, provider_id TEXT NOT NULL, agent_name TEXT NOT NULL, account_id TEXT, session_id TEXT, model TEXT, client TEXT, input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL, reasoning_tokens INTEGER NOT NULL, cache_read_tokens INTEGER NOT NULL, cache_write_tokens INTEGER NOT NULL, total_tokens INTEGER NOT NULL, cost_usd REAL NOT NULL, ai_units_nano INTEGER NOT NULL DEFAULT 0, request_multiplier REAL NOT NULL DEFAULT 0, ai_credits REAL NOT NULL DEFAULT 0, requests INTEGER NOT NULL, prompts INTEGER NOT NULL, lines_added INTEGER NOT NULL, lines_removed INTEGER NOT NULL, dedup_key TEXT NOT NULL UNIQUE, raw_event_id TEXT NOT NULL REFERENCES agentusage_usage_raw_events(event_id)); CREATE INDEX IF NOT EXISTS agentusage_usage_events_occurred_at ON agentusage_usage_events(occurred_at); CREATE TABLE IF NOT EXISTS agentusage_ingest_cursors (path TEXT PRIMARY KEY, byte_offset INTEGER NOT NULL, file_size INTEGER NOT NULL, last_event_hash TEXT, updated_at TEXT NOT NULL);")?;
        for statement in [
            "ALTER TABLE agentusage_usage_events ADD COLUMN ai_units_nano INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE agentusage_usage_events ADD COLUMN request_multiplier REAL NOT NULL DEFAULT 0",
            "ALTER TABLE agentusage_usage_events ADD COLUMN ai_credits REAL NOT NULL DEFAULT 0",
        ] {
            let _ = self.connection.execute(statement, []);
        }
        Ok(())
    }
}

impl UsageStore for SqliteStore {
    fn append_raw_event(&mut self, event: &RawEvent) -> Result<bool> {
        let inserted = self.connection.execute("INSERT OR IGNORE INTO agentusage_usage_raw_events (event_id,source_system,source_channel,occurred_at,payload,payload_hash) VALUES (?1,?2,?3,?4,?5,?6)", params![event.event_id, event.source_system, event.source_channel, event.occurred_at.to_rfc3339(), serde_json::to_string(&event.payload)?, event.payload_hash])?;
        Ok(inserted > 0)
    }

    fn append_usage_event(&mut self, event: &UsageEvent) -> Result<bool> {
        let inserted = self.connection.execute("INSERT OR IGNORE INTO agentusage_usage_events (event_id,occurred_at,provider_id,agent_name,account_id,session_id,model,client,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cache_write_tokens,total_tokens,cost_usd,ai_units_nano,request_multiplier,ai_credits,requests,prompts,lines_added,lines_removed,dedup_key,raw_event_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24)", params![event.event_id, event.occurred_at.to_rfc3339(), event.provider_id, event.agent_name, event.account_id, event.session_id, event.model, event.client, event.input_tokens, event.output_tokens, event.reasoning_tokens, event.cache_read_tokens, event.cache_write_tokens, event.total_tokens, event.cost_usd, event.ai_units_nano, event.request_multiplier, event.ai_credits, event.requests, event.prompts, event.lines_added, event.lines_removed, event.dedup_key, event.raw_event_id])?;
        Ok(inserted > 0)
    }

    fn cursor(&mut self, path: &str) -> Result<Option<FileCursor>> {
        Ok(self.connection.query_row("SELECT path,byte_offset,file_size,last_event_hash,updated_at FROM agentusage_ingest_cursors WHERE path=?1", [path], |row| Ok(FileCursor { path: row.get(0)?, byte_offset: row.get(1)?, file_size: row.get(2)?, last_event_hash: row.get(3)?, updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(4)?).map(|v| v.with_timezone(&Utc)).map_err(|_| rusqlite::Error::InvalidQuery)? })).optional()?)
    }

    fn save_cursor(&mut self, cursor: &FileCursor) -> Result<()> {
        self.connection.execute("INSERT INTO agentusage_ingest_cursors (path,byte_offset,file_size,last_event_hash,updated_at) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(path) DO UPDATE SET byte_offset=excluded.byte_offset,file_size=excluded.file_size,last_event_hash=excluded.last_event_hash,updated_at=excluded.updated_at", params![cursor.path, cursor.byte_offset, cursor.file_size, cursor.last_event_hash, cursor.updated_at.to_rfc3339()])?;
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
        let mut statement = self.connection.prepare("SELECT e.event_id,e.occurred_at,e.provider_id,e.agent_name,e.account_id,e.session_id,e.model,e.client,e.input_tokens,e.output_tokens,e.reasoning_tokens,e.cache_read_tokens,e.cache_write_tokens,e.total_tokens,e.cost_usd,e.ai_units_nano,e.request_multiplier,e.ai_credits,e.requests,e.prompts,e.lines_added,e.lines_removed,e.dedup_key,e.raw_event_id FROM agentusage_usage_events e JOIN agentusage_usage_raw_events raw ON raw.event_id = e.raw_event_id WHERE e.occurred_at >= ?1 AND e.occurred_at < ?2 AND (?3 IS NULL OR e.agent_name = ?3) AND NOT (e.client = 'IDE' AND e.total_tokens = 0 AND e.ai_credits = 0 AND EXISTS (SELECT 1 FROM agentusage_usage_events richer WHERE richer.model = e.model AND richer.client = 'IDE' AND richer.ai_credits > 0 AND richer.occurred_at >= ?1 AND richer.occurred_at < ?2)) AND NOT EXISTS (SELECT 1 FROM agentusage_usage_events duplicate JOIN agentusage_usage_raw_events duplicate_raw ON duplicate_raw.event_id = duplicate.raw_event_id WHERE json_extract(duplicate_raw.payload, '$.assistant_usage_event_id') IS NOT NULL AND json_extract(duplicate_raw.payload, '$.assistant_usage_event_id') = json_extract(raw.payload, '$.assistant_usage_event_id') AND duplicate.event_id < e.event_id)")?;
        let rows = statement.query_map(
            params![from.to_rfc3339(), to.to_rfc3339(), agent_name],
            |row| {
                Ok(UsageEvent {
                    event_id: row.get(0)?,
                    occurred_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(1)?)
                        .map(|v| v.with_timezone(&Utc))
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    provider_id: row.get(2)?,
                    agent_name: row.get(3)?,
                    account_id: row.get(4)?,
                    session_id: row.get(5)?,
                    model: row.get(6)?,
                    client: row.get(7)?,
                    input_tokens: row.get(8)?,
                    output_tokens: row.get(9)?,
                    reasoning_tokens: row.get(10)?,
                    cache_read_tokens: row.get(11)?,
                    cache_write_tokens: row.get(12)?,
                    total_tokens: row.get(13)?,
                    cost_usd: row.get(14)?,
                    ai_units_nano: row.get(15)?,
                    request_multiplier: row.get(16)?,
                    ai_credits: row.get(17)?,
                    requests: row.get(18)?,
                    prompts: row.get(19)?,
                    lines_added: row.get(20)?,
                    lines_removed: row.get(21)?,
                    dedup_key: row.get(22)?,
                    raw_event_id: row.get(23)?,
                })
            },
        )?;
        for row in rows {
            add_event(&mut summary, &row?);
        }
        summary.sessions = self.connection.query_row("SELECT COUNT(DISTINCT session_id) FROM agentusage_usage_events WHERE occurred_at >= ?1 AND occurred_at < ?2 AND (?3 IS NULL OR agent_name = ?3)", params![from.to_rfc3339(), to.to_rfc3339(), agent_name], |row| row.get(0))?;
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn persists_events_cursors_and_backend_neutral_summary() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let occurred_at = Utc.with_ymd_and_hms(2026, 7, 19, 12, 0, 0).unwrap();
        let raw = RawEvent {
            event_id: "raw-1".into(),
            source_system: "codex".into(),
            source_channel: "jsonl".into(),
            occurred_at,
            payload: serde_json::json!({"type":"token_count"}),
            payload_hash: "hash-1".into(),
        };
        assert!(store.append_raw_event(&raw).unwrap());
        let event = UsageEvent {
            event_id: "event-1".into(),
            occurred_at,
            provider_id: "codex".into(),
            agent_name: "codex".into(),
            session_id: Some("session-1".into()),
            model: Some("gpt-5".into()),
            client: Some("CLI".into()),
            input_tokens: 10,
            output_tokens: 4,
            reasoning_tokens: 2,
            cache_read_tokens: 6,
            total_tokens: 16,
            cost_usd: 0.5,
            requests: 1,
            prompts: 1,
            dedup_key: "dedup-1".into(),
            raw_event_id: "raw-1".into(),
            ..Default::default()
        };
        assert!(store.append_usage_event(&event).unwrap());
        assert!(!store.append_usage_event(&event).unwrap());
        let cursor = FileCursor {
            path: "sessions/a.jsonl".into(),
            byte_offset: 42,
            file_size: 100,
            updated_at: occurred_at,
            ..Default::default()
        };
        store.save_cursor(&cursor).unwrap();
        assert_eq!(store.cursor(&cursor.path).unwrap().unwrap().byte_offset, 42);
        let summary = store
            .summary(
                occurred_at - chrono::Duration::minutes(1),
                occurred_at + chrono::Duration::minutes(1),
            )
            .unwrap();
        assert_eq!(summary.total_tokens, 16);
        assert_eq!(summary.sessions, 1);
        assert_eq!(summary.cache_hit_rate(), Some(37.5));
        assert_eq!(summary.models["gpt-5"].total_tokens, 16);
    }
}
