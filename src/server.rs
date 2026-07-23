use anyhow::{Context, Result};
use chrono::{Duration, Local, NaiveDate};
use serde::Serialize;
use std::{collections::BTreeMap, process::Command, thread, time::Instant};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const PROVIDERS: [&str; 5] = ["codex", "claude_code", "opencode", "copilot", "pi"];

macro_rules! server_log {
    ($($arg:tt)*) => {
        eprintln!(
            "[{}] {}",
            Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            format_args!($($arg)*)
        )
    };
}

#[derive(Debug, Serialize)]
struct ProviderStatus {
    name: String,
    available: bool,
}

#[derive(Debug, Clone, Serialize)]
struct TrendPoint {
    date: NaiveDate,
    total_tokens: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    models: BTreeMap<String, i64>,
}

pub fn run(host: &str, port: u16, open: bool, verbose: bool) -> Result<()> {
    let address = format!("{host}:{port}");
    let server = Server::http(&address).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let url = format!("http://{address}");
    println!("Agentusage server listening at {url}");
    if verbose {
        server_log!("server.start detail_logging=enabled");
    }
    start_background_ingestion(crate::config::load()?, verbose);
    if open {
        open_browser(&url);
    }
    for request in server.incoming_requests() {
        if let Err(error) = handle_request(request, verbose) {
            server_log!("request.error error={error:#}");
        }
    }
    Ok(())
}

fn start_background_ingestion(config: crate::config::AppConfig, verbose: bool) {
    if !config.auto_sync {
        return;
    }
    let interval = config.refresh_interval;
    for provider in PROVIDERS {
        std::thread::spawn(move || {
            loop {
                let cycle_started = Instant::now();
                if verbose {
                    server_log!("ingest.start provider={provider}");
                }
                let result = (|| -> Result<()> {
                    let mode = crate::storage::prepare_backend_for_agent(false, provider)?;
                    let mut store = crate::storage::Backend::open_for_agent(mode, provider)?;
                    let _ = crate::ingest_provider(provider, None, &mut store)?;
                    Ok(())
                })();
                if let Err(error) = result {
                    server_log!("ingest.error provider={provider} error={error:#}");
                } else if verbose {
                    server_log!(
                        "ingest.complete provider={provider} duration_ms={}",
                        cycle_started.elapsed().as_secs_f64() * 1000.0
                    );
                }
                thread::sleep(interval);
            }
        });
    }
}

fn handle_request(request: Request, verbose: bool) -> Result<()> {
    let url = request.url().to_owned();
    let path = url.split('?').next().unwrap_or("/").to_owned();
    let started = Instant::now();
    if verbose {
        server_log!("request.start method={} path={path}", request.method());
    }
    let result = (|| -> Result<()> {
        match (request.method(), path.as_str()) {
            (&Method::Get, "/") => respond_html(request),
            (&Method::Get, "/api/providers") => respond_json(request, providers(verbose)),
            (&Method::Get, "/api/summary") => {
                let query = url.split_once('?').map(|(_, value)| value).unwrap_or("");
                let params = query_params(query);
                let provider = params
                    .get("provider")
                    .map(String::as_str)
                    .unwrap_or("codex");
                let window = params.get("window").map(String::as_str).unwrap_or("today");
                let summary = summary(provider, window, verbose)?;
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
                respond_json(request, trend(provider, window, verbose)?)
            }
            _ => {
                let response = Response::from_string("not found").with_status_code(StatusCode(404));
                request.respond(response)?;
                Ok(())
            }
        }
    })();
    if verbose {
        server_log!(
            "request.complete path={path} result={} duration_ms={}",
            if result.is_ok() { "ok" } else { "error" },
            started.elapsed().as_secs_f64() * 1000.0,
        );
    }
    result
}

fn providers(verbose: bool) -> Vec<ProviderStatus> {
    PROVIDERS
        .iter()
        .map(|name| ProviderStatus {
            name: (*name).to_owned(),
            available: backend_for(name, verbose).is_ok(),
        })
        .collect()
}

fn summary(provider: &str, window: &str, verbose: bool) -> Result<crate::storage::UsageSummary> {
    let (start, end) = window_dates(window)?;
    let from = crate::local_midnight_utc(start);
    let to = crate::local_midnight_utc(end + Duration::days(1));
    if verbose {
        server_log!("query.summary.start provider={provider} window={window} from={from} to={to}");
    }
    let mut store = backend_for(provider, verbose)
        .with_context(|| format!("no initialized storage for provider {provider}"))?;
    let query_started = Instant::now();
    let result = store.quick_summary_for_agent(agent_name(provider), from, to);
    if verbose {
        server_log!(
            "query.summary.complete provider={provider} duration_ms={}",
            query_started.elapsed().as_secs_f64() * 1000.0
        );
    }
    result
}

fn trend(provider: &str, window: &str, verbose: bool) -> Result<Vec<TrendPoint>> {
    let (mut start, end) = window_dates(window)?;
    // A useful chart for "all time" should remain readable and inexpensive.
    // The summary endpoint still supports the complete all-time range.
    if window == "all" || window == "all_time" {
        start = end - Duration::days(89);
    }
    let from = crate::local_midnight_utc(start);
    let to = crate::local_midnight_utc(end + Duration::days(1));
    if verbose {
        server_log!("query.trend.start provider={provider} window={window} from={from} to={to}");
    }
    let mut store = backend_for(provider, verbose)
        .with_context(|| format!("no initialized storage for provider {provider}"))?;
    let query_started = Instant::now();
    let daily_points = store.daily_trend_for_agent(agent_name(provider), from, to)?;
    let data_days = daily_points.len();
    if verbose {
        server_log!(
            "query.trend.complete provider={provider} data_days={data_days} duration_ms={}",
            query_started.elapsed().as_secs_f64() * 1000.0
        );
    }
    let points_by_date = daily_points
        .into_iter()
        .map(|point| {
            (
                point.date,
                TrendPoint {
                    date: point.date,
                    total_tokens: point.total_tokens,
                    input_tokens: point.input_tokens,
                    output_tokens: point.output_tokens,
                    cache_read_tokens: point.cache_read_tokens,
                    models: point.models,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut points = Vec::new();
    let mut day = start;
    while day <= end {
        points.push(points_by_date.get(&day).cloned().unwrap_or(TrendPoint {
            date: day,
            total_tokens: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            models: BTreeMap::new(),
        }));
        day += Duration::days(1);
    }
    if verbose {
        server_log!(
            "trend.render provider={provider} rendered_days={}",
            points.len()
        );
    }
    Ok(points)
}

fn backend_for(provider: &str, verbose: bool) -> Result<crate::storage::Backend> {
    let mode = crate::storage::prepare_backend_for_agent(false, provider)?;
    if verbose {
        match mode {
            crate::storage::BackendMode::Sqlite => server_log!(
                "backend.open provider={provider} backend=SQLite access=read_only path={}",
                crate::config::agent_db_path(provider)?.display()
            ),
            crate::storage::BackendMode::Postgres => {
                server_log!("backend.open provider={provider} backend=PostgreSQL access=read_only")
            }
        }
    }
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
