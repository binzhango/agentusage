use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::path::Path;

use super::{
    FileCursor, IngestRecord, RawEvent, UsageBucket, UsageEvent, UsageMetric, UsageStore,
    UsageSummary, add_event,
};

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

    pub fn open_read_only(path: &Path) -> Result<Self> {
        Ok(Self {
            connection: Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?,
        })
    }

    pub fn quick_summary_for_agent(
        &mut self,
        agent_name: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<UsageSummary> {
        let mut summary = UsageSummary {
            from,
            to,
            ..Default::default()
        };
        let filter = "occurred_at >= ?1 AND occurred_at < ?2 AND agent_name = ?3";
        let totals = self.connection.query_row(
            &format!("SELECT COUNT(DISTINCT session_id), COALESCE(SUM(requests),0), COALESCE(SUM(prompts),0), COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(reasoning_tokens),0), COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_write_tokens),0), COALESCE(SUM(total_tokens),0), COALESCE(SUM(cost_usd),0), COALESCE(SUM(ai_units_nano),0), COALESCE(SUM(request_multiplier),0), COALESCE(SUM(ai_credits),0), COALESCE(SUM(lines_added),0), COALESCE(SUM(lines_removed),0) FROM agentusage_usage_events WHERE {filter}"),
            params![from.to_rfc3339(), to.to_rfc3339(), agent_name],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?, row.get(10)?, row.get(11)?, row.get(12)?, row.get(13)?, row.get(14)?)),
        )?;
        summary.sessions = totals.0;
        summary.requests = totals.1;
        summary.prompts = totals.2;
        summary.input_tokens = totals.3;
        summary.output_tokens = totals.4;
        summary.reasoning_tokens = totals.5;
        summary.cache_read_tokens = totals.6;
        summary.cache_write_tokens = totals.7;
        summary.total_tokens = totals.8;
        summary.cost_usd = totals.9;
        summary.ai_units_nano = totals.10;
        summary.request_multiplier = totals.11;
        summary.ai_credits = totals.12;
        summary.lines_added = totals.13;
        summary.lines_removed = totals.14;

        for dimension in ["model", "client", "project"] {
            let (dimension_expr, from_sql, filter_sql) = if dimension == "project" {
                (
                    "COALESCE(e.project, json_extract(raw.payload, '$.payload.cwd'), json_extract(raw.payload, '$.cwd'))",
                    "agentusage_usage_events e LEFT JOIN agentusage_usage_raw_events raw ON raw.event_id = e.raw_event_id",
                    "e.occurred_at >= ?1 AND e.occurred_at < ?2 AND e.agent_name = ?3",
                )
            } else {
                (dimension, "agentusage_usage_events", filter)
            };
            let sql = format!(
                "SELECT {dimension_expr}, COALESCE(SUM(requests),0), COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(reasoning_tokens),0), COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_write_tokens),0), COALESCE(SUM(total_tokens),0), COALESCE(SUM(cost_usd),0), COALESCE(SUM(ai_units_nano),0), COALESCE(SUM(request_multiplier),0), COALESCE(SUM(ai_credits),0) FROM {from_sql} WHERE {filter_sql} AND {dimension_expr} IS NOT NULL AND {dimension_expr} != '' GROUP BY {dimension_expr}"
            );
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(
                params![from.to_rfc3339(), to.to_rfc3339(), agent_name],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        UsageBucket {
                            requests: row.get(1)?,
                            input_tokens: row.get(2)?,
                            output_tokens: row.get(3)?,
                            reasoning_tokens: row.get(4)?,
                            cache_read_tokens: row.get(5)?,
                            cache_write_tokens: row.get(6)?,
                            total_tokens: row.get(7)?,
                            cost_usd: row.get(8)?,
                            ai_units_nano: row.get(9)?,
                            request_multiplier: row.get(10)?,
                            ai_credits: row.get(11)?,
                        },
                    ))
                },
            )?;
            for row in rows {
                let (name, bucket) = row?;
                match dimension {
                    "model" => {
                        summary.models.insert(name, bucket);
                    }
                    "client" => {
                        summary.clients.insert(name, bucket);
                    }
                    _ => {
                        summary.projects.insert(name, bucket);
                    }
                }
            }
        }
        let mut metrics = self.connection.prepare("SELECT dimension,name,COUNT(*) FROM agentusage_usage_metrics WHERE occurred_at >= ?1 AND occurred_at < ?2 AND agent_name = ?3 GROUP BY dimension,name")?;
        let rows = metrics.query_map(
            params![from.to_rfc3339(), to.to_rfc3339(), agent_name],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get(2)?,
                ))
            },
        )?;
        for row in rows {
            let (dimension, name, count) = row?;
            match dimension.as_str() {
                "tool" => {
                    summary.tools.insert(name, count);
                }
                "language_v2" => {
                    summary.languages.insert(name, count);
                }
                _ => {}
            }
        }
        Ok(summary)
    }

    fn init(&self) -> Result<()> {
        self.connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS agentusage_ingest_records (record_id TEXT PRIMARY KEY, source_path TEXT NOT NULL, line_number INTEGER NOT NULL, occurred_at TEXT, provider_id TEXT NOT NULL, agent_name TEXT NOT NULL, session_id TEXT, event_type TEXT NOT NULL, payload_type TEXT, model TEXT, client TEXT, project TEXT, tool_name TEXT, payload TEXT NOT NULL, dedup_key TEXT NOT NULL UNIQUE); CREATE TABLE IF NOT EXISTS agentusage_usage_raw_events (event_id TEXT PRIMARY KEY, source_system TEXT NOT NULL, source_channel TEXT NOT NULL, occurred_at TEXT NOT NULL, payload TEXT NOT NULL, payload_hash TEXT NOT NULL); CREATE TABLE IF NOT EXISTS agentusage_usage_events (event_id TEXT PRIMARY KEY, occurred_at TEXT NOT NULL, provider_id TEXT NOT NULL, agent_name TEXT NOT NULL, account_id TEXT, session_id TEXT, model TEXT, client TEXT, project TEXT, input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL, reasoning_tokens INTEGER NOT NULL, cache_read_tokens INTEGER NOT NULL, cache_write_tokens INTEGER NOT NULL, total_tokens INTEGER NOT NULL, cost_usd REAL NOT NULL, ai_units_nano INTEGER NOT NULL DEFAULT 0, request_multiplier REAL NOT NULL DEFAULT 0, ai_credits REAL NOT NULL DEFAULT 0, requests INTEGER NOT NULL, prompts INTEGER NOT NULL, lines_added INTEGER NOT NULL, lines_removed INTEGER NOT NULL, dedup_key TEXT NOT NULL UNIQUE, raw_event_id TEXT NOT NULL REFERENCES agentusage_usage_raw_events(event_id)); CREATE TABLE IF NOT EXISTS agentusage_usage_metrics (metric_id TEXT PRIMARY KEY, occurred_at TEXT NOT NULL, provider_id TEXT NOT NULL, agent_name TEXT NOT NULL, session_id TEXT, dimension TEXT NOT NULL, name TEXT NOT NULL, dedup_key TEXT NOT NULL UNIQUE); CREATE INDEX IF NOT EXISTS agentusage_ingest_records_lookup ON agentusage_ingest_records(occurred_at,agent_name,event_type); CREATE INDEX IF NOT EXISTS agentusage_usage_events_occurred_at ON agentusage_usage_events(occurred_at); CREATE INDEX IF NOT EXISTS agentusage_usage_metrics_lookup ON agentusage_usage_metrics(occurred_at,agent_name,dimension); CREATE TABLE IF NOT EXISTS agentusage_ingest_cursors (path TEXT PRIMARY KEY, byte_offset INTEGER NOT NULL, file_size INTEGER NOT NULL, last_event_hash TEXT, updated_at TEXT NOT NULL);")?;
        for statement in [
            "ALTER TABLE agentusage_usage_events ADD COLUMN project TEXT",
            "ALTER TABLE agentusage_usage_events ADD COLUMN ai_units_nano INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE agentusage_usage_events ADD COLUMN request_multiplier REAL NOT NULL DEFAULT 0",
            "ALTER TABLE agentusage_usage_events ADD COLUMN ai_credits REAL NOT NULL DEFAULT 0",
        ] {
            let _ = self.connection.execute(statement, []);
        }
        self.connection.execute("CREATE INDEX IF NOT EXISTS agentusage_usage_events_project ON agentusage_usage_events(project)", [])?;
        Ok(())
    }
}

