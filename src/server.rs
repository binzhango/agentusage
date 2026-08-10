use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Local, NaiveDate, Utc};
use percent_encoding::percent_decode_str;
use serde::Serialize;
use std::{collections::BTreeMap, net::IpAddr, process::Command, thread, time::Instant};
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

#[derive(Debug, Serialize)]
struct EventPage {
    events: Vec<crate::storage::UsageEventDetail>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
struct PromptPage {
    prompts: Vec<crate::storage::PromptDetail>,
    next_cursor: Option<String>,
}

#[derive(Debug)]
struct ApiError {
    status: u16,
    code: &'static str,
    message: String,
}

type ApiResult<T> = std::result::Result<T, ApiError>;

#[derive(Serialize)]
struct ApiErrorEnvelope<'a> {
    error: ApiErrorBody<'a>,
}

#[derive(Serialize)]
struct ApiErrorBody<'a> {
    code: &'a str,
    message: &'a str,
}

pub fn run(host: &str, port: u16, open: bool, verbose: bool) -> Result<()> {
    if !is_loopback_host(host) {
        anyhow::bail!(
            "the local dashboard may only bind to loopback; use 127.0.0.1, localhost, or ::1"
        );
    }
    let address = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
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

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
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
            (&Method::Get, path) if path.starts_with("/provider/") => respond_html(request),
            (&Method::Get, "/api/providers") => respond_json(request, providers(verbose)),
            (&Method::Get, "/api/summary") => {
                let query = url.split_once('?').map(|(_, value)| value).unwrap_or("");
                let params = query_params(query);
                let provider = params
                    .get("provider")
                    .map(String::as_str)
                    .unwrap_or("codex");
                let window = params.get("window").map(String::as_str).unwrap_or("today");
                respond_api(request, api_summary(provider, window, verbose))
            }
            (&Method::Get, "/api/trend") => {
                let query = url.split_once('?').map(|(_, value)| value).unwrap_or("");
                let params = query_params(query);
                let provider = params
                    .get("provider")
                    .map(String::as_str)
                    .unwrap_or("codex");
                let window = params.get("window").map(String::as_str).unwrap_or("today");
                respond_api(request, api_trend(provider, window, verbose))
            }
            (&Method::Get, "/api/events") => {
                let query = url.split_once('?').map(|(_, value)| value).unwrap_or("");
                let params = query_params(query);
                let provider = params
                    .get("provider")
                    .map(String::as_str)
                    .unwrap_or("codex");
                let window = params.get("window").map(String::as_str).unwrap_or("today");
                respond_api(request, events(provider, window, &params, verbose))
            }
            (&Method::Get, path) if path.starts_with("/api/events/") => {
                let query = url.split_once('?').map(|(_, value)| value).unwrap_or("");
                let params = query_params(query);
                let provider = params
                    .get("provider")
                    .map(String::as_str)
                    .unwrap_or("codex");
                let event_id =
                    percent_decode_str(path.trim_start_matches("/api/events/")).decode_utf8_lossy();
                respond_api(request, event(provider, &event_id, verbose))
            }
            (&Method::Get, "/api/prompts") => {
                let query = url.split_once('?').map(|(_, value)| value).unwrap_or("");
                let params = query_params(query);
                let provider = params
                    .get("provider")
                    .map(String::as_str)
                    .unwrap_or("codex");
                let window = params.get("window").map(String::as_str).unwrap_or("today");
                respond_api(request, prompts(provider, window, &params, verbose))
            }
            (&Method::Get, path) if path.starts_with("/api/prompts/") => {
                let query = url.split_once('?').map(|(_, value)| value).unwrap_or("");
                let params = query_params(query);
                let provider = params
                    .get("provider")
                    .map(String::as_str)
                    .unwrap_or("codex");
                let prompt_id = percent_decode_str(path.trim_start_matches("/api/prompts/"))
                    .decode_utf8_lossy();
                respond_api(request, prompt(provider, &prompt_id, verbose))
            }
            _ => {
                if path.starts_with("/api/") {
                    respond_api::<serde_json::Value>(
                        request,
                        Err(ApiError {
                            status: 404,
                            code: "not_found",
                            message: "API route not found".into(),
                        }),
                    )
                } else {
                    let response =
                        Response::from_string("not found").with_status_code(StatusCode(404));
                    request.respond(response)?;
                    Ok(())
                }
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

fn api_summary(
    provider: &str,
    window: &str,
    verbose: bool,
) -> ApiResult<crate::storage::UsageSummary> {
    validate_api_provider(provider)?;
    validate_api_window(window)?;
    summary(provider, window, verbose).map_err(|error| storage_error(provider, error, verbose))
}

fn api_trend(provider: &str, window: &str, verbose: bool) -> ApiResult<Vec<TrendPoint>> {
    validate_api_provider(provider)?;
    validate_api_window(window)?;
    trend(provider, window, verbose).map_err(|error| storage_error(provider, error, verbose))
}

fn events(
    provider: &str,
    window: &str,
    params: &std::collections::BTreeMap<String, String>,
    verbose: bool,
) -> ApiResult<EventPage> {
    validate_api_provider(provider)?;
    validate_api_window(window)?;
    let (start, end) = window_dates(window).map_err(invalid_window)?;
    let from = crate::local_midnight_utc(start);
    let to = crate::local_midnight_utc(end + Duration::days(1));
    let limit = page_limit(params)?;
    let before = params
        .get("cursor")
        .map(|cursor| parse_event_cursor(cursor))
        .transpose()?;
    let query = crate::storage::UsageEventQuery {
        from,
        to,
        before,
        limit,
        model: params
            .get("model")
            .filter(|value| !value.is_empty())
            .cloned(),
        session_id: params
            .get("session")
            .filter(|value| !value.is_empty())
            .cloned(),
        status: params
            .get("status")
            .filter(|value| !value.is_empty())
            .cloned(),
    };
    let mut store =
        backend_for(provider, verbose).map_err(|error| storage_error(provider, error, verbose))?;
    let events = store
        .usage_events(agent_name(provider), &query)
        .map_err(|error| query_error(provider, error, verbose))?;
    let next_cursor = (events.len() == limit).then(|| {
        let last = events.last().expect("non-empty page at limit");
        format!(
            "{}|{}",
            last.usage.occurred_at.to_rfc3339(),
            last.usage.event_id
        )
    });
    Ok(EventPage {
        events,
        next_cursor,
    })
}

fn prompts(
    provider: &str,
    window: &str,
    params: &std::collections::BTreeMap<String, String>,
    verbose: bool,
) -> ApiResult<PromptPage> {
    validate_api_provider(provider)?;
    validate_api_window(window)?;
    let (start, end) = window_dates(window).map_err(invalid_window)?;
    let from = crate::local_midnight_utc(start);
    let to = crate::local_midnight_utc(end + Duration::days(1));
    let limit = page_limit(params)?;
    let before = params
        .get("cursor")
        .map(|cursor| parse_event_cursor(cursor))
        .transpose()?;
    let query = crate::storage::PromptQuery {
        from,
        to,
        before,
        limit,
        session_id: params
            .get("session")
            .filter(|value| !value.is_empty())
            .cloned(),
        search: params
            .get("search")
            .filter(|value| !value.trim().is_empty())
            .cloned(),
    };
    let mut store =
        backend_for(provider, verbose).map_err(|error| storage_error(provider, error, verbose))?;
    let prompts = store
        .prompts(agent_name(provider), &query)
        .map_err(|error| query_error(provider, error, verbose))?;
    let next_cursor = (prompts.len() == limit).then(|| {
        let last = prompts.last().expect("non-empty prompt page at limit");
        format!(
            "{}|{}",
            last.usage.occurred_at.to_rfc3339(),
            last.usage.event_id
        )
    });
    Ok(PromptPage {
        prompts,
        next_cursor,
    })
}

fn prompt(
    provider: &str,
    prompt_id: &str,
    verbose: bool,
) -> ApiResult<crate::storage::PromptDetail> {
    validate_api_provider(provider)?;
    if prompt_id.trim().is_empty() {
        return Err(ApiError {
            status: 400,
            code: "invalid_prompt_id",
            message: "prompt id is required".into(),
        });
    }
    let mut store =
        backend_for(provider, verbose).map_err(|error| storage_error(provider, error, verbose))?;
    let prompt = store
        .prompt(prompt_id)
        .map_err(|error| query_error(provider, error, verbose))?;
    match prompt {
        Some(prompt) if prompt.usage.agent_name == agent_name(provider) => Ok(prompt),
        _ => Err(ApiError {
            status: 404,
            code: "prompt_not_found",
            message: "prompt not found".into(),
        }),
    }
}

fn page_limit(params: &std::collections::BTreeMap<String, String>) -> ApiResult<usize> {
    let limit = params
        .get("limit")
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|_| ApiError {
            status: 400,
            code: "invalid_limit",
            message: "limit must be an integer between 1 and 200".into(),
        })?
        .unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(ApiError {
            status: 400,
            code: "invalid_limit",
            message: "limit must be between 1 and 200".into(),
        });
    }
    Ok(limit)
}

