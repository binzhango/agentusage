# Changelog

All notable changes to `agentusage` are documented in this file.

The release workflow updates this file automatically when `main` is released.

## [Unreleased]

Changes that have not been released yet.

### Added

- Added request-level TUI inspection with daily sparklines, timestamps, source
  metadata, status, duration, and token provenance.
- Added paginated `/api/events` list/detail endpoints with filters, stable
  cursors, preserved raw payloads, and structured JSON errors.
- Added provider-aware prompt retrieval for Codex, Claude Code, OpenCode, and
  Pi, with paginated/searchable API routes, opt-in expandable browser history,
  and TUI prompt inspection. Assistant messages, tool results, and metadata
  records are excluded.
- Added SQLite/PostgreSQL parity coverage for summaries, trends, and events.
- Added Pi coding-agent JSONL ingestion with prompt, request, token, cache,
  cost, model, provider, project, and tool-call tracking.
- Added Pi provider/model labels such as `openai-codex:gpt-5.6-luna` and a
  provider breakdown in the dashboard.

### Changed

- Token totals and cache rates now use explicit OpenAI, Anthropic, or additive
  provider semantics without double-counting inclusive breakdown fields.
- OpenCode mutable message snapshots are updated in place, Copilot CLI/IDE
  records use stable shared IDs, and invalid source timestamps are rejected.
- Daily trends now use local calendar boundaries consistently across SQLite and
  PostgreSQL; dashboard/API reads validate schemas without mutating them.
- Normalized storage now carries a schema version and rejects obsolete derived
  databases so old and corrected accounting cannot be mixed silently.
- Selecting SQLite for an obsolete derived database now rebuilds it cleanly,
  including stale WAL/SHM sidecars, before synchronizing provider history.
- The shared application is compiled once as a library with separate
  `agentusage` and `au` entry points.
- Claude Code and OpenCode synchronization rescan existing local history once
  after this upgrade so earlier prompts become available without duplicate
  normalized events.

### Security

- Prompt text remains local, browser prompt requests require an explicit
  `Show prompts` action, API responses use `Cache-Control: no-store`, and the
  HTTP server accepts loopback bind addresses only.
- Local agent instruction files are excluded from crates.io packages and
  release package verification.

### Removed

- Removed the unfinished telemetry hook/daemon surface and obsolete command and
  API aliases.
