use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::{
    collections::BTreeMap,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use super::{
    DailyUsagePoint, FileCursor, IngestRecord, PromptDetail, PromptQuery, RawEvent, UsageEvent,
    UsageEventDetail, UsageEventQuery, UsageMetric, UsageStore, UsageSummary, add_event,
    prompt_detail, usage_event_detail,
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

    /// Rebuild a provider database whose contents can be derived from source
    /// history. This intentionally replaces obsolete schemas rather than
    /// mutating them in place.
    pub fn rebuild(path: &Path) -> Result<Self> {
        for target in sqlite_files(path) {
            match std::fs::remove_file(&target) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("failed to remove derived SQLite file {}", target.display())
                    });
                }
            }
        }
        Self::open(path)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let store = Self {
            connection: Connection::open_in_memory()?,
        };
        store.init()?;
        Ok(store)
    }

    pub fn open_read_only(path: &Path) -> Result<Self> {
        let store = Self {
            connection: Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?,
        };
        store.validate_schema()?;
        Ok(store)
    }

    pub fn daily_trend_for_agent(
        &mut self,
        agent_name: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<DailyUsagePoint>> {
        let mut statement = self.connection.prepare(
            "SELECT occurred_at, model, input_tokens, output_tokens, cache_read_tokens, total_tokens FROM agentusage_usage_events WHERE occurred_at >= ?1 AND occurred_at < ?2 AND agent_name = ?3 ORDER BY occurred_at, model",
        )?;
        let rows = statement.query_map(
            params![from.to_rfc3339(), to.to_rfc3339(), agent_name],
            |row| {
                Ok((
                    DateTime::parse_from_rfc3339(&row.get::<_, String>(0)?)
                        .map(|value| value.with_timezone(&Local).date_naive())
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )?;
        let mut points = BTreeMap::new();
        for row in rows {
            let (date, model, input_tokens, output_tokens, cache_read_tokens, total_tokens) = row?;
            let point = points.entry(date).or_insert_with(|| DailyUsagePoint {
                date,
                ..Default::default()
            });
            point.input_tokens += input_tokens;
            point.output_tokens += output_tokens;
            point.cache_read_tokens += cache_read_tokens;
            point.total_tokens += total_tokens;
            if let Some(model) = model.filter(|name| !name.is_empty()) {
                *point.models.entry(model).or_default() += total_tokens;
            }
        }
        Ok(points.into_values().collect())
    }

    fn attach_rate_limit(&self, summary: &mut UsageSummary, agent_name: &str) -> Result<()> {
        let latest: Option<String> = self
            .connection
            .query_row(
                "SELECT payload FROM agentusage_usage_raw_events WHERE source_system=?1 ORDER BY occurred_at DESC LIMIT 1",
                params![agent_name],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(payload) = latest
            .and_then(|payload| serde_json::from_str(&payload).ok())
            .and_then(|payload| super::quota_from_payload(&payload))
        {
            let (used, window, resets) = payload;
            summary.primary_used_percent = Some(used);
            summary.primary_window_minutes = window;
            summary.primary_resets_at = resets;
        }
        Ok(())
    }

    fn init(&self) -> Result<()> {
        if self.usage_schema_exists()? {
            self.validate_schema()?;
        }
        self.connection
            .execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")?;
        self.connection.execute_batch(super::schema::SQLITE)?;
        Ok(())
    }

    fn usage_schema_exists(&self) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='agentusage_usage_events')",
            [],
            |row| row.get(0),
        )?)
    }

    fn validate_schema(&self) -> Result<()> {
        if !self.usage_schema_exists()? {
            anyhow::bail!("SQLite usage schema is not initialized");
        }
        let has_version_table: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='agentusage_schema')",
            [],
            |row| row.get(0),
        )?;
        if !has_version_table {
            anyhow::bail!(
                "obsolete SQLite usage schema; rebuild the derived provider database and synchronize again"
            );
        }
        let version = self
            .connection
            .query_row(
                "SELECT version FROM agentusage_schema WHERE singleton=1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if version != Some(super::schema::VERSION) {
            anyhow::bail!(
                "obsolete SQLite usage schema; rebuild the derived provider database and synchronize again"
            );
        }
        let mut statement = self
            .connection
            .prepare("SELECT name FROM pragma_table_info('agentusage_usage_events')")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
        for required in [
            "project",
            "reasoning_tokens",
            "cache_read_tokens",
            "cache_write_tokens",
            "total_tokens",
            "ai_units_nano",
            "request_multiplier",
            "ai_credits",
            "raw_event_id",
        ] {
            if !columns.contains(required) {
                anyhow::bail!(
                    "invalid SQLite usage schema (missing {required}); rebuild the derived provider database and synchronize again"
                );
            }
        }
        Ok(())
    }
}

