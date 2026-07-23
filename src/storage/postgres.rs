use super::{
    DailyUsagePoint, FileCursor, IngestRecord, RawEvent, UsageBucket, UsageEvent, UsageMetric,
    UsageStore, UsageSummary, add_event,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use postgres::{Client, NoTls};
use std::collections::BTreeMap;

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
        self.client.batch_execute(super::schema::POSTGRES)?;
        // Bring databases created by older releases up to the canonical shape.
        self.client.batch_execute("ALTER TABLE agentusage_usage_events ADD COLUMN IF NOT EXISTS project TEXT; ALTER TABLE agentusage_usage_events ADD COLUMN IF NOT EXISTS ai_units_nano BIGINT NOT NULL DEFAULT 0; ALTER TABLE agentusage_usage_events ADD COLUMN IF NOT EXISTS request_multiplier DOUBLE PRECISION NOT NULL DEFAULT 0; ALTER TABLE agentusage_usage_events ADD COLUMN IF NOT EXISTS ai_credits DOUBLE PRECISION NOT NULL DEFAULT 0;")?;
        self.client.batch_execute(
            "CREATE INDEX IF NOT EXISTS agentusage_usage_events_project ON agentusage_usage_events(project);",
        )?;
        Ok(())
    }

    pub fn daily_trend_for_agent(
        &mut self,
        agent_name: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<DailyUsagePoint>> {
        let rows = self.client.query(
            "SELECT occurred_at::date, model, COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(total_tokens),0) FROM agentusage_usage_events WHERE occurred_at >= $1 AND occurred_at < $2 AND agent_name = $3 GROUP BY 1,2 ORDER BY 1,2",
            &[&from, &to, &agent_name],
        )?;
        let mut points = BTreeMap::new();
        for row in rows {
            let date = row.get(0);
            let model: Option<String> = row.get(1);
            let point = points.entry(date).or_insert_with(|| DailyUsagePoint {
                date,
                ..Default::default()
            });
            point.input_tokens += row.get::<_, i64>(2);
            point.output_tokens += row.get::<_, i64>(3);
            point.cache_read_tokens += row.get::<_, i64>(4);
            let total_tokens = row.get::<_, i64>(5);
            point.total_tokens += total_tokens;
            if let Some(model) = model.filter(|name| !name.is_empty()) {
                *point.models.entry(model).or_default() += total_tokens;
            }
        }
        Ok(points.into_values().collect())
    }
}

impl UsageStore for PostgresStore {
    fn append_record(&mut self, record: &IngestRecord) -> Result<bool> {
        let n = self.client.execute(
            "INSERT INTO agentusage_ingest_records (record_id,source_path,line_number,occurred_at,provider_id,agent_name,session_id,event_type,payload_type,model,client,project,tool_name,payload,dedup_key) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) ON CONFLICT (dedup_key) DO NOTHING",
            &[
                &record.record_id,
                &record.source_path,
                &record.line_number,
                &record.occurred_at,
                &record.provider_id,
                &record.agent_name,
                &record.session_id,
                &record.event_type,
                &record.payload_type,
                &record.model,
                &record.client,
                &record.project,
                &record.tool_name,
                &record.payload,
                &record.dedup_key,
            ],
        )?;
        Ok(n > 0)
    }

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

    fn append_metric(&mut self, metric: &UsageMetric) -> Result<bool> {
        let n = self.client.execute(
            "INSERT INTO agentusage_usage_metrics (metric_id,occurred_at,provider_id,agent_name,session_id,dimension,name,dedup_key) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT (dedup_key) DO NOTHING",
            &[
                &metric.metric_id,
                &metric.occurred_at,
                &metric.provider_id,
                &metric.agent_name,
                &metric.session_id,
                &metric.dimension,
                &metric.name,
                &metric.dedup_key,
            ],
        )?;
        Ok(n > 0)
    }

