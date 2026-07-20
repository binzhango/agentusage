# agentusage

[![CI](https://github.com/binzhango/agentusage/actions/workflows/ci.yml/badge.svg)](https://github.com/binzhango/agentusage/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/binzhango/agentusage)](https://github.com/binzhango/agentusage/releases/latest)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Local-first usage reports for AI coding agents.

`agentusage` reads local agent history, normalizes it into SQLite or
PostgreSQL, and reports tokens, cost, models, clients, cache usage, code
changes, and Copilot AI credits. Your usage data stays on your machine unless
you explicitly configure a PostgreSQL server.

> **Project status:** early access. Provider adapters and report fields are
> still expanding. Feedback and fixtures are welcome.

## Roadmap

- [ ] Add Google Antigravity CLI usage support.
- [ ] Expand the local web server into a fully interactive dashboard with richer
  filtering, provider detail views, and responsive usage exploration.
- [x] Add `config.toml` controls for automatic provider-file synchronization.

## Highlights

- Daily, weekly, monthly, yearly, and arbitrary date-range reports.
- Input, output, reasoning, cache-read, cache-write, and total-token breakdowns.
- Model and client attribution, including CLI, IDE, Desktop, and Copilot usage.
- Project/workspace, tool-call, and language breakdowns where provider data is available.
- SQLite by default, with optional PostgreSQL storage.
- Idempotent ingestion, raw-event preservation, and incremental processing.
- Copilot CLI and VS Code Copilot model, token, and AI-credit tracking.
- Telemetry hooks with stdin, positional payload, spool-only, and daemon modes.
- macOS, Linux, and Windows release binaries.

## Architecture

```mermaid
flowchart LR
    subgraph Sources[Provider sources]
        CodexFiles[Codex rollout JSONL]
        ClaudeFiles[Claude Code JSONL]
        OpenCodeFiles[OpenCode JSONL]
        CopilotFiles[Copilot DB / JSONL / logs]
        Hook[Provider hook event]
    end

    subgraph Ingestion[Ingestion and normalization]
        Sync[Incremental sync<br/>cursors + deduplication]
        Adapters[Provider adapters<br/>Codex / Claude / OpenCode / Copilot]
        Telemetry[Telemetry hook parser]
        Normalize[Flexible event normalization<br/>raw payload preserved]
    end

    subgraph Storage[Canonical storage]
        SQLite[(SQLite)]
        Postgres[(PostgreSQL)]
        Tables[(Raw events<br/>Usage events<br/>Metrics<br/>Ingest records<br/>Cursors)]
    end

    subgraph Consumers[Database-backed consumers]
        Commands[CLI reports]
        TUI[Terminal dashboard]
        Server[HTTP dashboard / API]
    end

    Config[config.toml<br/>auto_sync + refresh_seconds]

    CodexFiles --> Sync
    ClaudeFiles --> Sync
    OpenCodeFiles --> Sync
    CopilotFiles --> Sync
    Hook --> Telemetry
    Telemetry --> Normalize
    Sync --> Adapters --> Normalize
    Normalize --> SQLite
    Normalize --> Postgres
    SQLite --> Tables
    Postgres --> Tables
    Tables --> Commands
    Tables --> TUI
    Tables --> Server
    Config -. controls refresh .-> Sync
    Config -. controls background sync .-> TUI
    Config -. controls background sync .-> Server
```

Provider files are read only by ingestion. Commands, the terminal dashboard,
and the server query SQLite/PostgreSQL tables; quota is calculated from the
latest persisted raw provider event, while usage and credits are aggregated
from normalized usage events.

## Installation

If Rust is installed, the simplest option is:

```bash
cargo install --git https://github.com/binzhango/agentusage --locked --bins
agentusage --version
```

This installs both `agentusage` and the `au` alias into Cargo's binary
directory, usually `~/.cargo/bin`. For a local checkout, use
`cargo install --path . --locked --bins`.

Download the archive for your platform from the
[latest GitHub release](https://github.com/binzhango/agentusage/releases/latest),
extract it, and put the `agentusage` binary on your `PATH`.

Example for Apple Silicon macOS:

```bash
curl -fL -o agentusage.tar.gz \
  https://github.com/binzhango/agentusage/releases/latest/download/agentusage-macos-aarch64.tar.gz
tar -xzf agentusage.tar.gz
sudo install -m 0755 agentusage /usr/local/bin/agentusage
sudo install -m 0755 au /usr/local/bin/au
agentusage --version
au --version
```

`au` is an executable alias for `agentusage`; both commands support the same
dashboard, server, report, and telemetry subcommands.

Available release targets:

| Platform | Archive |
| --- | --- |
| macOS Apple Silicon | `agentusage-macos-aarch64.tar.gz` |
| Linux ARM64 | `agentusage-linux-aarch64.tar.gz` |
| Linux x86_64 | `agentusage-linux-x86_64.tar.gz` |
| Windows x86_64 | `agentusage-windows-x86_64.zip` |

Release archives include `SHA256SUMS`. Verify the checksum before installing
in a production environment.

## Quick start

```bash
agentusage dashboard
```

`agentusage dashboard` opens the interactive terminal dashboard. It shows
Codex, Claude Code, OpenCode, and Copilot usage together. Use `w` to cycle
Today, 7 days, 30 days, and all-time windows, `r` to refresh, and `q` to quit.
The dashboard requires an interactive terminal.

Running `agentusage` without a subcommand prints help. The local browser
dashboard is available with:

```bash
agentusage server
agentusage server --open
```

The server listens on `127.0.0.1:8787` by default and serves the dashboard UI
and JSON API without exposing usage data to the network. Use `--host` and
`--port` to change the bind address when needed.

On first use, the CLI checks for an initialized database. If none exists, it
asks whether to initialize SQLite or use PostgreSQL. Reports never read
provider files directly; run `agentusage ingest --provider <provider>` to
import provider data into the selected database first.

## Reports

Detailed reports are available under `agentusage command`. The original period
commands remain supported as compatibility aliases. All report commands accept
`--provider`.

```bash
# Today
agentusage command daily --provider codex
agentusage command daily --provider claude_code
agentusage command daily --provider opencode
agentusage command daily --provider copilot

# Compatibility alias
agentusage daily --provider codex

# Specific date
agentusage daily --provider codex --date 2026-07-19

# Week, month, and year
agentusage weekly --provider codex
agentusage monthly --provider copilot --month 2026-07
agentusage yearly --provider claude_code --year 2026

# Inclusive date range
agentusage range --provider copilot \
  --from 2026-07-01 --to 2026-07-19
```

The server API is available at `/api/providers` and
`/api/summary?provider=codex&window=today`. Supported windows are `today`,
`7d`, `30d`, and `all`.

Reports include:

- requests, prompts, sessions, lines added, and lines removed;
- input, output, reasoning, cache-read, cache-write, and total tokens;
- estimated cost and cache-hit rate when pricing data is available;
- model and client breakdowns;
- project/workspace breakdowns when provider metadata includes a working directory;
- tool-call and language breakdowns for imported provider telemetry;
- Copilot AI credits and native AI-unit values when the source provides them.

Provider files can be imported from an alternate source directory during the
explicit ingestion step:

```bash
agentusage ingest --provider codex --sessions-dir /path/to/codex/sessions
```

## Supported providers

| Provider | Local source | Report details |
| --- | --- | --- |
| `codex` | Codex rollout JSONL | Tokens, models, cache, cost, sessions, and code changes |
| `claude_code` | Claude Code session JSONL | Tokens, models, and sessions |
| `opencode` | OpenCode session JSONL | Tokens, models, sessions, and cost fields |
| `copilot` | Copilot CLI databases and VS Code chat JSONL/logs | CLI/IDE attribution, tokens, models, and AI credits |

For VS Code Copilot, an entry such as `MAI-Code-1-Flash • 1.6 credits` is
reported with its resolved model and exact `copilotCredits` value when that
metadata is present locally.

## Storage and configuration

Provider reports use separate SQLite databases by default. Commands, the
dashboard, and the server read metrics and status only from these tables; local
provider files are read only by ingestion:

```text
~/.local/state/agentusage/codex.db
~/.local/state/agentusage/claude_code.db
~/.local/state/agentusage/opencode.db
~/.local/state/agentusage/copilot.db
```

Telemetry hooks and the daemon use:

```text
~/.local/state/agentusage/telemetry.db
```

Set `XDG_STATE_HOME` to change the state directory:

```bash
XDG_STATE_HOME="$HOME/.local/state" agentusage daily --provider codex
```

## Automatic synchronization

Automatic sync is configured in `~/.config/agentusage/config.toml` (or the path
set by `AGENTUSAGE_CONFIG`):

```toml
[sync]
auto_sync = true
refresh_seconds = 300
```

The dashboard and server ingest provider files at this interval. Report
commands perform one incremental sync before querying the database, and the
provider hook performs the same sync after an agent event. Set
`refresh_seconds` to the interval you prefer.

PostgreSQL is available through `AGENTUSAGE_POSTGRES_URL`. When no initialized
provider SQLite database exists, the first-run prompt can select PostgreSQL:

```bash
export AGENTUSAGE_POSTGRES_URL='postgresql://user:password@localhost/agentusage'
agentusage monthly --provider copilot
```

The project does not use an `OPENUSAGE_*` database variable.

## Telemetry hooks and daemon

Pass a hook payload positionally:

```bash
agentusage telemetry hook codex \
  '{"turn_id":"turn-1","usage":{"input_tokens":10,"output_tokens":4}}' \
  --verbose
```

Or pipe the payload through stdin:

```bash
printf '%s' '{"event":"message","usage":{"input_tokens":10}}' \
  | agentusage telemetry hook claude_code --verbose
```

Supported hook sources are `codex`, `claude_code`, and `opencode`. Useful
options include:

- `--account-id` to preserve an account dimension;
- `--db-path` to use a custom telemetry database;
- `--spool-only` to write to the local spool without immediate ingestion;
- `--verbose` to print the event and deduplication result.

When a hook is configured for a provider, the non-spool hook path also runs an
incremental provider-file sync into that provider's database. Reports perform
the same sync before querying, so the database remains the only read path for
metrics and status.

Start the daemon with the default or a custom database:

```bash
agentusage telemetry daemon
agentusage telemetry daemon --db-path /path/to/telemetry.db
```

## Command reference

```text
agentusage --help
agentusage dashboard --help
agentusage server --help
agentusage command --help
agentusage command daily --help
agentusage daily --help
agentusage weekly --help
agentusage monthly --help
agentusage yearly --help
agentusage range --help
agentusage telemetry --help
```

Running `agentusage` without a subcommand prints this help. Use
`agentusage dashboard` for the terminal UI and `agentusage server` for the
browser UI.

## Privacy and data safety

`agentusage` is designed to operate locally. It reads provider files and
stores normalized events in local databases; it does not require a hosted
account or send usage data to a project-controlled service. Treat local usage
databases, raw events, and PostgreSQL credentials as sensitive data.

## Development

Contributor checks are documented in
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md). Automatic versioning, crates.io
publishing, platform builds, and GitHub releases are documented in
[docs/RELEASING.md](docs/RELEASING.md).

Release history is maintained in [CHANGELOG.md](CHANGELOG.md).

The Rust source is rooted in this repository. The local reference checkout is
excluded from Rust packages, GitHub Actions, and release archives.

## Contributing

Bug reports and pull requests are welcome. When adding or changing a provider:

1. Include a sanitized fixture or regression test when possible.
2. Preserve idempotent ingestion and backward-compatible storage behavior.
3. Run the checks in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).
4. Do not commit provider credentials, raw private transcripts, or local
   databases.

## License

`agentusage` is available under the [MIT License](LICENSE).