fn sqlite_files(path: &Path) -> [PathBuf; 3] {
    let sidecar = |suffix: &str| {
        let mut value = path.as_os_str().to_os_string();
        value.push(suffix);
        PathBuf::from(value)
    };
    [path.to_path_buf(), sidecar("-wal"), sidecar("-shm")]
}

impl UsageStore for SqliteStore {
    fn begin_batch(&mut self) -> Result<()> {
        self.connection.execute_batch("BEGIN IMMEDIATE")?;
        Ok(())
    }

    fn end_batch(&mut self) -> Result<()> {
        self.connection.execute_batch("COMMIT")?;
        Ok(())
    }

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

    fn upsert_raw_event(&mut self, event: &RawEvent) -> Result<bool> {
        let changed = self.connection.execute(
            "INSERT INTO agentusage_usage_raw_events (event_id,source_system,source_channel,occurred_at,payload,payload_hash) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(event_id) DO UPDATE SET source_system=excluded.source_system,source_channel=excluded.source_channel,occurred_at=excluded.occurred_at,payload=excluded.payload,payload_hash=excluded.payload_hash",
            params![event.event_id, event.source_system, event.source_channel, event.occurred_at.to_rfc3339(), serde_json::to_string(&event.payload)?, event.payload_hash],
        )?;
        Ok(changed > 0)
    }

    fn append_usage_event(&mut self, event: &UsageEvent) -> Result<bool> {
        let inserted = self.connection.execute("INSERT OR IGNORE INTO agentusage_usage_events (event_id,occurred_at,provider_id,agent_name,account_id,session_id,model,client,project,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cache_write_tokens,total_tokens,cost_usd,ai_units_nano,request_multiplier,ai_credits,requests,prompts,lines_added,lines_removed,dedup_key,raw_event_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25)", params![event.event_id, event.occurred_at.to_rfc3339(), event.provider_id, event.agent_name, event.account_id, event.session_id, event.model, event.client, event.project, event.input_tokens, event.output_tokens, event.reasoning_tokens, event.cache_read_tokens, event.cache_write_tokens, event.total_tokens, event.cost_usd, event.ai_units_nano, event.request_multiplier, event.ai_credits, event.requests, event.prompts, event.lines_added, event.lines_removed, event.dedup_key, event.raw_event_id])?;
        if inserted == 0 && event.project.is_some() {
            self.connection.execute("UPDATE agentusage_usage_events SET project=?1 WHERE dedup_key=?2 AND (project IS NULL OR project='unknown')", params![event.project, event.dedup_key])?;
        }
        Ok(inserted > 0)
    }

