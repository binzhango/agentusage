# Changelog

All notable changes to `agentusage` are documented in this file.

The release workflow updates this file automatically when `main` is released.

## [Unreleased]

Changes that have not been released yet.

### Added

- Added Pi coding-agent JSONL ingestion with prompt, request, token, cache,
  cost, model, provider, project, and tool-call tracking.
- Added Pi provider/model labels such as `openai-codex:gpt-5.6-luna` and a
  provider breakdown in the dashboard.

### Changed

- Pi usage now automatically migrates legacy derived databases to a fresh
  JSONL-rebuilt database, preserving the old database as a `.legacy` backup.
