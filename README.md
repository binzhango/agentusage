# agentusage

Rust implementation of local-first AI coding-agent usage tracking.

The repository root is the Rust project. The binary reads local agent telemetry,
normalizes it into storage, and generates reports without requiring a hosted
service.

## Current Rust slice

The first slice provides compatible telemetry hook ingestion for Claude Code,
Codex, and OpenCode, including SQLite raw-event storage and idempotent
deduplication.

```bash
cargo run -- telemetry hook codex \
  '{"turn_id":"turn-1","usage":{"input_tokens":10,"output_tokens":4}}' \
  --verbose
```

Use `make ci` for the full local verification suite and `cargo build --release`
for a release binary.

## Supported toolchain

The project requires Rust 1.85 or newer and uses the stable toolchain. Install
Rust with [rustup](https://rustup.rs/), then run:

```bash
rustup toolchain install stable
rustup component add rustfmt clippy
make ci
```

The source-reference checkout used during development is intentionally outside
the Rust package, CI inputs, and release archives.

## Codex daily usage

Read today’s usage from local Codex rollout logs:

```bash
cargo run -- daily --provider codex
```

Use `--date YYYY-MM-DD` for a specific local calendar day or
`--sessions-dir PATH` to point at another Codex sessions directory.

Additional aggregation commands are available:

```bash
cargo run -- weekly --date 2026-07-19
cargo run -- monthly --month 2026-07
cargo run -- yearly --year 2026
cargo run -- range --from 2026-07-01 --to 2026-07-19
```

## Storage location

The default SQLite database is stored at:

```text
$XDG_STATE_HOME/agentusage/telemetry.db
```

When `XDG_STATE_HOME` is unset, Rust uses
`~/.local/state/agentusage/telemetry.db` on macOS/Linux. Set the variable to
override the state directory:

```bash
XDG_STATE_HOME="$HOME/.local/state" cargo run -- daily --provider codex
```

## Releases

Every push to `main` (including a merged pull request) automatically bumps the
patch version, publishes the crate to crates.io, builds macOS/Linux/Windows
archives, and creates a GitHub release. The repository needs a
`CARGO_REGISTRY_TOKEN` secret containing a crates.io API token. See
[docs/RELEASING.md](docs/RELEASING.md) for setup and smoke tests.