    fn upsert_usage_event(&mut self, event: &UsageEvent) -> Result<bool> {
        let changed = self.connection.execute(
            "INSERT INTO agentusage_usage_events (event_id,occurred_at,provider_id,agent_name,account_id,session_id,model,client,project,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cache_write_tokens,total_tokens,cost_usd,ai_units_nano,request_multiplier,ai_credits,requests,prompts,lines_added,lines_removed,dedup_key,raw_event_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25) ON CONFLICT(event_id) DO UPDATE SET occurred_at=excluded.occurred_at,provider_id=excluded.provider_id,agent_name=excluded.agent_name,account_id=excluded.account_id,session_id=excluded.session_id,model=excluded.model,client=excluded.client,project=excluded.project,input_tokens=excluded.input_tokens,output_tokens=excluded.output_tokens,reasoning_tokens=excluded.reasoning_tokens,cache_read_tokens=excluded.cache_read_tokens,cache_write_tokens=excluded.cache_write_tokens,total_tokens=excluded.total_tokens,cost_usd=excluded.cost_usd,ai_units_nano=excluded.ai_units_nano,request_multiplier=excluded.request_multiplier,ai_credits=excluded.ai_credits,requests=excluded.requests,prompts=excluded.prompts,lines_added=excluded.lines_added,lines_removed=excluded.lines_removed,dedup_key=excluded.dedup_key,raw_event_id=excluded.raw_event_id",
            params![event.event_id, event.occurred_at.to_rfc3339(), event.provider_id, event.agent_name, event.account_id, event.session_id, event.model, event.client, event.project, event.input_tokens, event.output_tokens, event.reasoning_tokens, event.cache_read_tokens, event.cache_write_tokens, event.total_tokens, event.cost_usd, event.ai_units_nano, event.request_multiplier, event.ai_credits, event.requests, event.prompts, event.lines_added, event.lines_removed, event.dedup_key, event.raw_event_id],
        )?;
        Ok(changed > 0)
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
        let mut statement = self.connection.prepare("SELECT e.event_id,e.occurred_at,e.provider_id,e.agent_name,e.account_id,e.session_id,e.model,e.client,e.project,e.input_tokens,e.output_tokens,e.reasoning_tokens,e.cache_read_tokens,e.cache_write_tokens,e.total_tokens,e.cost_usd,e.ai_units_nano,e.request_multiplier,e.ai_credits,e.requests,e.prompts,e.lines_added,e.lines_removed,e.dedup_key,e.raw_event_id FROM agentusage_usage_events e JOIN agentusage_usage_raw_events raw ON raw.event_id = e.raw_event_id WHERE e.occurred_at >= ?1 AND e.occurred_at < ?2 AND (?3 IS NULL OR e.agent_name = ?3)")?;
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
        if let Some(agent_name) = agent_name {
            self.attach_rate_limit(&mut summary, agent_name)?;
        }
        summary.sessions = self.connection.query_row("SELECT COUNT(DISTINCT session_id) FROM agentusage_usage_events WHERE occurred_at >= ?1 AND occurred_at < ?2 AND (?3 IS NULL OR agent_name = ?3)", params![from.to_rfc3339(), to.to_rfc3339(), agent_name], |row| row.get(0))?;
        Ok(summary)
    }

