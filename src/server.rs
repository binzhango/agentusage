use anyhow::{Context, Result};
use chrono::{Duration, Local, NaiveDate};
use serde::Serialize;
use std::{collections::BTreeMap, process::Command, thread};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const PROVIDERS: [&str; 5] = ["codex", "claude_code", "opencode", "copilot", "pi"];

#[derive(Debug, Serialize)]
struct ProviderStatus {
    name: String,
    available: bool,
}

#[derive(Debug, Serialize)]
struct TrendPoint {
    date: NaiveDate,
    total_tokens: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    models: BTreeMap<String, i64>,
}

pub fn run(host: &str, port: u16, open: bool) -> Result<()> {
    let address = format!("{host}:{port}");
    let server = Server::http(&address).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let url = format!("http://{address}");
    println!("agentusage server listening at {url}");
    start_background_ingestion(crate::config::load()?);
    if open {
        open_browser(&url);
    }
    for request in server.incoming_requests() {
        if let Err(error) = handle_request(request) {
            eprintln!("agentusage server request error: {error:#}");
        }
    }
    Ok(())
}

fn start_background_ingestion(config: crate::config::AppConfig) {
    if !config.auto_sync {
        return;
    }
    let interval = config.refresh_interval;
    for provider in PROVIDERS {
        std::thread::spawn(move || {
            loop {
                let result = (|| -> Result<()> {
                    let mode = crate::storage::prepare_backend_for_agent(false, provider)?;
                    let mut store = crate::storage::Backend::open_for_agent(mode, provider)?;
                    let _ = crate::ingest_provider(provider, None, &mut store)?;
                    Ok(())
                })();
                if let Err(error) = result {
                    eprintln!("agentusage server background ingestion ({provider}): {error:#}");
                }
                thread::sleep(interval);
            }
        });
    }
}

fn handle_request(request: Request) -> Result<()> {
    let url = request.url().to_owned();
    match (request.method(), url.split('?').next().unwrap_or("/")) {
        (&Method::Get, "/") => respond_html(request),
        (&Method::Get, "/api/providers") => respond_json(request, providers()),
        (&Method::Get, "/api/summary") => {
            let query = url.split_once('?').map(|(_, value)| value).unwrap_or("");
            let params = query_params(query);
            let provider = params
                .get("provider")
                .map(String::as_str)
                .unwrap_or("codex");
            let window = params.get("window").map(String::as_str).unwrap_or("today");
            let summary = summary(provider, window)?;
            respond_json(request, summary)
        }
        (&Method::Get, "/api/trend") => {
            let query = url.split_once('?').map(|(_, value)| value).unwrap_or("");
            let params = query_params(query);
            let provider = params
                .get("provider")
                .map(String::as_str)
                .unwrap_or("codex");
            let window = params.get("window").map(String::as_str).unwrap_or("today");
            respond_json(request, trend(provider, window)?)
        }
        _ => {
            let response = Response::from_string("not found").with_status_code(StatusCode(404));
            request.respond(response)?;
            Ok(())
        }
    }
}

fn providers() -> Vec<ProviderStatus> {
    PROVIDERS
        .iter()
        .map(|name| ProviderStatus {
            name: (*name).to_owned(),
            available: backend_for(name).is_ok(),
        })
        .collect()
}

fn summary(provider: &str, window: &str) -> Result<crate::storage::UsageSummary> {
    let (start, end) = window_dates(window)?;
    let from = crate::local_midnight_utc(start);
    let to = crate::local_midnight_utc(end + Duration::days(1));
    let mut store = backend_for(provider)
        .with_context(|| format!("no initialized storage for provider {provider}"))?;
    store.quick_summary_for_agent(agent_name(provider), from, to)
}

fn trend(provider: &str, window: &str) -> Result<Vec<TrendPoint>> {
    let (mut start, end) = window_dates(window)?;
    // A useful chart for "all time" should remain readable and inexpensive.
    // The summary endpoint still supports the complete all-time range.
    if window == "all" || window == "all_time" {
        start = end - Duration::days(89);
    }
    let mut points = Vec::new();
    let mut day = start;
    while day <= end {
        let next = day + Duration::days(1);
        let from = crate::local_midnight_utc(day);
        let to = crate::local_midnight_utc(next);
        let mut store = backend_for(provider)
            .with_context(|| format!("no initialized storage for provider {provider}"))?;
        let point = store.quick_summary_for_agent(agent_name(provider), from, to)?;
        points.push(TrendPoint {
            date: day,
            total_tokens: point.total_tokens,
            input_tokens: point.input_tokens,
            output_tokens: point.output_tokens,
            cache_read_tokens: point.cache_read_tokens,
            models: point
                .models
                .into_iter()
                .map(|(name, bucket)| (name, bucket.total_tokens))
                .collect(),
        });
        day = next;
    }
    Ok(points)
}

fn backend_for(provider: &str) -> Result<crate::storage::Backend> {
    let mode = crate::storage::prepare_backend_for_agent(false, provider)?;
    crate::storage::Backend::open_read_only_for_agent(mode, provider)
}

fn agent_name(provider: &str) -> &str {
    match provider {
        "claude" | "claude_code" => "claude_code",
        "opencode" | "open_code" => "opencode",
        other => other,
    }
}

fn window_dates(window: &str) -> Result<(NaiveDate, NaiveDate)> {
    let end = Local::now().date_naive();
    let start = match window {
        "today" => end,
        "7d" | "7days" => end - Duration::days(6),
        "30d" | "30days" => end - Duration::days(29),
        "all" | "all_time" => NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
        other => anyhow::bail!("unsupported window {other:?}; use today, 7d, 30d, or all"),
    };
    Ok((start, end))
}

fn query_params(query: &str) -> std::collections::BTreeMap<String, String> {
    query
        .split('&')
        .filter_map(|part| part.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

fn respond_html(request: Request) -> Result<()> {
    let response = Response::from_string(crate::view::index_html())
        .with_header(content_type("text/html; charset=utf-8"));
    request.respond(response)?;
    Ok(())
}

fn respond_json<T: Serialize>(request: Request, value: T) -> Result<()> {
    let body = serde_json::to_string(&value)?;
    request.respond(Response::from_string(body).with_header(content_type("application/json")))?;
    Ok(())
}

fn content_type(value: &str) -> Header {
    Header::from_bytes("Content-Type", value).expect("static header")
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let _ = Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = Command::new("cmd").args(["/C", "start", "", url]).spawn();
}

#[cfg(test)]
mod tests {
    use super::window_dates;

    #[test]
    fn accepts_supported_windows() {
        for window in ["today", "7d", "30d", "all"] {
            assert!(window_dates(window).is_ok());
        }
        assert!(window_dates("bad").is_err());
    }
}
