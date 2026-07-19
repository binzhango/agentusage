# agentusage

`agentusage` is a local-first CLI for tracking AI coding-agent usage. It reads
local agent history, stores normalized events in SQLite or PostgreSQL, and
prints token, cost, model, client, cache, code-change, and Copilot-credit
breakdowns.

## Install

Download the archive for your platform from the latest
[GitHub release](https://github.com/binzhango/agentusage/releases/latest),
extract it, and put the binary on your `PATH`.

Example for Apple Silicon macOS:

```bash
curl -fL -o agentusage.tar.gz \
  https://github.com/binzhango/agentusage/releases/latest/download/agentusage-macos-aarch64.tar.gz
tar -xzf agentusage.tar.gz
sudo install -m 0755 agentusage /usr/local/bin/agentusage
agentusage --version
```

The release archives also include Linux x86_64/ARM64, macOS Intel, and Windows
x86_64 binaries. Verify `SHA256SUMS` from the release before installing in a
production environment.

## Quick start

Run the command directly after installation:

```bash
agentusage daily
```

On first use, `agentusage` checks for an initialized database. If none exists,
it asks whether to initialize SQLite, use PostgreSQL, or continue with the
JSONL fallback. Non-interactive invocations do not silently initialize a
database.

## Reports

All report commands support `--provider`:

```bash
# Today
agentusage daily --provider codex
agentusage daily --provider claude_code
agentusage daily --provider opencode
agentusage daily --provider copilot

# A specific date
agentusage daily --provider codex --date 2026-07-19

# Current week, month, or year
agentusage weekly --provider codex
agentusage monthly --provider copilot
agentusage yearly --provider claude_code

# Explicit month and year
agentusage monthly --provider copilot --month 2026-07
agentusage yearly --provider codex --year 2026

# Inclusive date range
agentusage range --provider copilot \
  --from 2026-07-01 --to 2026-07-19
```

Reports include:

- input, output, reasoning, cache-read, cache-write, and total tokens;
- estimated cost and cache-hit rate when token pricing is available;
- requests, prompts, sessions, lines added, and lines removed;
- model and client breakdowns such as CLI, IDE, Desktop, or user;
- Copilot AI credits and native AI-unit values when the source provides them.

Codex can read an alternate rollout-log directory:

```bash
agentusage daily --provider codex --sessions-dir /path/to/codex/sessions
```

## Provider sources

The Rust adapters currently support:

- `codex`: local Codex rollout JSONL;
- `claude_code`: Claude Code local session JSONL;
- `opencode`: OpenCode local session JSONL;
- `copilot`: Copilot CLI usage databases plus VS Code Copilot chat-session
  JSONL and extension logs.

For VS Code Copilot, the report includes the resolved model, IDE request count,
prompt/completion tokens, and exact `copilotCredits` values when VS Code stores
them. For example, a VS Code entry shown as `MAI-Code-1-Flash • 1.6 credits`
is reported as `MAI-Code-1-Flash` with approximately `1.6` AI credits.

## Storage

Provider reports use separate SQLite databases by default:

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

PostgreSQL is available through the only supported database variable. When no
initialized provider SQLite database exists, the first-run prompt can select
PostgreSQL:

```bash
export AGENTUSAGE_POSTGRES_URL='postgresql://user:password@localhost/agentusage'
agentusage monthly --provider copilot
```

`agentusage` never uses an `OPENUSAGE_*` database variable.

## Telemetry hooks and daemon

Ingest a hook payload passed as a positional argument:

```bash
agentusage telemetry hook codex \
  '{"turn_id":"turn-1","usage":{"input_tokens":10,"output_tokens":4}}' \
  --verbose
```

Or pipe the payload through standard input:

```bash
printf '%s' '{"event":"message","usage":{"input_tokens":10}}' \
  | agentusage telemetry hook claude_code --verbose
```

Supported hook sources are `codex`, `claude_code`, and `opencode`. Use
`--account-id` to preserve an account dimension, `--db-path` for a custom
telemetry database, and `--spool-only` to write the event to the local spool
without immediate database ingestion.

Start the telemetry daemon with the default database or a custom path:

```bash
agentusage telemetry daemon
agentusage telemetry daemon --db-path /path/to/telemetry.db
```

## Command reference

```text
agentusage --help
agentusage daily --help
agentusage weekly --help
agentusage monthly --help
agentusage yearly --help
agentusage range --help
agentusage telemetry --help
```

## Development

Contributor checks and automatic release instructions are documented in
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) and
[docs/RELEASING.md](docs/RELEASING.md). The Rust source is rooted in this
repository; the local reference checkout is excluded from Rust packages,
GitHub Actions, and release archives.
