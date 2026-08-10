use super::{
    DailyUsagePoint, FileCursor, IngestRecord, PromptDetail, PromptQuery, RawEvent, UsageBucket,
    UsageEvent, UsageEventDetail, UsageEventQuery, UsageMetric, UsageStore, UsageSummary,
    add_event, prompt_detail, usage_event_detail,
};
use anyhow::Result;
use chrono::{DateTime, Local, Utc};
use postgres::{Client, Config, NoTls, config::Host};
use std::{collections::BTreeMap, net::IpAddr, str::FromStr};

pub struct PostgresStore {
    client: Client,
}

impl PostgresStore {
    pub fn connect(url: &str) -> Result<Self> {
        let mut store = Self::connect_without_init(url)?;
        store.init()?;
        Ok(store)
    }

    pub fn connect_read_only(url: &str) -> Result<Self> {
        let mut store = Self::connect_without_init(url)?;
        store.validate_schema()?;
        store
            .client
            .batch_execute("SET default_transaction_read_only = on")?;
        Ok(store)
    }

    fn connect_without_init(url: &str) -> Result<Self> {
        let config = Config::from_str(url)?;
        for host in config.get_hosts() {
            if let Host::Tcp(host) = host {
                let local = host.eq_ignore_ascii_case("localhost")
                    || host
                        .parse::<IpAddr>()
                        .is_ok_and(|address| address.is_loopback());
                if !local {
                    anyhow::bail!(
                        "remote PostgreSQL is disabled because this build does not provide TLS; use a local socket or loopback host"
                    );
                }
            }
        }
        Ok(Self {
            client: config.connect(NoTls)?,
        })
    }

    fn init(&mut self) -> Result<()> {
        let initialized: bool = self
            .client
            .query_one(
                "SELECT to_regclass('agentusage_usage_events') IS NOT NULL",
                &[],
            )?
            .get(0);
        if initialized {
            self.validate_schema()?;
        }
        self.client.batch_execute(super::schema::POSTGRES)?;
        Ok(())
    }