fn event(
    provider: &str,
    event_id: &str,
    verbose: bool,
) -> ApiResult<crate::storage::UsageEventDetail> {
    validate_api_provider(provider)?;
    if event_id.trim().is_empty() {
        return Err(ApiError {
            status: 400,
            code: "invalid_event_id",
            message: "event id is required".into(),
        });
    }
    let mut store =
        backend_for(provider, verbose).map_err(|error| storage_error(provider, error, verbose))?;
    let event = store
        .usage_event(event_id)
        .map_err(|error| query_error(provider, error, verbose))?;
    match event {
        Some(event) if event.usage.agent_name == agent_name(provider) => Ok(event),
        _ => Err(ApiError {
            status: 404,
            code: "event_not_found",
            message: "usage event not found".into(),
        }),
    }
}

fn parse_event_cursor(value: &str) -> ApiResult<crate::storage::UsageEventCursor> {
    let Some((occurred_at, event_id)) = value.split_once('|') else {
        return Err(ApiError {
            status: 400,
            code: "invalid_cursor",
            message: "cursor must contain an RFC 3339 timestamp and event id".into(),
        });
    };
    let occurred_at = DateTime::parse_from_rfc3339(occurred_at)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| ApiError {
            status: 400,
            code: "invalid_cursor",
            message: "cursor timestamp must be RFC 3339".into(),
        })?;
    if event_id.is_empty() {
        return Err(ApiError {
            status: 400,
            code: "invalid_cursor",
            message: "cursor event id is empty".into(),
        });
    }
    Ok(crate::storage::UsageEventCursor {
        occurred_at,
        event_id: event_id.to_owned(),
    })
}