    fn usage_events(
        &mut self,
        agent_name: &str,
        query: &UsageEventQuery,
    ) -> Result<Vec<UsageEventDetail>> {
        let mut before_at = query
            .before
            .as_ref()
            .map(|cursor| cursor.occurred_at)
            .unwrap_or(query.to)
            .min(query.to);
        let mut before_id = query
            .before
            .as_ref()
            .map(|cursor| cursor.event_id.clone())
            .unwrap_or_else(|| "\u{10ffff}".into());
        let requested_limit = query.limit.clamp(1, 200);
        let fetch_limit = if query.status.is_some() {
            200
        } else {
            requested_limit
        } as i64;
        let mut statement = self.connection.prepare(
            "SELECT e.event_id,e.occurred_at,e.provider_id,e.agent_name,e.account_id,e.session_id,e.model,e.client,e.project,e.input_tokens,e.output_tokens,e.reasoning_tokens,e.cache_read_tokens,e.cache_write_tokens,e.total_tokens,e.cost_usd,e.ai_units_nano,e.request_multiplier,e.ai_credits,e.requests,e.prompts,e.lines_added,e.lines_removed,e.dedup_key,e.raw_event_id,raw.source_system,raw.source_channel,raw.payload FROM agentusage_usage_events e JOIN agentusage_usage_raw_events raw ON raw.event_id=e.raw_event_id WHERE e.agent_name=?1 AND e.occurred_at>=?2 AND e.occurred_at<?3 AND (e.occurred_at<?4 OR (e.occurred_at=?4 AND e.event_id<?5)) AND (?6 IS NULL OR e.model=?6) AND (?7 IS NULL OR e.session_id=?7) ORDER BY e.occurred_at DESC,e.event_id DESC LIMIT ?8",
        )?;
        let mut events = Vec::new();
        loop {
            let batch = statement
                .query_map(
                    params![
                        agent_name,
                        query.from.to_rfc3339(),
                        query.to.to_rfc3339(),
                        before_at.to_rfc3339(),
                        before_id,
                        query.model,
                        query.session_id,
                        fetch_limit
                    ],
                    sqlite_event_parts,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let batch_len = batch.len();
            let Some(last) = batch.last() else {
                break;
            };
            before_at = last.0.occurred_at;
            before_id = last.0.event_id.clone();
            for (usage, source_system, source_channel, payload) in batch {
                let detail = usage_event_detail(
                    usage,
                    source_system,
                    source_channel,
                    serde_json::from_str(&payload)?,
                    false,
                );
                if query
                    .status
                    .as_ref()
                    .is_none_or(|status| detail.status.eq_ignore_ascii_case(status.trim()))
                {
                    events.push(detail);
                    if events.len() == requested_limit {
                        return Ok(events);
                    }
                }
            }
            if batch_len < fetch_limit as usize {
                break;
            }
        }
        Ok(events)
    }

    fn usage_event(&mut self, event_id: &str) -> Result<Option<UsageEventDetail>> {
        let row = self
            .connection
            .query_row(
                "SELECT e.event_id,e.occurred_at,e.provider_id,e.agent_name,e.account_id,e.session_id,e.model,e.client,e.project,e.input_tokens,e.output_tokens,e.reasoning_tokens,e.cache_read_tokens,e.cache_write_tokens,e.total_tokens,e.cost_usd,e.ai_units_nano,e.request_multiplier,e.ai_credits,e.requests,e.prompts,e.lines_added,e.lines_removed,e.dedup_key,e.raw_event_id,raw.source_system,raw.source_channel,raw.payload FROM agentusage_usage_events e JOIN agentusage_usage_raw_events raw ON raw.event_id=e.raw_event_id WHERE e.event_id=?1",
                [event_id],
                sqlite_event_parts,
            )
            .optional()?;
        row.map(|(usage, source_system, source_channel, payload)| {
            Ok(usage_event_detail(
                usage,
                source_system,
                source_channel,
                serde_json::from_str(&payload)?,
                true,
            ))
        })
        .transpose()
    }

    fn prompts(&mut self, agent_name: &str, query: &PromptQuery) -> Result<Vec<PromptDetail>> {
        let mut before_at = query
            .before
            .as_ref()
            .map(|cursor| cursor.occurred_at)
            .unwrap_or(query.to)
            .min(query.to);
        let mut before_id = query
            .before
            .as_ref()
            .map(|cursor| cursor.event_id.clone())
            .unwrap_or_else(|| "\u{10ffff}".into());
        let requested_limit = query.limit.clamp(1, 200);
        let fetch_limit = 200_i64;
        let search = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_lowercase);
        let mut statement = self.connection.prepare(
            "SELECT e.event_id,e.occurred_at,e.provider_id,e.agent_name,e.account_id,e.session_id,e.model,e.client,e.project,e.input_tokens,e.output_tokens,e.reasoning_tokens,e.cache_read_tokens,e.cache_write_tokens,e.total_tokens,e.cost_usd,e.ai_units_nano,e.request_multiplier,e.ai_credits,e.requests,e.prompts,e.lines_added,e.lines_removed,e.dedup_key,e.raw_event_id,raw.source_system,raw.source_channel,raw.payload FROM agentusage_usage_events e JOIN agentusage_usage_raw_events raw ON raw.event_id=e.raw_event_id WHERE e.agent_name=?1 AND e.prompts>0 AND e.occurred_at>=?2 AND e.occurred_at<?3 AND (e.occurred_at<?4 OR (e.occurred_at=?4 AND e.event_id<?5)) AND (?6 IS NULL OR e.session_id=?6) ORDER BY e.occurred_at DESC,e.event_id DESC LIMIT ?7",
        )?;
        let mut prompts = Vec::new();
        loop {
            let batch = statement
                .query_map(
                    params![
                        agent_name,
                        query.from.to_rfc3339(),
                        query.to.to_rfc3339(),
                        before_at.to_rfc3339(),
                        before_id,
                        query.session_id,
                        fetch_limit
                    ],
                    sqlite_event_parts,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let batch_len = batch.len();
            let Some(last) = batch.last() else {
                break;
            };
            before_at = last.0.occurred_at;
            before_id = last.0.event_id.clone();
            for (usage, source_system, source_channel, payload) in batch {
                let Some(prompt) = prompt_detail(
                    usage,
                    source_system,
                    source_channel,
                    serde_json::from_str(&payload)?,
                ) else {
                    continue;
                };
                if search
                    .as_ref()
                    .is_none_or(|search| prompt.text.to_lowercase().contains(search))
                {
                    prompts.push(prompt);
                    if prompts.len() == requested_limit {
                        return Ok(prompts);
                    }
                }
            }
            if batch_len < fetch_limit as usize {
                break;
            }
        }
        Ok(prompts)
    }
}

fn sqlite_event_parts(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(UsageEvent, String, String, String)> {
    Ok((
        UsageEvent {
            event_id: row.get(0)?,
            occurred_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(1)?)
                .map(|value| value.with_timezone(&Utc))
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
        },
        row.get(25)?,
        row.get(26)?,
        row.get(27)?,
    ))
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
            .summary_for_agent(
                None,
                occurred_at - chrono::Duration::minutes(1),
                occurred_at + chrono::Duration::minutes(1),
            )
            .unwrap();
        assert_eq!(summary.total_tokens, 16);
        assert_eq!(summary.sessions, 1);
        assert_eq!(
            crate::core::TokenSemantics::Additive.cache_hit_rate(
                summary.input_tokens,
                summary.cache_read_tokens,
                summary.cache_write_tokens,
            ),
            Some(37.5)
        );
        assert_eq!(summary.models["gpt-5"].total_tokens, 16);
        assert_eq!(summary.projects["agentusage"].total_tokens, 16);
        let trend = store
            .daily_trend_for_agent(
                "codex",
                occurred_at - chrono::Duration::minutes(1),
                occurred_at + chrono::Duration::minutes(1),
            )
            .unwrap();
        assert_eq!(trend.len(), 1);
        assert_eq!(
            trend[0].date,
            occurred_at.with_timezone(&Local).date_naive()
        );
        assert_eq!(trend[0].total_tokens, 16);
        assert_eq!(trend[0].models["gpt-5"], 16);
    }