impl UsageStore for SqliteStore {
    fn append_record(&mut self, record: &IngestRecord) -> Result<bool> {
        let inserted = self.connection.execute(
            "INSERT OR IGNORE INTO agentusage_ingest_records (record_id,source_path,line_number,occurred_at,provider_id,agent_name,session_id,event_type,payload_type,model,client,project,tool_name,payload,dedup_key) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            params![
                record.record_id,
                record.source_path,
                record.line_number,
                record.occurred_at.map(|value| value.to_rfc3339()),
                record.provider_id,
                record.agent_name,
                record.session_id,
                record.event_type,
                record.payload_type,
                record.model,
                record.client,
                record.project,
                record.tool_name,
                serde_json::to_string(&record.payload)?,
                record.dedup_key
            ],
        )?;
        Ok(inserted > 0)
    }

    fn append_raw_event(&mut self, event: &RawEvent) -> Result<bool> {
        let inserted = self.connection.execute("INSERT OR IGNORE INTO agentusage_usage_raw_events (event_id,source_system,source_channel,occurred_at,payload,payload_hash) VALUES (?1,?2,?3,?4,?5,?6)", params![event.event_id, event.source_system, event.source_channel, event.occurred_at.to_rfc3339(), serde_json::to_string(&event.payload)?, event.payload_hash])?;
        Ok(inserted > 0)
    }

    fn append_usage_event(&mut self, event: &UsageEvent) -> Result<bool> {
        let inserted = self.connection.execute("INSERT OR IGNORE INTO agentusage_usage_events (event_id,occurred_at,provider_id,agent_name,account_id,session_id,model,client,project,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cache_write_tokens,total_tokens,cost_usd,ai_units_nano,request_multiplier,ai_credits,requests,prompts,lines_added,lines_removed,dedup_key,raw_event_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25)", params![event.event_id, event.occurred_at.to_rfc3339(), event.provider_id, event.agent_name, event.account_id, event.session_id, event.model, event.client, event.project, event.input_tokens, event.output_tokens, event.reasoning_tokens, event.cache_read_tokens, event.cache_write_tokens, event.total_tokens, event.cost_usd, event.ai_units_nano, event.request_multiplier, event.ai_credits, event.requests, event.prompts, event.lines_added, event.lines_removed, event.dedup_key, event.raw_event_id])?;
        if inserted == 0 && event.project.is_some() {
            self.connection.execute("UPDATE agentusage_usage_events SET project=?1 WHERE dedup_key=?2 AND (project IS NULL OR project='unknown')", params![event.project, event.dedup_key])?;
        }
        Ok(inserted > 0)
    }

    fn append_metric(&mut self, metric: &UsageMetric) -> Result<bool> {
        let inserted = self.connection.execute(
            "INSERT OR IGNORE INTO agentusage_usage_metrics (metric_id,occurred_at,provider_id,agent_name,session_id,dimension,name,dedup_key) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                metric.metric_id,
                metric.occurred_at.to_rfc3339(),
                metric.provider_id,
                metric.agent_name,
                metric.session_id,
                metric.dimension,
                metric.name,
                metric.dedup_key
            ],
        )?;
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
        let mut statement = self.connection.prepare("SELECT e.event_id,e.occurred_at,e.provider_id,e.agent_name,e.account_id,e.session_id,e.model,e.client,e.project,e.input_tokens,e.output_tokens,e.reasoning_tokens,e.cache_read_tokens,e.cache_write_tokens,e.total_tokens,e.cost_usd,e.ai_units_nano,e.request_multiplier,e.ai_credits,e.requests,e.prompts,e.lines_added,e.lines_removed,e.dedup_key,e.raw_event_id FROM agentusage_usage_events e JOIN agentusage_usage_raw_events raw ON raw.event_id = e.raw_event_id WHERE e.occurred_at >= ?1 AND e.occurred_at < ?2 AND (?3 IS NULL OR e.agent_name = ?3) AND NOT (e.client = 'IDE' AND e.total_tokens = 0 AND e.ai_credits = 0 AND EXISTS (SELECT 1 FROM agentusage_usage_events richer WHERE richer.model = e.model AND richer.client = 'IDE' AND richer.ai_credits > 0 AND richer.occurred_at >= ?1 AND richer.occurred_at < ?2)) AND NOT EXISTS (SELECT 1 FROM agentusage_usage_events duplicate JOIN agentusage_usage_raw_events duplicate_raw ON duplicate_raw.event_id = duplicate.raw_event_id WHERE json_extract(duplicate_raw.payload, '$.assistant_usage_event_id') IS NOT NULL AND json_extract(duplicate_raw.payload, '$.assistant_usage_event_id') = json_extract(raw.payload, '$.assistant_usage_event_id') AND duplicate.event_id < e.event_id)")?;
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
                    project: row.get(8)?,
                    input_tokens: row.get(9)?,
                    output_tokens: row.get(10)?,
                    reasoning_tokens: row.get(11)?,
                    cache_read_tokens: row.get(12)?,
                    cache_write_tokens: row.get(13)?,
                    total_tokens: row.get(14)?,
                    cost_usd: row.get(15)?,
                    ai_units_nano: row.get(16)?,
                    request_multiplier: row.get(17)?,
                    ai_credits: row.get(18)?,
                    requests: row.get(19)?,
                    prompts: row.get(20)?,
                    lines_added: row.get(21)?,
                    lines_removed: row.get(22)?,
                    dedup_key: row.get(23)?,
                    raw_event_id: row.get(24)?,
                })
            },
        )?;
        for row in rows {
            add_event(&mut summary, &row?);
        }
        // Older ingested rows may not have a normalized project yet. Recover it
        // from the raw payload so CLI reports remain useful without re-ingestion.
        let mut projects = self.connection.prepare(
            "SELECT COALESCE(e.project, json_extract(raw.payload, '$.payload.cwd'), json_extract(raw.payload, '$.cwd')), COALESCE(SUM(e.requests),0), COALESCE(SUM(e.input_tokens),0), COALESCE(SUM(e.output_tokens),0), COALESCE(SUM(e.reasoning_tokens),0), COALESCE(SUM(e.cache_read_tokens),0), COALESCE(SUM(e.cache_write_tokens),0), COALESCE(SUM(e.total_tokens),0), COALESCE(SUM(e.cost_usd),0), COALESCE(SUM(e.ai_units_nano),0), COALESCE(SUM(e.request_multiplier),0), COALESCE(SUM(e.ai_credits),0) FROM agentusage_usage_events e LEFT JOIN agentusage_usage_raw_events raw ON raw.event_id = e.raw_event_id WHERE e.occurred_at >= ?1 AND e.occurred_at < ?2 AND (?3 IS NULL OR e.agent_name = ?3) AND COALESCE(e.project, json_extract(raw.payload, '$.payload.cwd'), json_extract(raw.payload, '$.cwd')) IS NOT NULL GROUP BY 1",
        )?;
        let project_rows = projects.query_map(
            params![from.to_rfc3339(), to.to_rfc3339(), agent_name],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    UsageBucket {
                        requests: row.get(1)?,
                        input_tokens: row.get(2)?,
                        output_tokens: row.get(3)?,
                        reasoning_tokens: row.get(4)?,
                        cache_read_tokens: row.get(5)?,
                        cache_write_tokens: row.get(6)?,
                        total_tokens: row.get(7)?,
                        cost_usd: row.get(8)?,
                        ai_units_nano: row.get(9)?,
                        request_multiplier: row.get(10)?,
                        ai_credits: row.get(11)?,
                    },
                ))
            },
        )?;
        for row in project_rows {
            let (project, bucket) = row?;
            summary.projects.insert(project, bucket);
        }
        let mut metrics = self.connection.prepare("SELECT dimension,name,COUNT(*) FROM agentusage_usage_metrics WHERE occurred_at >= ?1 AND occurred_at < ?2 AND (?3 IS NULL OR agent_name = ?3) GROUP BY dimension,name")?;
        let rows = metrics.query_map(
            params![from.to_rfc3339(), to.to_rfc3339(), agent_name],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get(2)?,
                ))
            },
        )?;
        for row in rows {
            let (dimension, name, count) = row?;
            match dimension.as_str() {
                "tool" => {
                    summary.tools.insert(name, count);
                }
                "language_v2" => {
                    summary.languages.insert(name, count);
                }
                _ => {}
            }
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
            project: Some("agentusage".into()),
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
        assert_eq!(summary.projects["agentusage"].total_tokens, 16);
    }
}
