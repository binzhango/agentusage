# Development guide

## Checks

```bash
make fmt       # formatting check
make check     # compile check
make test      # unit and integration tests
make lint      # Clippy with warnings denied
make package   # Cargo package boundary check
make ci        # all checks
```

Keep provider parsing deterministic and idempotent. Add a fixture or regression
test whenever a local agent format changes. Normalized databases are derived
state: update the canonical schema directly and document when users must rebuild
their provider database instead of adding compatibility migrations.

PostgreSQL parity is exercised in CI with `AGENTUSAGE_TEST_POSTGRES_URL`. The
test compares aggregate summaries, local-day trends, event filters, and detail
metadata against the SQLite implementation.

Use `cargo run --bin agentusage -- dashboard` to exercise the Ratatui
dashboard, or `cargo run --bin agentusage -- server` to exercise the local
browser dashboard. The bare `agentusage` command prints help. Use
`cargo run --bin agentusage -- daily` or the other period commands when running
detailed reports from a pipe or CI.

The Rust crate is rooted at this directory. Local reference material is not a
Cargo workspace member and must not be added to package includes, CI checkout
inputs, or release archives.