    #[test]
    fn event_queries_are_paginated_and_include_auditable_detail() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let occurred_at = Utc.with_ymd_and_hms(2026, 7, 19, 12, 0, 0).unwrap();
        for (event_id, request_id, total) in [("event-b", "req-b", 20), ("event-a", "req-a", 10)] {
            let raw_id = format!("raw-{event_id}");
            store
                .append_raw_event(&RawEvent {
                    event_id: raw_id.clone(),
                    source_system: "codex".into(),
                    source_channel: "jsonl".into(),
                    occurred_at,
                    payload: serde_json::json!({
                        "request_id": request_id,
                        "status": "completed",
                        "duration_ms": 42,
                        "usage": {"total_tokens": total}
                    }),
                    payload_hash: format!("hash-{event_id}"),
                })
                .unwrap();
            store
                .append_usage_event(&UsageEvent {
                    event_id: event_id.into(),
                    occurred_at,
                    provider_id: "codex".into(),
                    agent_name: "codex".into(),
                    session_id: Some("session-1".into()),
                    model: Some("gpt-5".into()),
                    total_tokens: total,
                    requests: 1,
                    dedup_key: format!("dedup-{event_id}"),
                    raw_event_id: raw_id,
                    ..Default::default()
                })
                .unwrap();
        }
        let base = UsageEventQuery {
            from: occurred_at - chrono::Duration::minutes(1),
            to: occurred_at + chrono::Duration::minutes(1),
            before: None,
            limit: 1,
            model: None,
            session_id: None,
            status: None,
        };
        let first = store.usage_events("codex", &base).unwrap();
        assert_eq!(first[0].usage.event_id, "event-b");
        assert_eq!(first[0].source_request_id.as_deref(), Some("req-b"));
        assert_eq!(first[0].status, "completed");
        assert_eq!(first[0].duration_ms, Some(42));
        assert_eq!(first[0].total_source, "provider_reported");