    fn append_usage_event(&mut self, event: &UsageEvent) -> Result<bool> {
        let n = self.client.execute("INSERT INTO agentusage_usage_events (event_id,occurred_at,provider_id,agent_name,account_id,session_id,model,client,project,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cache_write_tokens,total_tokens,cost_usd,ai_units_nano,request_multiplier,ai_credits,requests,prompts,lines_added,lines_removed,dedup_key,raw_event_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25) ON CONFLICT (dedup_key) DO NOTHING", &[&event.event_id, &event.occurred_at, &event.provider_id, &event.agent_name, &event.account_id, &event.session_id, &event.model, &event.client, &event.project, &event.input_tokens, &event.output_tokens, &event.reasoning_tokens, &event.cache_read_tokens, &event.cache_write_tokens, &event.total_tokens, &event.cost_usd, &event.ai_units_nano, &event.request_multiplier, &event.ai_credits, &event.requests, &event.prompts, &event.lines_added, &event.lines_removed, &event.dedup_key, &event.raw_event_id])?;
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
        let rows = self.client.query("SELECT e.event_id,e.occurred_at,e.provider_id,e.agent_name,e.account_id,e.session_id,e.model,e.client,e.project,e.input_tokens,e.output_tokens,e.reasoning_tokens,e.cache_read_tokens,e.cache_write_tokens,e.total_tokens,e.cost_usd,e.ai_units_nano,e.request_multiplier,e.ai_credits,e.requests,e.prompts,e.lines_added,e.lines_removed,e.dedup_key,e.raw_event_id FROM agentusage_usage_events e JOIN agentusage_usage_raw_events raw ON raw.event_id = e.raw_event_id WHERE e.occurred_at >= $1 AND e.occurred_at < $2 AND ($3::text IS NULL OR e.agent_name = $3) AND NOT (e.client = 'IDE' AND e.total_tokens = 0 AND e.ai_credits = 0 AND EXISTS (SELECT 1 FROM agentusage_usage_events richer WHERE richer.model = e.model AND richer.client = 'IDE' AND richer.ai_credits > 0 AND richer.occurred_at >= $1 AND richer.occurred_at < $2)) AND NOT EXISTS (SELECT 1 FROM agentusage_usage_events duplicate JOIN agentusage_usage_raw_events duplicate_raw ON duplicate_raw.event_id = duplicate.raw_event_id WHERE duplicate_raw.payload->>'assistant_usage_event_id' IS NOT NULL AND duplicate_raw.payload->>'assistant_usage_event_id' = raw.payload->>'assistant_usage_event_id' AND duplicate.event_id < e.event_id)", &[&from, &to, &agent_name])?;
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
                    project: row.get(8),
                    input_tokens: row.get(9),
                    output_tokens: row.get(10),
                    reasoning_tokens: row.get(11),
                    cache_read_tokens: row.get(12),
                    cache_write_tokens: row.get(13),
                    total_tokens: row.get(14),
                    cost_usd: row.get(15),
                    ai_units_nano: row.get(16),
                    request_multiplier: row.get(17),
                    ai_credits: row.get(18),
                    requests: row.get(19),
                    prompts: row.get(20),
                    lines_added: row.get(21),
                    lines_removed: row.get(22),
                    dedup_key: row.get(23),
                    raw_event_id: row.get(24),
                },
            );
        }
        for dimension in ["model", "provider_id", "client"] {
            let rows = self.client.query(
                &format!(
                    "SELECT {dimension}, COALESCE(SUM(requests),0), COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(reasoning_tokens),0), COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_write_tokens),0), COALESCE(SUM(total_tokens),0), COALESCE(SUM(cost_usd),0), COALESCE(SUM(ai_units_nano),0), COALESCE(SUM(request_multiplier),0), COALESCE(SUM(ai_credits),0) FROM agentusage_usage_events WHERE occurred_at >= $1 AND occurred_at < $2 AND ($3::text IS NULL OR agent_name = $3) AND {dimension} IS NOT NULL AND {dimension} <> '' GROUP BY {dimension}"
                ),
                &[&from, &to, &agent_name],
            )?;
            for row in rows {
                let name: String = row.get(0);
                let bucket = bucket_from_row(&row);
                if dimension == "model" {
                    summary.models.insert(name, bucket);
                } else if dimension == "provider_id" {
                    summary.providers.insert(name, bucket);
                } else {
                    summary.clients.insert(name, bucket);
                }
            }
        }
        let project_rows = self.client.query(
            "SELECT COALESCE(NULLIF(e.project,''), raw.payload->'payload'->>'cwd', raw.payload->>'cwd'), COALESCE(SUM(e.requests),0), COALESCE(SUM(e.input_tokens),0), COALESCE(SUM(e.output_tokens),0), COALESCE(SUM(e.reasoning_tokens),0), COALESCE(SUM(e.cache_read_tokens),0), COALESCE(SUM(e.cache_write_tokens),0), COALESCE(SUM(e.total_tokens),0), COALESCE(SUM(e.cost_usd),0), COALESCE(SUM(e.ai_units_nano),0), COALESCE(SUM(e.request_multiplier),0), COALESCE(SUM(e.ai_credits),0) FROM agentusage_usage_events e JOIN agentusage_usage_raw_events raw ON raw.event_id=e.raw_event_id WHERE e.occurred_at >= $1 AND e.occurred_at < $2 AND ($3::text IS NULL OR e.agent_name = $3) AND COALESCE(NULLIF(e.project,''), raw.payload->'payload'->>'cwd', raw.payload->>'cwd') IS NOT NULL GROUP BY 1",
            &[&from, &to, &agent_name],
        )?;
        for row in project_rows {
            let name: String = row.get(0);
            summary.projects.insert(name, bucket_from_row(&row));
        }
        let metric_rows = self.client.query(
            "SELECT dimension,name,COUNT(*) FROM agentusage_usage_metrics WHERE occurred_at >= $1 AND occurred_at < $2 AND ($3::text IS NULL OR agent_name = $3) GROUP BY dimension,name",
            &[&from, &to, &agent_name],
        )?;
        for row in metric_rows {
            let dimension: String = row.get(0);
            let name: String = row.get(1);
            let count: i64 = row.get(2);
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
            let latest = self.client.query_opt(
                "SELECT payload FROM agentusage_usage_raw_events WHERE source_system=$1 ORDER BY occurred_at DESC LIMIT 1",
                &[&agent_name],
            )?;
            if let Some(row) = latest {
                let payload: serde_json::Value = row.get(0);
                if let Some((used, window, resets)) = super::quota_from_payload(&payload) {
                    summary.primary_used_percent = Some(used);
                    summary.primary_window_minutes = window;
                    summary.primary_resets_at = resets;
                }
            }
        }
        summary.sessions = self.client.query_one("SELECT COUNT(DISTINCT session_id) FROM agentusage_usage_events WHERE occurred_at >= $1 AND occurred_at < $2 AND ($3::text IS NULL OR agent_name = $3)", &[&from, &to, &agent_name])?.get(0);
        Ok(summary)
    }
}

fn bucket_from_row(row: &postgres::Row) -> UsageBucket {
    UsageBucket {
        requests: row.get(1),
        input_tokens: row.get(2),
        output_tokens: row.get(3),
        reasoning_tokens: row.get(4),
        cache_read_tokens: row.get(5),
        cache_write_tokens: row.get(6),
        total_tokens: row.get(7),
        cost_usd: row.get(8),
        ai_units_nano: row.get(9),
        request_multiplier: row.get(10),
        ai_credits: row.get(11),
    }
}
