#![allow(dead_code)]

mod config;
mod core;
mod providers;
mod server;
mod storage;
mod telemetry;
mod tui;

use anyhow::{Context, Result};
use chrono::{Datelike, Local, NaiveDate, TimeZone, Utc};
use clap::{CommandFactory, Parser, Subcommand};
use std::io::{self, Read};

#[derive(Debug, Parser)]
#[command(
    name = "agentusage",
    version,
    about = "Track AI coding agent usage locally"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Dashboard,
    Server {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 8787)]
        port: u16,
        #[arg(long)]
        open: bool,
    },
    #[command(name = "command")]
    Report {
        #[command(subcommand)]
        command: ReportCommand,
    },
    Telemetry {
        #[command(subcommand)]
        command: TelemetryCommand,
    },
    Daily {
        #[arg(long, default_value = "codex")]
        provider: String,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        sessions_dir: Option<String>,
    },
    Weekly {
        #[arg(long, default_value = "codex")]
        provider: String,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        sessions_dir: Option<String>,
    },
    Monthly {
        #[arg(long, default_value = "codex")]
        provider: String,
        #[arg(long)]
        month: Option<String>,
        #[arg(long)]
        sessions_dir: Option<String>,
    },
    Yearly {
        #[arg(long, default_value = "codex")]
        provider: String,
        #[arg(long)]
        year: Option<i32>,
        #[arg(long)]
        sessions_dir: Option<String>,
    },
    Range {
        #[arg(long, default_value = "codex")]
        provider: String,
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        sessions_dir: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum ReportCommand {
    Daily {
        #[arg(long, default_value = "codex")]
        provider: String,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        sessions_dir: Option<String>,
    },
    Weekly {
        #[arg(long, default_value = "codex")]
        provider: String,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        sessions_dir: Option<String>,
    },
    Monthly {
        #[arg(long, default_value = "codex")]
        provider: String,
        #[arg(long)]
        month: Option<String>,
        #[arg(long)]
        sessions_dir: Option<String>,
    },
    Yearly {
        #[arg(long, default_value = "codex")]
        provider: String,
        #[arg(long)]
        year: Option<i32>,
        #[arg(long)]
        sessions_dir: Option<String>,
    },
    Range {
        #[arg(long, default_value = "codex")]
        provider: String,
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        sessions_dir: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum TelemetryCommand {
    Hook {
        source: String,
        payload: Option<String>,
        #[arg(long, default_value = "")]
        account_id: String,
        #[arg(long)]
        db_path: Option<String>,
        #[arg(long)]
        spool_only: bool,
        #[arg(long)]
        verbose: bool,
    },
    Daemon {
        #[arg(long)]
        db_path: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => {
            Cli::command().print_help()?;
            println!();
            Ok(())
        }
        Some(Command::Dashboard) => tui::run(),
        Some(Command::Server { host, port, open }) => server::run(&host, port, open),
        Some(Command::Report { command }) => run_report_command(command),
        Some(Command::Telemetry { command }) => match command {
            TelemetryCommand::Hook {
                source,
                payload,
                account_id,
                db_path,
                spool_only,
                verbose,
            } => run_hook(source, payload, account_id, db_path, spool_only, verbose),
            TelemetryCommand::Daemon { db_path } => {
                let path = db_path
                    .map(std::path::PathBuf::from)
                    .map(Ok)
                    .unwrap_or_else(config::default_db_path)?;
                let store = telemetry::Store::open(&path)?;
                println!("telemetry daemon ready db={}", path.display());
                store.keep_alive();
                Ok(())
            }
        },
        Some(Command::Daily {
            provider,
            date,
            sessions_dir,
        }) => {
            let backend = prepare_report_backend(&provider)?;
            validate_provider(&provider)?;
            let target = parse_date_or_today(date.as_deref())?;
            let report =
                report_for_period(&provider, target, target, sessions_dir.as_deref(), backend)?;
            print_report(&report);
            Ok(())
        }
        Some(Command::Weekly {
            provider,
            date,
            sessions_dir,
        }) => {
            let backend = prepare_report_backend(&provider)?;
            validate_provider(&provider)?;
            let anchor = parse_date_or_today(date.as_deref())?;
            let start =
                anchor - chrono::Duration::days(anchor.weekday().num_days_from_monday() as i64);
            print_report(&report_for_period(
                &provider,
                start,
                start + chrono::Duration::days(6),
                sessions_dir.as_deref(),
                backend,
            )?);
            Ok(())
        }
        Some(Command::Monthly {
            provider,
            month,
            sessions_dir,
        }) => {
            let backend = prepare_report_backend(&provider)?;
            validate_provider(&provider)?;
            let value = month.unwrap_or_else(|| Local::now().format("%Y-%m").to_string());
            let start = NaiveDate::parse_from_str(&format!("{value}-01"), "%Y-%m-%d")
                .with_context(|| format!("invalid month {value:?}; expected YYYY-MM"))?;
            let next = if start.month() == 12 {
                NaiveDate::from_ymd_opt(start.year() + 1, 1, 1).unwrap()
            } else {
                NaiveDate::from_ymd_opt(start.year(), start.month() + 1, 1).unwrap()
            };
            print_report(&report_for_period(
                &provider,
                start,
                next - chrono::Duration::days(1),
                sessions_dir.as_deref(),
                backend,
            )?);
            Ok(())
        }
        Some(Command::Yearly {
            provider,
            year,
            sessions_dir,
        }) => {
            let backend = prepare_report_backend(&provider)?;
            validate_provider(&provider)?;
            let year = year.unwrap_or_else(|| Local::now().year());
            let start = NaiveDate::from_ymd_opt(year, 1, 1).context("invalid year")?;
            print_report(&report_for_period(
                &provider,
                start,
                NaiveDate::from_ymd_opt(year, 12, 31).unwrap(),
                sessions_dir.as_deref(),
                backend,
            )?);
            Ok(())
        }
        Some(Command::Range {
            provider,
            from,
            to,
            sessions_dir,
        }) => {
            let backend = prepare_report_backend(&provider)?;
            validate_provider(&provider)?;
            let start = NaiveDate::parse_from_str(&from, "%Y-%m-%d")
                .with_context(|| format!("invalid --from {from:?}"))?;
            let end = NaiveDate::parse_from_str(&to, "%Y-%m-%d")
                .with_context(|| format!("invalid --to {to:?}"))?;
            print_report(&report_for_period(
                &provider,
                start,
                end,
                sessions_dir.as_deref(),
                backend,
            )?);
            Ok(())
        }
    }
}

fn parse_date_or_today(value: Option<&str>) -> Result<NaiveDate> {
    value
        .map(|value| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .with_context(|| format!("invalid date {value:?}; expected YYYY-MM-DD"))
        })
        .unwrap_or_else(|| Ok(Local::now().date_naive()))
}

fn run_report_command(command: ReportCommand) -> Result<()> {
    match command {
        ReportCommand::Daily {
            provider,
            date,
            sessions_dir,
        } => {
            let backend = prepare_report_backend(&provider)?;
            validate_provider(&provider)?;
            let target = parse_date_or_today(date.as_deref())?;
            print_report(&report_for_period(
                &provider,
                target,
                target,
                sessions_dir.as_deref(),
                backend,
            )?);
        }
        ReportCommand::Weekly {
            provider,
            date,
            sessions_dir,
        } => {
            let backend = prepare_report_backend(&provider)?;
            validate_provider(&provider)?;
            let anchor = parse_date_or_today(date.as_deref())?;
            let start =
                anchor - chrono::Duration::days(anchor.weekday().num_days_from_monday() as i64);
            print_report(&report_for_period(
                &provider,
                start,
                start + chrono::Duration::days(6),
                sessions_dir.as_deref(),
                backend,
            )?);
        }
        ReportCommand::Monthly {
            provider,
            month,
            sessions_dir,
        } => {
            let backend = prepare_report_backend(&provider)?;
            validate_provider(&provider)?;
            let value = month.unwrap_or_else(|| Local::now().format("%Y-%m").to_string());
            let start = NaiveDate::parse_from_str(&format!("{value}-01"), "%Y-%m-%d")
                .with_context(|| format!("invalid month {value:?}; expected YYYY-MM"))?;
            let next = if start.month() == 12 {
                NaiveDate::from_ymd_opt(start.year() + 1, 1, 1).unwrap()
            } else {
                NaiveDate::from_ymd_opt(start.year(), start.month() + 1, 1).unwrap()
            };
            print_report(&report_for_period(
                &provider,
                start,
                next - chrono::Duration::days(1),
                sessions_dir.as_deref(),
                backend,
            )?);
        }
        ReportCommand::Yearly {
            provider,
            year,
            sessions_dir,
        } => {
            let backend = prepare_report_backend(&provider)?;
            validate_provider(&provider)?;
            let year = year.unwrap_or_else(|| Local::now().year());
            let start = NaiveDate::from_ymd_opt(year, 1, 1).context("invalid year")?;
            print_report(&report_for_period(
                &provider,
                start,
                NaiveDate::from_ymd_opt(year, 12, 31).unwrap(),
                sessions_dir.as_deref(),
                backend,
            )?);
        }
        ReportCommand::Range {
            provider,
            from,
            to,
            sessions_dir,
        } => {
            let backend = prepare_report_backend(&provider)?;
            validate_provider(&provider)?;
            let start = NaiveDate::parse_from_str(&from, "%Y-%m-%d")
                .with_context(|| format!("invalid --from {from:?}"))?;
            let end = NaiveDate::parse_from_str(&to, "%Y-%m-%d")
                .with_context(|| format!("invalid --to {to:?}"))?;
            print_report(&report_for_period(
                &provider,
                start,
                end,
                sessions_dir.as_deref(),
                backend,
            )?);
        }
    }
    Ok(())
}

fn prepare_report_backend(provider: &str) -> Result<storage::BackendMode> {
    let backend = storage::prepare_backend_for_agent(true, provider)?;
    Ok(backend)
}

fn report_for_period(
    provider: &str,
    start: NaiveDate,
    end: NaiveDate,
    sessions_dir: Option<&str>,
    backend: storage::BackendMode,
) -> Result<providers::codex::DailyUsage> {
    if provider != "codex" && backend == storage::BackendMode::Jsonl {
        anyhow::bail!("{provider} reports require SQLite or PostgreSQL storage");
    }
    if backend == storage::BackendMode::Jsonl {
        return providers::codex::usage_between(start, end, sessions_dir);
    }

    let mut store = storage::Backend::open_for_agent(backend, provider)?;
    let (files_scanned, files_with_usage, token_records, malformed_lines) = match provider {
        "codex" => {
            let s = providers::codex::ingest_into_store(sessions_dir, &mut store)?;
            (
                s.files_scanned,
                s.files_with_usage,
                s.token_records,
                s.malformed_lines,
            )
        }
        "claude" | "claude_code" => providers::local::ingest_into_store(
            providers::local::Agent::ClaudeCode,
            sessions_dir,
            &mut store,
        )?,
        "opencode" | "open_code" => providers::local::ingest_into_store(
            providers::local::Agent::OpenCode,
            sessions_dir,
            &mut store,
        )?,
        "copilot" => {
            let s = providers::copilot::ingest_into_store(None, &mut store)?;
            (
                s.files_scanned,
                s.files_with_usage,
                s.token_records,
                s.malformed_lines,
            )
        }
        _ => anyhow::bail!("unsupported provider {provider}"),
    };
    let from = local_midnight_utc(start);
    let to = local_midnight_utc(end + chrono::Duration::days(1));
    let summary = storage::UsageStore::summary_for_agent(
        &mut store,
        Some(agent_name_for_report(provider)),
        from,
        to,
    )?;
    Ok(providers::codex::DailyUsage {
        provider: provider.to_owned(),
        date: start,
        end_date: end,
        sessions: summary.sessions as usize,
        requests: summary.requests as usize,
        prompts: summary.prompts as usize,
        input_tokens: summary.input_tokens,
        output_tokens: summary.output_tokens,
        reasoning_tokens: summary.reasoning_tokens,
        cached_input_tokens: summary.cache_read_tokens,
        cache_write_tokens: summary.cache_write_tokens,
        total_tokens: summary.total_tokens,
        cost_usd: summary.cost_usd,
        ai_units_nano: summary.ai_units_nano,
        ai_credits: summary.ai_credits,
        lines_added: summary.lines_added,
        lines_removed: summary.lines_removed,
        tools: summary
            .tools
            .into_iter()
            .map(|(name, count)| (name, count as usize))
            .collect(),
        languages: summary
            .languages
            .into_iter()
            .map(|(name, count)| (name, count as usize))
            .collect(),
        models: summary
            .models
            .into_iter()
            .map(|(name, value)| {
                (
                    name,
                    providers::codex::TokenBreakdown {
                        requests: value.requests,
                        input: value.input_tokens,
                        output: value.output_tokens,
                        reasoning: value.reasoning_tokens,
                        cache_read: value.cache_read_tokens,
                        cache_write: value.cache_write_tokens,
                        total: value.total_tokens,
                        cost_usd: value.cost_usd,
                        ai_units_nano: value.ai_units_nano,
                        request_multiplier: value.request_multiplier,
                        ai_credits: value.ai_credits,
                    },
                )
            })
            .collect(),
        clients: summary
            .clients
            .into_iter()
            .map(|(name, value)| {
                let name = if provider == "copilot" && name == "user" {
                    "CLI".to_owned()
                } else {
                    name
                };
                (
                    name,
                    providers::codex::TokenBreakdown {
                        requests: value.requests,
                        input: value.input_tokens,
                        output: value.output_tokens,
                        reasoning: value.reasoning_tokens,
                        cache_read: value.cache_read_tokens,
                        cache_write: value.cache_write_tokens,
                        total: value.total_tokens,
                        cost_usd: value.cost_usd,
                        ai_units_nano: value.ai_units_nano,
                        request_multiplier: value.request_multiplier,
                        ai_credits: value.ai_credits,
                    },
                )
            })
            .collect(),
        projects: summary
            .projects
            .into_iter()
            .map(|(name, value)| {
                (
                    name,
                    providers::codex::TokenBreakdown {
                        requests: value.requests,
                        input: value.input_tokens,
                        output: value.output_tokens,
                        reasoning: value.reasoning_tokens,
                        cache_read: value.cache_read_tokens,
                        cache_write: value.cache_write_tokens,
                        total: value.total_tokens,
                        cost_usd: value.cost_usd,
                        ai_units_nano: value.ai_units_nano,
                        request_multiplier: value.request_multiplier,
                        ai_credits: value.ai_credits,
                    },
                )
            })
            .collect(),
        files_scanned,
        files_with_usage,
        token_records,
        malformed_lines,
        desktop_usage: (provider == "claude" || provider == "claude_code")
            .then(providers::local::desktop_usage)
            .flatten(),
    })
}

fn local_midnight_utc(date: NaiveDate) -> chrono::DateTime<Utc> {
    Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).expect("valid midnight"))
        .single()
        .expect("local midnight must be unambiguous")
        .with_timezone(&Utc)
}

