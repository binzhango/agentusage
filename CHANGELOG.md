# Changelog

All notable changes to `agentusage` are documented in this file.

The release workflow updates this file automatically when `main` is released.

## [Unreleased]

Changes that have not been released yet.

### Added

- Added a startup release check for both `agentusage` and `au`. When GitHub
  confirms that a newer release exists, the CLI prints a friendly upgrade
  reminder and the TUI keeps the notice visible in its header; offline and
  failed checks stay silent.
- Added a persistent, high-contrast TUI date-range selector for `Today`,
  `Week`, `30 Days`, and `All`, with direct `1`-`4` shortcuts in addition to
  `w` cycling.
- Added a responsive TUI control bar that adapts to the provider, request, and
  prompt views, with balanced Navigate/View/App groups on wide terminals and a
  compact two-line layout on narrower screens.
- Added flexible TUI prompt-history navigation: direct `p` access from the
  provider grid, horizontal provider navigation with arrows or `h`/`l`,
  `Home`/`End` jumps, and hierarchical `Esc`/`Backspace` return navigation.
- Added macOS-friendly TUI scrolling with `Ctrl+U`/`Ctrl+D`,
  `Ctrl+B`/`Ctrl+F`, `Space`/`b`, and mouse-wheel support.
- Added the `m` TUI mouse-capture toggle so macOS users can disable mouse
  reporting, select rendered prompt text with the terminal, and copy it with
  `Cmd+C`, then restore wheel scrolling when finished.

### Fixed

- Prompt-history tables now fit the available terminal width exactly and use
  Unicode display widths, preventing wrapped or skewed columns when prompts or
  model names contain wide characters.
- PostgreSQL summary buckets now cast `SUM(BIGINT)` results back to `BIGINT`,
  preventing model, provider, and client aggregates from failing Rust `i64`
  deserialization in PostgreSQL-backed dashboards and reports.
- PostgreSQL loopback-host validation now compiles cleanly on Windows with
  warnings denied while retaining Unix-socket support on Unix platforms.

## [1.4.0] - 2026-08-10

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

## [1.3.0] - 2026-07-23

- Merge pull request #13 from binzhango/feature_browser (5ece9e8)
- Restructure project documentation (16a218b)
- Polish dashboard controls and chart tooltips (4e49a72)


## [1.2.0] - 2026-07-23

- Merge pull request #12 from binzhango/feature_pi_agent (160a43f)
- Fix clippy backend selection warning (77e6c66)
- Polish dashboard exports and server observability (510bc19)
- Document Pi agent integration (622f188)
- Auto-migrate legacy Pi usage database (9948a3f)
- Fix clippy test module ordering (7a15d80)
- Add Pi coding agent usage ingestion (bcd838a)


## [1.1.0] - 2026-07-21

- Merge pull request #11 from binzhango/feature_choose-interactive-metrics-framework (eaff361)
- Merge branch 'main' into feature_choose-interactive-metrics-framework (a4f4fa5)
- Start stable release line at v1.0.0 (826dc3b)
- Ship stable usage dashboard experience (7dba4ac)
- Add spending window comparison (cb4a42b)
- Update release actions for Node 24 (35e9764)


## [0.1.9] - 2026-07-21

- Merge pull request #10 from binzhango/feature_dashboard_table (fb2d2f7)
- Simplify report command usage (e6252e0)


## [0.1.8] - 2026-07-21

- Merge pull request #9 from binzhango/feature_dashboard_table (3522d75)
- Fix redundant timestamp closure (44e3700)
- Improve dashboard tables and provider startup (d88348c)
- Add database-backed sync and architecture docs (6a8171a)


## [0.1.7] - 2026-07-20

- Merge pull request #8 from binzhango/feature_agentusage_tui (79d0396)
- Fix incremental ingestion and usage reporting (d5c1bd5)


## [0.1.6] - 2026-07-20

- Merge pull request #7 from binzhango/feature_agentusage_tui (a185293)
- Clean up transient release notes (4ae6b09)
- chore(release): v0.1.5 [skip ci] (82d4056)
- Merge pull request #6 from binzhango/feature_agentusage_tui (891a2d3)
- Use commit changelog for releases (461d3d9)


## [0.1.5] - 2026-07-20

- Merge pull request #6 from binzhango/feature_agentusage_tui (891a2d3)
- Use commit changelog for releases (461d3d9)


## [0.1.4] - 2026-07-20

- Merge pull request #5 from binzhango/feature_agentusage_tui (9e7e4e9)
- Simplify CLI installation (650b24c)
- chore(release): v0.1.3 [skip ci] (2a10d99)
- Merge pull request #4 from binzhango/feature_agentusage_tui (96c2d36)
- Fix Windows browser launcher warning (690dc3b)
- Add roadmap checklist (05e0e81)
- Add au executable alias (184ab40)
- Unify dashboard server and CLI interfaces (b5d4aec)


## [0.1.3] - 2026-07-20

- Merge pull request #4 from binzhango/feature_agentusage_tui (96c2d36)
- Fix Windows browser launcher warning (690dc3b)
- Add roadmap checklist (05e0e81)
- Add au executable alias (184ab40)
- Unify dashboard server and CLI interfaces (b5d4aec)


## [0.1.2] - 2026-07-19

- Merge pull request #3 from binzhango/feature_init (174900e)
- Remove Intel macOS release target (454508c)
- chore(release): v0.1.1 [skip ci] (bd9ff5f)
- Merge pull request #2 from binzhango/feature_init (0b1568b)
- Automate release changelog generation (d980e05)
- Allow dirty package check during release bump (970fc26)
- Update GitHub Actions for Node 24 (47f800f)
- Merge pull request #1 from binzhango/feature_init (68b2fe5)
- Make Copilot timestamp test timezone independent (1675c49)
- Polish OSS README (3658a0d)
- Document agentusage CLI usage (f009c06)
- Add Rust CI and automatic releases (97f4ae8)
- Initial commit (a150702)


## [0.1.1] - 2026-07-19

- Merge pull request #2 from binzhango/feature_init (0b1568b)
- Automate release changelog generation (d980e05)
- Allow dirty package check during release bump (970fc26)
- Update GitHub Actions for Node 24 (47f800f)
- Merge pull request #1 from binzhango/feature_init (68b2fe5)
- Make Copilot timestamp test timezone independent (1675c49)
- Polish OSS README (3658a0d)
- Document agentusage CLI usage (f009c06)
- Add Rust CI and automatic releases (97f4ae8)
- Initial commit (a150702)