    fn validate_schema(&mut self) -> Result<()> {
        let initialized: bool = self
            .client
            .query_one(
                "SELECT to_regclass('agentusage_usage_events') IS NOT NULL",
                &[],
            )?
            .get(0);
        if !initialized {
            anyhow::bail!("PostgreSQL usage schema is not initialized");
        }
        let has_version_table: bool = self
            .client
            .query_one("SELECT to_regclass('agentusage_schema') IS NOT NULL", &[])?
            .get(0);
        if !has_version_table {
            anyhow::bail!(
                "obsolete PostgreSQL usage schema; replace the derived schema and synchronize again"
            );
        }
        let version = self
            .client
            .query_opt(
                "SELECT version FROM agentusage_schema WHERE singleton=1",
                &[],
            )?
            .map(|row| row.get::<_, i64>(0));
        if version != Some(super::schema::VERSION) {
            anyhow::bail!(
                "obsolete PostgreSQL usage schema; replace the derived schema and synchronize again"
            );
        }
        let rows = self.client.query(
            "SELECT column_name FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='agentusage_usage_events'",
            &[],
        )?;
        let columns = rows
            .into_iter()
            .map(|row| row.get::<_, String>(0))
            .collect::<std::collections::HashSet<_>>();
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
                    "invalid PostgreSQL usage schema (missing {required}); replace the derived schema and synchronize again"
                );
            }
        }
        Ok(())
    }

    pub fn daily_trend_for_agent(
        &mut self,
        agent_name: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<DailyUsagePoint>> {
        let rows = self.client.query(
            "SELECT occurred_at, model, input_tokens, output_tokens, cache_read_tokens, total_tokens FROM agentusage_usage_events WHERE occurred_at >= $1 AND occurred_at < $2 AND agent_name = $3 ORDER BY occurred_at, model",
            &[&from, &to, &agent_name],
        )?;
        let mut points = BTreeMap::new();
        for row in rows {
            let occurred_at: DateTime<Utc> = row.get(0);
            let date = occurred_at.with_timezone(&Local).date_naive();
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

    fn upsert_raw_event(&mut self, event: &RawEvent) -> Result<bool> {
        let n = self.client.execute(
            "INSERT INTO agentusage_usage_raw_events (event_id,source_system,source_channel,occurred_at,payload,payload_hash) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (event_id) DO UPDATE SET source_system=EXCLUDED.source_system,source_channel=EXCLUDED.source_channel,occurred_at=EXCLUDED.occurred_at,payload=EXCLUDED.payload,payload_hash=EXCLUDED.payload_hash",
            &[&event.event_id, &event.source_system, &event.source_channel, &event.occurred_at, &event.payload, &event.payload_hash],
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
    fn upsert_usage_event(&mut self, event: &UsageEvent) -> Result<bool> {
        let n = self.client.execute(
            "INSERT INTO agentusage_usage_events (event_id,occurred_at,provider_id,agent_name,account_id,session_id,model,client,project,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cache_write_tokens,total_tokens,cost_usd,ai_units_nano,request_multiplier,ai_credits,requests,prompts,lines_added,lines_removed,dedup_key,raw_event_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25) ON CONFLICT (event_id) DO UPDATE SET occurred_at=EXCLUDED.occurred_at,provider_id=EXCLUDED.provider_id,agent_name=EXCLUDED.agent_name,account_id=EXCLUDED.account_id,session_id=EXCLUDED.session_id,model=EXCLUDED.model,client=EXCLUDED.client,project=EXCLUDED.project,input_tokens=EXCLUDED.input_tokens,output_tokens=EXCLUDED.output_tokens,reasoning_tokens=EXCLUDED.reasoning_tokens,cache_read_tokens=EXCLUDED.cache_read_tokens,cache_write_tokens=EXCLUDED.cache_write_tokens,total_tokens=EXCLUDED.total_tokens,cost_usd=EXCLUDED.cost_usd,ai_units_nano=EXCLUDED.ai_units_nano,request_multiplier=EXCLUDED.request_multiplier,ai_credits=EXCLUDED.ai_credits,requests=EXCLUDED.requests,prompts=EXCLUDED.prompts,lines_added=EXCLUDED.lines_added,lines_removed=EXCLUDED.lines_removed,dedup_key=EXCLUDED.dedup_key,raw_event_id=EXCLUDED.raw_event_id",
            &[&event.event_id, &event.occurred_at, &event.provider_id, &event.agent_name, &event.account_id, &event.session_id, &event.model, &event.client, &event.project, &event.input_tokens, &event.output_tokens, &event.reasoning_tokens, &event.cache_read_tokens, &event.cache_write_tokens, &event.total_tokens, &event.cost_usd, &event.ai_units_nano, &event.request_multiplier, &event.ai_credits, &event.requests, &event.prompts, &event.lines_added, &event.lines_removed, &event.dedup_key, &event.raw_event_id],
        )?;
        Ok(n > 0)
    }
    fn cursor(&mut self, path: &str) -> Result<Option<FileCursor>> {
        Ok(self.client.query_opt("SELECT path,byte_offset,file_size,last_event_hash,updated_at FROM agentusage_ingest_cursors WHERE path=$1", &[&path])?.map(|row| FileCursor { path: row.get(0), byte_offset: row.get(1), file_size: row.get(2), last_event_hash: row.get(3), updated_at: row.get(4) }))
    }
    fn save_cursor(&mut self, cursor: &FileCursor) -> Result<()> {
        self.client.execute("INSERT INTO agentusage_ingest_cursors VALUES ($1,$2,$3,$4,$5) ON CONFLICT(path) DO UPDATE SET byte_offset=EXCLUDED.byte_offset,file_size=EXCLUDED.file_size,last_event_hash=EXCLUDED.last_event_hash,updated_at=EXCLUDED.updated_at", &[&cursor.path, &cursor.byte_offset, &cursor.file_size, &cursor.last_event_hash, &cursor.updated_at])?;
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
        let rows = self.client.query("SELECT e.event_id,e.occurred_at,e.provider_id,e.agent_name,e.account_id,e.session_id,e.model,e.client,e.project,e.input_tokens,e.output_tokens,e.reasoning_tokens,e.cache_read_tokens,e.cache_write_tokens,e.total_tokens,e.cost_usd,e.ai_units_nano,e.request_multiplier,e.ai_credits,e.requests,e.prompts,e.lines_added,e.lines_removed,e.dedup_key,e.raw_event_id FROM agentusage_usage_events e JOIN agentusage_usage_raw_events raw ON raw.event_id = e.raw_event_id WHERE e.occurred_at >= $1 AND e.occurred_at < $2 AND ($3::text IS NULL OR e.agent_name = $3)", &[&from, &to, &agent_name])?;
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
            // PostgreSQL promotes SUM(BIGINT) to NUMERIC. Cast integer totals
            // back to BIGINT so they match the canonical i64 storage model.
            let rows = self.client.query(
                &format!(
                    "SELECT {dimension}, COALESCE(SUM(requests),0)::BIGINT, COALESCE(SUM(input_tokens),0)::BIGINT, COALESCE(SUM(output_tokens),0)::BIGINT, COALESCE(SUM(reasoning_tokens),0)::BIGINT, COALESCE(SUM(cache_read_tokens),0)::BIGINT, COALESCE(SUM(cache_write_tokens),0)::BIGINT, COALESCE(SUM(total_tokens),0)::BIGINT, COALESCE(SUM(cost_usd),0), COALESCE(SUM(ai_units_nano),0)::BIGINT, COALESCE(SUM(request_multiplier),0), COALESCE(SUM(ai_credits),0) FROM agentusage_usage_events WHERE occurred_at >= $1 AND occurred_at < $2 AND ($3::text IS NULL OR agent_name = $3) AND {dimension} IS NOT NULL AND {dimension} <> '' GROUP BY {dimension}"
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
        let mut events = Vec::new();
        loop {
            let rows = self.client.query(
                "SELECT e.event_id,e.occurred_at,e.provider_id,e.agent_name,e.account_id,e.session_id,e.model,e.client,e.project,e.input_tokens,e.output_tokens,e.reasoning_tokens,e.cache_read_tokens,e.cache_write_tokens,e.total_tokens,e.cost_usd,e.ai_units_nano,e.request_multiplier,e.ai_credits,e.requests,e.prompts,e.lines_added,e.lines_removed,e.dedup_key,e.raw_event_id,raw.source_system,raw.source_channel,raw.payload FROM agentusage_usage_events e JOIN agentusage_usage_raw_events raw ON raw.event_id=e.raw_event_id WHERE e.agent_name=$1 AND e.occurred_at>=$2 AND e.occurred_at<$3 AND (e.occurred_at<$4 OR (e.occurred_at=$4 AND e.event_id<$5)) AND ($6::text IS NULL OR e.model=$6) AND ($7::text IS NULL OR e.session_id=$7) ORDER BY e.occurred_at DESC,e.event_id DESC LIMIT $8",
                &[&agent_name, &query.from, &query.to, &before_at, &before_id, &query.model, &query.session_id, &fetch_limit],
            )?;
            let batch_len = rows.len();
            let Some(last) = rows.last() else {
                break;
            };
            before_at = last.get(1);
            before_id = last.get(0);
            for row in rows {
                let detail = postgres_event_detail(&row, false);
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
        Ok(self
            .client
            .query_opt(
                "SELECT e.event_id,e.occurred_at,e.provider_id,e.agent_name,e.account_id,e.session_id,e.model,e.client,e.project,e.input_tokens,e.output_tokens,e.reasoning_tokens,e.cache_read_tokens,e.cache_write_tokens,e.total_tokens,e.cost_usd,e.ai_units_nano,e.request_multiplier,e.ai_credits,e.requests,e.prompts,e.lines_added,e.lines_removed,e.dedup_key,e.raw_event_id,raw.source_system,raw.source_channel,raw.payload FROM agentusage_usage_events e JOIN agentusage_usage_raw_events raw ON raw.event_id=e.raw_event_id WHERE e.event_id=$1",
                &[&event_id],
            )?
            .map(|row| postgres_event_detail(&row, true)))
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
        let mut prompts = Vec::new();
        loop {
            let rows = self.client.query(
                "SELECT e.event_id,e.occurred_at,e.provider_id,e.agent_name,e.account_id,e.session_id,e.model,e.client,e.project,e.input_tokens,e.output_tokens,e.reasoning_tokens,e.cache_read_tokens,e.cache_write_tokens,e.total_tokens,e.cost_usd,e.ai_units_nano,e.request_multiplier,e.ai_credits,e.requests,e.prompts,e.lines_added,e.lines_removed,e.dedup_key,e.raw_event_id,raw.source_system,raw.source_channel,raw.payload FROM agentusage_usage_events e JOIN agentusage_usage_raw_events raw ON raw.event_id=e.raw_event_id WHERE e.agent_name=$1 AND e.prompts>0 AND e.occurred_at>=$2 AND e.occurred_at<$3 AND (e.occurred_at<$4 OR (e.occurred_at=$4 AND e.event_id<$5)) AND ($6::text IS NULL OR e.session_id=$6) ORDER BY e.occurred_at DESC,e.event_id DESC LIMIT $7",
                &[&agent_name, &query.from, &query.to, &before_at, &before_id, &query.session_id, &fetch_limit],
            )?;
            let batch_len = rows.len();
            let Some(last) = rows.last() else {
                break;
            };
            before_at = last.get(1);
            before_id = last.get(0);
            for row in rows {
                let event = postgres_event_detail(&row, true);
                let Some(prompt) = event.raw_payload.and_then(|payload| {
                    prompt_detail(
                        event.usage,
                        event.source_system,
                        event.source_channel,
                        payload,
                    )
                }) else {
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

fn postgres_event_detail(row: &postgres::Row, include_raw: bool) -> UsageEventDetail {
    usage_event_detail(
        UsageEvent {
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
        row.get(25),
        row.get(26),
        row.get(27),
        include_raw,
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{RawEvent, UsageEvent, UsageMetric, sqlite::SqliteStore};
    use chrono::{Duration, TimeZone};
    use std::env;
    use uuid::Uuid;

    fn seed<S: UsageStore>(store: &mut S) {
        let occurred_at = Utc.with_ymd_and_hms(2026, 7, 19, 23, 55, 0).unwrap();
        for (suffix, input, output, status) in
            [("a", 100, 20, "completed"), ("b", 40, 10, "cancelled")]
        {
            let raw_id = format!("raw-{suffix}");
            store
                .append_raw_event(&RawEvent {
                    event_id: raw_id.clone(),
                    source_system: "codex".into(),
                    source_channel: "jsonl".into(),
                    occurred_at,
                    payload: serde_json::json!({
                        "request_id": format!("request-{suffix}"),
                        "status": status,
                        "duration_ms": 125,
                        "source_path": "session.jsonl",
                        "line_number": 8,
                        "payload": {
                            "type": "user_message",
                            "message": format!("prompt {suffix}")
                        }
                    }),
                    payload_hash: format!("hash-{suffix}"),
                })
                .unwrap();
            store
                .append_usage_event(&UsageEvent {
                    event_id: format!("event-{suffix}"),
                    occurred_at,
                    provider_id: "openai".into(),
                    agent_name: "codex".into(),
                    session_id: Some(format!("session-{suffix}")),
                    model: Some("gpt-5".into()),
                    client: Some("cli".into()),
                    project: Some("agentusage".into()),
                    input_tokens: input,
                    output_tokens: output,
                    reasoning_tokens: 5,
                    cache_read_tokens: 30,
                    total_tokens: input + output,
                    cost_usd: 0.01,
                    requests: 1,
                    prompts: 1,
                    dedup_key: format!("dedup-{suffix}"),
                    raw_event_id: raw_id,
                    ..Default::default()
                })
                .unwrap();
        }
        store
            .append_metric(&UsageMetric {
                metric_id: "metric-1".into(),
                occurred_at,
                provider_id: "openai".into(),
                agent_name: "codex".into(),
                session_id: Some("session-a".into()),
                dimension: "tool".into(),
                name: "apply_patch".into(),
                dedup_key: "metric-dedup-1".into(),
            })
            .unwrap();
    }

    #[test]
    fn sqlite_and_postgres_return_equivalent_usage_views() {
        let Ok(url) = env::var("AGENTUSAGE_TEST_POSTGRES_URL") else {
            return;
        };
        let schema = format!("agentusage_test_{}", Uuid::new_v4().simple());
        let config = Config::from_str(&url).unwrap();
        let mut client = config.connect(NoTls).unwrap();
        client
            .batch_execute(&format!(
                "CREATE SCHEMA {schema}; SET search_path TO {schema}"
            ))
            .unwrap();
        let mut postgres = PostgresStore { client };
        postgres.init().unwrap();
        let mut sqlite = SqliteStore::open_in_memory().unwrap();
        seed(&mut sqlite);
        seed(&mut postgres);

        let from = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
        let to = from + Duration::days(2);
        let sqlite_summary = sqlite.summary_for_agent(Some("codex"), from, to).unwrap();
        let postgres_summary = postgres.summary_for_agent(Some("codex"), from, to).unwrap();
        assert_eq!(sqlite_summary.sessions, postgres_summary.sessions);
        assert_eq!(sqlite_summary.requests, postgres_summary.requests);
        assert_eq!(sqlite_summary.prompts, postgres_summary.prompts);
        assert_eq!(sqlite_summary.input_tokens, postgres_summary.input_tokens);
        assert_eq!(sqlite_summary.output_tokens, postgres_summary.output_tokens);
        assert_eq!(sqlite_summary.total_tokens, postgres_summary.total_tokens);
        assert_eq!(sqlite_summary.models.len(), postgres_summary.models.len());
        assert_eq!(
            sqlite_summary.projects.len(),
            postgres_summary.projects.len()
        );
        assert_eq!(sqlite_summary.tools, postgres_summary.tools);

        let sqlite_trend = sqlite.daily_trend_for_agent("codex", from, to).unwrap();
        let postgres_trend = postgres.daily_trend_for_agent("codex", from, to).unwrap();
        assert_eq!(sqlite_trend.len(), postgres_trend.len());
        assert_eq!(sqlite_trend[0].date, postgres_trend[0].date);
        assert_eq!(sqlite_trend[0].total_tokens, 170);
        assert_eq!(sqlite_trend[0].total_tokens, postgres_trend[0].total_tokens);

        let query = UsageEventQuery {
            from,
            to,
            before: None,
            limit: 10,
            model: Some("gpt-5".into()),
            session_id: None,
            status: Some("cancelled".into()),
        };
        let sqlite_events = sqlite.usage_events("codex", &query).unwrap();
        let postgres_events = postgres.usage_events("codex", &query).unwrap();
        assert_eq!(sqlite_events.len(), 1);
        assert_eq!(sqlite_events[0].usage.event_id, "event-b");
        assert_eq!(
            sqlite_events[0].usage.event_id,
            postgres_events[0].usage.event_id
        );
        assert_eq!(sqlite_events[0].duration_ms, postgres_events[0].duration_ms);

        let prompt_query = PromptQuery {
            from,
            to,
            before: None,
            limit: 10,
            session_id: None,
            search: Some("prompt b".into()),
        };
        let sqlite_prompts = sqlite.prompts("codex", &prompt_query).unwrap();
        let postgres_prompts = postgres.prompts("codex", &prompt_query).unwrap();
        assert_eq!(sqlite_prompts.len(), 1);
        assert_eq!(sqlite_prompts[0].text, "prompt b");
        assert_eq!(sqlite_prompts[0].text, postgres_prompts[0].text);

        postgres
            .client
            .batch_execute(&format!("DROP SCHEMA {schema} CASCADE"))
            .unwrap();
    }
}
