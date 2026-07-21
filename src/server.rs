use anyhow::{Context, Result};
use chrono::{Duration, Local, NaiveDate};
use serde::Serialize;
use std::{process::Command, thread};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const PROVIDERS: [&str; 4] = ["codex", "claude_code", "opencode", "copilot"];

#[derive(Debug, Serialize)]
struct ProviderStatus {
    name: String,
    available: bool,
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
    let response = Response::from_string(INDEX_HTML).with_header(content_type("text/html"));
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

const INDEX_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Agentusage</title><style>
body{margin:0;background:#181923;color:#ddd;font:15px ui-monospace,SFMono-Regular,Menlo,monospace}header{padding:18px 24px;border-bottom:1px solid #454557;display:flex;justify-content:space-between}main{padding:18px;display:grid;grid-template-columns:repeat(auto-fit,minmax(340px,1fr));gap:16px}.card{border:1px solid #555568;border-radius:6px;padding:16px;background:#1e1f2b}.card h2{margin:0 0 12px;color:#e9b4c9}.muted{color:#8b8b9e}.metric{display:flex;justify-content:space-between;padding:5px 0}.controls{display:flex;gap:8px}button{background:#292a3a;color:#eee;border:1px solid #666;border-radius:4px;padding:6px 10px}pre{white-space:pre-wrap;color:#cfcfe3}
</style></head><body><header><strong>● Agentusage</strong><span class="controls"><button data-window="today">Today</button><button data-window="7d">7 Days</button><button data-window="30d">30 Days</button><button data-window="all">All Time</button></span></header><main id="app"><p class="muted">Loading provider data…</p></main><script>
let windowName='today';const app=document.querySelector('#app');document.querySelectorAll('button').forEach(b=>b.onclick=()=>{windowName=b.dataset.window;load()});
async function load(){const providers=await fetch('/api/providers').then(r=>r.json());app.innerHTML='';for(const p of providers){const card=document.createElement('section');card.className='card';if(!p.available){card.innerHTML='<h2>'+p.name+'</h2><p class="muted">Unavailable</p>';app.append(card);continue}const s=await fetch('/api/summary?provider='+encodeURIComponent(p.name)+'&window='+windowName).then(r=>r.json());card.innerHTML='<h2>'+p.name+'</h2><div class="metric"><span>tokens</span><b>'+s.total_tokens+'</b></div><div class="metric"><span>requests</span><b>'+s.requests+'</b></div><div class="metric"><span>sessions</span><b>'+s.sessions+'</b></div><div class="metric"><span>cost</span><b>$'+Number(s.cost_usd).toFixed(6)+'</b></div><pre>'+JSON.stringify({models:s.models,clients:s.clients,projects:s.projects,tools:s.tools,languages:s.languages},null,2)+'</pre>';app.append(card)}}load();
</script></body></html>"#;

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