fn validate_api_provider(provider: &str) -> ApiResult<()> {
    if PROVIDERS.contains(&provider) {
        Ok(())
    } else {
        Err(ApiError {
            status: 400,
            code: "invalid_provider",
            message: format!("unsupported provider {provider:?}"),
        })
    }
}

fn validate_api_window(window: &str) -> ApiResult<()> {
    window_dates(window).map(|_| ()).map_err(invalid_window)
}

fn invalid_window(error: anyhow::Error) -> ApiError {
    ApiError {
        status: 400,
        code: "invalid_window",
        message: error.to_string(),
    }
}

fn storage_error(provider: &str, error: anyhow::Error, verbose: bool) -> ApiError {
    if verbose {
        server_log!("api.storage_unavailable provider={provider} error={error:#}");
    }
    ApiError {
        status: 503,
        code: "storage_unavailable",
        message: format!("storage for provider {provider:?} is unavailable"),
    }
}

fn query_error(provider: &str, error: anyhow::Error, verbose: bool) -> ApiError {
    if verbose {
        server_log!("api.query_failed provider={provider} error={error:#}");
    }
    ApiError {
        status: 500,
        code: "query_failed",
        message: "the usage query failed".into(),
    }
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
    let result = store.agent_summary(agent_name(provider), from, to);
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
    if window == "all" {
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
    provider
}

fn window_dates(window: &str) -> Result<(NaiveDate, NaiveDate)> {
    let end = Local::now().date_naive();
    let start = match window {
        "today" => end,
        "7d" => end - Duration::days(6),
        "30d" => end - Duration::days(29),
        "all" => NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
        other => anyhow::bail!("unsupported window {other:?}; use today, 7d, 30d, or all"),
    };
    Ok((start, end))
}

fn query_params(query: &str) -> std::collections::BTreeMap<String, String> {
    query
        .split('&')
        .filter_map(|part| part.split_once('='))
        .map(|(key, value)| {
            let key = percent_decode_str(key).decode_utf8_lossy().into_owned();
            let value = value.replace('+', " ");
            let value = percent_decode_str(&value).decode_utf8_lossy().into_owned();
            (key, value)
        })
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
    request.respond(
        Response::from_string(body)
            .with_header(content_type("application/json"))
            .with_header(no_store()),
    )?;
    Ok(())
}

fn respond_api<T: Serialize>(request: Request, result: ApiResult<T>) -> Result<()> {
    match result {
        Ok(value) => respond_json(request, value),
        Err(error) => {
            let body = serde_json::to_string(&ApiErrorEnvelope {
                error: ApiErrorBody {
                    code: error.code,
                    message: &error.message,
                },
            })?;
            request.respond(
                Response::from_string(body)
                    .with_status_code(StatusCode(error.status))
                    .with_header(content_type("application/json"))
                    .with_header(no_store()),
            )?;
            Ok(())
        }
    }
}

fn content_type(value: &str) -> Header {
    Header::from_bytes("Content-Type", value).expect("static header")
}

fn no_store() -> Header {
    Header::from_bytes("Cache-Control", "no-store").expect("static header")
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
    use super::{
        handle_request, is_loopback_host, page_limit, parse_event_cursor, query_params,
        validate_api_provider, window_dates,
    };
    use std::{
        io::{Read, Write},
        net::TcpStream,
        thread,
    };
    use tiny_http::Server;

    fn http_request(path: &str) -> Option<String> {
        // Some package sandboxes prohibit loopback sockets. The same test runs
        // normally in CI and on developer machines that permit local binds.
        let server = Server::http("127.0.0.1:0").ok()?;
        let address = server.server_addr().to_ip().unwrap();
        let worker = thread::spawn(move || {
            let request = server.recv().unwrap();
            handle_request(request, false).unwrap();
        });
        let mut stream = TcpStream::connect(address).unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        worker.join().unwrap();
        Some(response)
    }

    #[test]
    fn accepts_supported_windows() {
        for window in ["today", "7d", "30d", "all"] {
            assert!(window_dates(window).is_ok());
        }
        assert!(window_dates("bad").is_err());
    }

    #[test]
    fn rejects_unknown_providers_before_opening_storage() {
        let error = validate_api_provider("unknown").unwrap_err();
        assert_eq!(error.status, 400);
        assert_eq!(error.code, "invalid_provider");
    }

    #[test]
    fn decodes_filters_and_validates_event_cursors() {
        let params = query_params("model=gpt-5%20codex&session=a%2Fb&search=parser+tests&limit=25");
        assert_eq!(params["model"], "gpt-5 codex");
        assert_eq!(params["session"], "a/b");
        assert_eq!(params["search"], "parser tests");
        assert_eq!(page_limit(&params).unwrap(), 25);
        let cursor = parse_event_cursor("2026-07-19T12:00:00Z|event-1").unwrap();
        assert_eq!(cursor.event_id, "event-1");
        assert!(parse_event_cursor("invalid").is_err());
        assert!(page_limit(&query_params("limit=201")).is_err());
    }

    #[test]
    fn server_bind_is_restricted_to_loopback() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("localhost"));
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(!is_loopback_host("192.0.2.10"));
    }

    #[test]
    fn api_routes_return_structured_non_cached_errors() {
        let Some(response) = http_request("/api/summary?provider=unknown&window=today") else {
            return;
        };
        assert!(response.starts_with("HTTP/1.1 400"));
        assert!(response.contains("Content-Type: application/json"));
        assert!(response.contains("Cache-Control: no-store"));
        assert!(response.contains(r#""code":"invalid_provider""#));

        let response = http_request("/api/does-not-exist").unwrap();
        assert!(response.starts_with("HTTP/1.1 404"));
        assert!(response.contains(r#""code":"not_found""#));
    }
}