        let second = store
            .usage_events(
                "codex",
                &UsageEventQuery {
                    before: Some(crate::storage::UsageEventCursor {
                        occurred_at,
                        event_id: first[0].usage.event_id.clone(),
                    }),
                    ..base
                },
            )
            .unwrap();
        assert_eq!(second[0].usage.event_id, "event-a");
        let detail = store.usage_event("event-a").unwrap().unwrap();
        assert!(detail.raw_payload.is_some());
    }

    #[test]
    fn rejects_obsolete_usage_schemas_instead_of_mutating_them() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute(
                "CREATE TABLE agentusage_usage_events (event_id TEXT PRIMARY KEY)",
                [],
            )
            .unwrap();
        let store = SqliteStore { connection };
        let error = store.validate_schema().unwrap_err().to_string();
        assert!(error.contains("obsolete SQLite usage schema"));
    }

    #[test]
    fn rebuild_replaces_an_obsolete_database_and_its_sidecars() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("codex.db");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE agentusage_usage_events (event_id TEXT PRIMARY KEY);
                 CREATE TABLE legacy_marker (value TEXT);",
            )
            .unwrap();
        drop(connection);
        for sidecar in sqlite_files(&path).into_iter().skip(1) {
            std::fs::write(sidecar, b"obsolete").unwrap();
        }

        let store = SqliteStore::rebuild(&path).unwrap();
        store.validate_schema().unwrap();
        let has_legacy_marker: bool = store
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='legacy_marker')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!has_legacy_marker);
    }

    #[test]
    fn status_filters_scan_past_non_matching_storage_batches() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let occurred_at = Utc.with_ymd_and_hms(2026, 7, 19, 12, 0, 0).unwrap();
        for index in 0..205 {
            let event_id = format!("event-{index:03}");
            let raw_id = format!("raw-{index:03}");
            store
                .append_raw_event(&RawEvent {
                    event_id: raw_id.clone(),
                    source_system: "codex".into(),
                    source_channel: "jsonl".into(),
                    occurred_at,
                    payload: serde_json::json!({
                        "status": if index == 0 { "cancelled" } else { "completed" }
                    }),
                    payload_hash: format!("hash-{index:03}"),
                })
                .unwrap();
            store
                .append_usage_event(&UsageEvent {
                    event_id: event_id.clone(),
                    occurred_at,
                    provider_id: "codex".into(),
                    agent_name: "codex".into(),
                    requests: 1,
                    dedup_key: event_id,
                    raw_event_id: raw_id,
                    ..Default::default()
                })
                .unwrap();
        }
        let events = store
            .usage_events(
                "codex",
                &UsageEventQuery {
                    from: occurred_at - chrono::Duration::minutes(1),
                    to: occurred_at + chrono::Duration::minutes(1),
                    before: None,
                    limit: 1,
                    model: None,
                    session_id: None,
                    status: Some("cancelled".into()),
                },
            )
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].usage.event_id, "event-000");
    }
}
