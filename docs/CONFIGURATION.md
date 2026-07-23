# Configuration

Agentusage is local-first and uses SQLite by default. This guide covers state
paths, automatic synchronization, PostgreSQL, telemetry hooks, and operational
security.

## Configuration file

The default configuration path is:

```text
~/.config/agentusage/config.toml
```

Set `AGENTUSAGE_CONFIG` to use another file.

## Local storage

Provider usage is stored in separate SQLite databases:

```text
~/.local/state/agentusage/codex.db
~/.local/state/agentusage/claude_code.db
~/.local/state/agentusage/opencode.db
~/.local/state/agentusage/copilot.db
~/.local/state/agentusage/pi.db
```

Telemetry hooks and the daemon use:

```text
~/.local/state/agentusage/telemetry.db
```

Set `XDG_STATE_HOME` to change the state root:

```bash
XDG_STATE_HOME="$HOME/.local/state" au daily --provider codex
```

Provider source files are read during synchronization. Reports, dashboards, and
API endpoints read normalized storage.

## Automatic synchronization

Configure background ingestion in `config.toml`:

```toml
[sync]
auto_sync = true
refresh_seconds = 300
```

When enabled, the browser server starts an ingestion loop for every supported
provider and refreshes at the configured interval.

Report commands perform an incremental sync before querying. Non-spool
telemetry hooks also synchronize their provider after processing an event.

## PostgreSQL

Set `AGENTUSAGE_POSTGRES_URL` to use PostgreSQL when no initialized provider
SQLite database exists:

```bash
export AGENTUSAGE_POSTGRES_URL='postgresql://user:password@localhost/agentusage'
au monthly --provider copilot
```

The first-run storage prompt can select PostgreSQL. Agentusage does not use an
`OPENUSAGE_*` database variable.

Treat the connection URL and database contents as sensitive.

## Pi session directory

Pi sessions are discovered recursively below:

```text
~/.pi/agent/sessions/
```

Override this path with an environment variable or command option:

```bash
PI_CODING_AGENT_SESSION_DIR=/path/to/sessions au sync pi
au sync pi --sessions-dir /path/to/sessions
```

## Telemetry hooks

Pass a hook payload positionally:

```bash
au telemetry hook codex \
  '{"turn_id":"turn-1","usage":{"input_tokens":10,"output_tokens":4}}' \
  --verbose
```

Or pipe JSON through standard input:

```bash
printf '%s' '{"event":"message","usage":{"input_tokens":10}}' \
  | au telemetry hook claude_code --verbose
```

Supported hook sources are `codex`, `claude_code`, and `opencode`.

Useful options:

| Option | Purpose |
| --- | --- |
| `--account-id` | Preserve an account dimension |
| `--db-path` | Use a custom telemetry database |
| `--spool-only` | Write to the local spool without immediate ingestion |
| `--verbose` | Print event and deduplication details |

Start the telemetry daemon with:

```bash
au telemetry daemon
au telemetry daemon --db-path /path/to/telemetry.db
```

## Server logging

Normal server output is intentionally concise:

```bash
au server
```

Use verbose mode for troubleshooting:

```bash
au server --verbose
```

Verbose logs include request paths, provider windows, backend selection,
read-only database opens, query duration, rendered trend days, and background
ingestion.

## Privacy and network safety

Agentusage does not require a hosted account and does not send usage data to a
project-controlled service. Local provider history and normalized databases may
contain sensitive metadata or raw events.

The HTTP server binds to loopback by default:

```text
127.0.0.1:8787
```

The server currently has no built-in authentication or CORS policy. Do not bind
it to a public or untrusted network without authentication and access control
provided by a reverse proxy.

## Backups and migration

Source agent history remains the primary input for normalized provider
databases. Preserve source history if you expect to rebuild local usage.

When the Pi schema or ingestion format requires a rebuild, Agentusage retains
the previous derived database as `pi.db.legacy` or a numbered variant.
