# Configuration

Agentusage reads agent history from disk, imports usage into SQLite or PostgreSQL, and serves the browser dashboard on loopback. Source directories and database directories serve different purposes.

## Configuration file

| Platform | Default config file | Default SQLite directory |
| --- | --- | --- |
| macOS and Linux | `~/.config/agentusage/config.toml` | `~/.local/state/agentusage/` |
| Windows | `%APPDATA%\agentusage\config.toml` | `%LOCALAPPDATA%\agentusage` |

Set `AGENTUSAGE_CONFIG` to select a specific file. On macOS and Linux, `XDG_CONFIG_HOME` changes the base configuration directory. On all platforms, `XDG_STATE_HOME` changes the base state directory. Agentusage appends `agentusage/` to either base directory.

## Automatic synchronization

These are the defaults when the configuration file is absent:

```toml
[sync]
auto_sync = true
refresh_seconds = 300
```

- **Browser and API.** At startup, `au server` attempts an import for each supported provider. Each worker waits the configured number of seconds after a cycle before its next attempt. Initialize a provider’s storage with `au sync <provider>` in an interactive terminal. Reload an open browser page or select another date range to fetch the latest totals.
- **Terminal.** `au dashboard` first displays stored usage. With automatic sync enabled, it then imports provider history and refreshes periodically. Press `r` to sync and refresh manually.
- **CLI.** `au sync` and the period report commands still import when `auto_sync = false`. With that setting, the server can continue serving stored history indefinitely. Restart a running server or terminal dashboard after changing the config file. `refresh_seconds` must be a positive integer.

### When recent Codex Desktop usage is missing

1. Run `au sync codex` and check the reported file and token-record counts. An incremental sync can report zero new records when the stored history is current.
2. Run `au daily --provider codex` and reload **Today** in the browser. Today uses the server machine’s local date. **All time** includes earlier sessions in provider totals and limits the chart to the latest 90 days.
3. If manual sync finds new usage but the server stays stale, check which config file and state directory the server process uses. A temporary `AGENTUSAGE_CONFIG` can disable automatic imports, and `XDG_STATE_HOME` can point it at a different database.
4. If the source files are elsewhere, pass their directory to `au sync codex --sessions-dir /path/to/sessions`. This override applies to that invocation. Use `au server --verbose` to see background import progress and storage errors.

## Local storage

Each initialized provider has its own SQLite file, such as `codex.db`, `claude_code.db`, `opencode.db`, `copilot.db`, or `pi.db`. Manual sync and reports can initialize storage interactively. HTTP requests read the stored data.

| Provider | Default source used by sync |
| --- | --- |
| Codex | Rollout JSONL under `~/.codex/sessions/`, including Desktop sessions |
| Claude Code | Session JSONL under `~/.claude/projects/` |
| OpenCode | `~/.opencode/events/` when present, otherwise `~/.local/state/opencode/events/` |
| GitHub Copilot | CLI session store under `~/.copilot/` or `COPILOT_HOME`, plus supported VS Code databases, chat sessions, and logs |
| Pi | Session JSONL under `~/.pi/agent/sessions/` |

`au sync <provider> --sessions-dir <directory>` accepts a source override for Codex, Claude Code, OpenCode, and Pi.

## Pi session directory

Use `PI_CODING_AGENT_SESSION_DIR` to set Pi’s source directory for commands or for a server started with that environment. An explicit `--sessions-dir` takes precedence on a manual sync.

```bash
PI_CODING_AGENT_SESSION_DIR=/path/to/sessions au server
au sync pi --sessions-dir /path/to/sessions
```

## PostgreSQL

Agentusage uses a valid provider SQLite database first, then checks `AGENTUSAGE_POSTGRES_URL`. Set the URL before running the initial sync to make PostgreSQL available as a storage choice.

```bash
export AGENTUSAGE_POSTGRES_URL='postgresql://user:password@localhost/agentusage'
au sync codex
```

PostgreSQL accepts loopback TCP connections and local Unix sockets. Queries open read-only sessions. Synchronization opens a writable connection and initializes the schema when needed. Protect the connection URL and database contents.

## Server logging

`au server --verbose` logs request paths, database selection, query timing, the number of trend buckets, and provider import activity. Look for `ingest.complete provider=codex` for a successful Codex import or `ingest.error` for the reported cause of a failure.

## Privacy and network safety

The HTTP server accepts loopback addresses and defaults to `127.0.0.1:8787`. It has no authentication. A local process with access to the server or database can read prompt text and raw events. The browser fetches prompt bodies on demand, and JSON responses carry `Cache-Control: no-store`.

## Rebuilding derived storage

If Agentusage reports an **obsolete SQLite schema**, an interactive `au sync <provider>` offers to replace that provider’s derived database. A rebuild can recover only the history still available in the original source files. Preserve any database or source history you need before choosing the rebuild option. Routine synchronization and the dashboard cache-display changes do not require a rebuild.