fn validate_provider(provider: &str) -> Result<()> {
    match provider {
        "codex" | "claude" | "claude_code" | "opencode" | "open_code" | "copilot" => Ok(()),
        other => {
            anyhow::bail!(
                "unsupported provider {other:?}; supported: codex, claude_code, opencode, copilot"
            )
        }
    }
}

fn agent_name_for_report(provider: &str) -> &str {
    match provider {
        "claude" | "claude_code" => "claude_code",
        "opencode" | "open_code" => "opencode",
        "copilot" => "copilot",
        _ => "codex",
    }
}

fn cache_hit_rate(report: &providers::codex::DailyUsage) -> Option<f64> {
    let denominator = report.input_tokens + report.cached_input_tokens + report.cache_write_tokens;
    (denominator > 0 && report.cached_input_tokens > 0)
        .then(|| report.cached_input_tokens as f64 / denominator as f64 * 100.0)
}

fn print_report(report: &providers::codex::DailyUsage) {
    eprintln!(
        "[agentusage] {} files_scanned={} files_with_usage={} token_records={} malformed_lines={} sessions={}",
        report.provider,
        report.files_scanned,
        report.files_with_usage,
        report.token_records,
        report.malformed_lines,
        report.sessions
    );
    println!(
        "period               {} to {}\nprovider             {}\nsessions             {}\nrequests             {}\nprompts              {}\ninput tokens         {}\noutput tokens        {}\nreasoning tokens     {}\ncached input tokens  {}\ncache write tokens   {}\ntotal tokens         {}\nestimated cost       ${:.6}\ncache hit rate       {}\nlines added          {}\nlines removed        {}",
        report.date,
        report.end_date,
        report.provider,
        report.sessions,
        report.requests,
        report.prompts,
        report.input_tokens,
        report.output_tokens,
        report.reasoning_tokens,
        report.cached_input_tokens,
        report.cache_write_tokens,
        report.total_tokens,
        report.cost_usd,
        cache_hit_rate(report)
            .map(|v| format!("{v:.2}%"))
            .unwrap_or_else(|| "n/a".into()),
        report.lines_added,
        report.lines_removed
    );
    println!(
        "ai credits           {:.6}\nai units (nano)      {}",
        report.ai_credits, report.ai_units_nano
    );
    if let Some(desktop) = &report.desktop_usage {
        println!(
            "desktop samples      {}\ndesktop 5h signal     {}\ndesktop 7d signal     {}",
            desktop.samples, desktop.five_hour_signal, desktop.seven_day_signal
        );
        eprintln!(
            "[agentusage] claude_desktop samples={} latest_timestamp_ms={} five_hour_signal={} seven_day_signal={}",
            desktop.samples,
            desktop.latest_timestamp_ms,
            desktop.five_hour_signal,
            desktop.seven_day_signal
        );
    }
    println!("\nmodel breakdown:");
    for (name, usage) in &report.models {
        println!(
            "  {name}: requests={} in={} out={} cache_read={} cache_write={} reason={} total={} cost=${:.6} ai_credits={:.6} multiplier={:.4}",
            usage.requests,
            usage.input,
            usage.output,
            usage.cache_read,
            usage.cache_write,
            usage.reasoning,
            usage.total,
            usage.cost_usd,
            usage.ai_credits,
            usage.request_multiplier
        );
    }
    println!("\nclient breakdown:");
    for (name, usage) in &report.clients {
        println!(
            "  {name}: requests={} total_tokens={} cost=${:.6} ai_credits={:.6}",
            usage.requests, usage.total, usage.cost_usd, usage.ai_credits
        );
    }
    println!("\nproject breakdown:");
    for (name, usage) in &report.projects {
        println!(
            "  {name}: requests={} total_tokens={} cost=${:.6}",
            usage.requests, usage.total, usage.cost_usd
        );
    }
}

fn run_hook(
    source: String,
    payload: Option<String>,
    account_id: String,
    db_path: Option<String>,
    spool_only: bool,
    verbose: bool,
) -> Result<()> {
    let raw = match payload {
        Some(payload) if !payload.trim().is_empty() => payload.into_bytes(),
        _ => {
            let mut input = Vec::new();
            io::stdin()
                .read_to_end(&mut input)
                .context("read hook payload from stdin")?;
            input
        }
    };
    if raw.iter().all(u8::is_ascii_whitespace) {
        anyhow::bail!("hook payload is empty");
    }

    let request = providers::parse_hook(&source, &raw, account_id)?;
    let db = db_path
        .map(std::path::PathBuf::from)
        .map(Ok)
        .unwrap_or_else(config::default_db_path)?;
    let store = telemetry::Store::open(&db)?;
    let result = if spool_only {
        store.write_spool(&request)?
    } else {
        store.ingest(request)?
    };

    if verbose {
        println!(
            "telemetry hook {} status={} deduped={} event_id={}",
            source, result.status, result.deduped, result.event_id
        );
    }
    Ok(())
}
