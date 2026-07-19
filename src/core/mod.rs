use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Status {
    #[serde(rename = "OK")]
    Ok,
    #[serde(rename = "NEAR_LIMIT")]
    NearLimit,
    #[serde(rename = "LIMITED")]
    Limited,
    #[serde(rename = "AUTH_REQUIRED")]
    AuthRequired,
    #[serde(rename = "UNSUPPORTED")]
    Unsupported,
    #[serde(rename = "ERROR")]
    Error,
    #[default]
    #[serde(rename = "UNKNOWN")]
    Unknown,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Metric {
    pub limit: Option<f64>,
    pub remaining: Option<f64>,
    pub used: Option<f64>,
    pub unit: String,
    pub window: String,
}

impl Metric {
    pub fn percent(&self) -> f64 {
        match (self.limit, self.remaining, self.used) {
            (Some(limit), Some(remaining), _) if limit > 0.0 => remaining / limit * 100.0,
            (Some(limit), _, Some(used)) if limit > 0.0 => (limit - used) / limit * 100.0,
            _ => -1.0,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TokenUsage {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub requests: Option<i64>,
}

impl TokenUsage {
    pub fn sum_total_tokens(&mut self) {
        if self.total_tokens.is_some() {
            return;
        }
        let values = [
            self.input_tokens,
            self.output_tokens,
            self.reasoning_tokens,
            self.cache_read_tokens,
            self.cache_write_tokens,
        ];
        if values.iter().any(Option::is_some) {
            self.total_tokens = Some(values.into_iter().flatten().sum());
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct UsageSnapshot {
    pub provider_id: String,
    pub account_id: String,
    pub timestamp: DateTime<Utc>,
    pub status: Status,
    pub metrics: std::collections::BTreeMap<String, Metric>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct UsageEvent {
    pub time: DateTime<Utc>,
    pub provider_id: String,
    pub model: String,
    pub project: String,
    pub session: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub reasoning_tokens: i64,
    pub cost_usd: f64,
    pub has_cost: bool,
}

pub trait UsageProvider {
    fn id(&self) -> &str;
}

pub trait ItemizedUsageProvider {
    fn itemized_usage(&self) -> anyhow::Result<Vec<UsageEvent>>;
}
