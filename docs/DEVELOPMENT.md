# Development guide

## Run the checkout

Use a current stable Rust toolchain. Both binaries share the same command implementation. To exercise the checkout directly, run

```bash
cargo run --locked --bin au -- dashboard
cargo run --locked --bin au -- server --open
```

Run each command in its own terminal. The TUI requires an interactive terminal. Use a period report such as `cargo run --locked --bin au -- daily --provider codex` for plain-text output.

## Where to change the dashboards

| Area | Files | What they control |
| --- | --- | --- |
| Browser presentation | [dashboard.html](../src/web/dashboard.html), [dashboard.css](../src/web/dashboard.css), [dashboard.js](../src/web/dashboard.js) | Provider navigation, range selection, cache metrics, history, and SVG reports |
| Activity chart | [activity.js](../src/web/activity.js) | Time axis, peak marker, keyboard inspection, and aligned rows |
| HTTP and assets | [server.rs](../src/server.rs), [view.rs](../src/view.rs) | Loopback routes, time windows, zero-filled trend buckets, and embedded assets |
| Terminal | [tui/mod.rs](../src/tui/mod.rs), [overview.rs](../src/tui/overview.rs), [activity.rs](../src/tui/activity.rs) | State, key handling, provider overview, request and prompt inspection |
| Usage data | [providers](../src/providers), [core](../src/core), [storage](../src/storage) | Parsers, token semantics, and SQLite/PostgreSQL queries |

The Rust binary embeds the HTML, CSS, and JavaScript at compile time. After changing a browser asset, rebuild and restart the server, then reload the page. The browser code runs directly in the browser.

## Checks

```bash
make fmt       # formatting check
make check     # compile check
make test      # unit and integration tests
make lint      # Clippy with warnings denied
make package   # build and verify the Cargo package
make ci        # run all of the above
```

CI runs on Linux, macOS, and Windows. A separate job sets `AGENTUSAGE_TEST_POSTGRES_URL` and compares PostgreSQL with SQLite for summaries, daily and hourly trends, event filtering, and prompt retrieval. To run the parity test locally, set that variable to a local test database where the test user can create and drop its isolated schema, then run

```bash
cargo test --locked sqlite_and_postgres_return_equivalent_usage_views
```

For dashboard changes, check an empty period and one with usage. Test a narrow terminal and one at least 110 columns wide. For browser chart or cache changes, compare the page and its downloaded SVG against the same provider summary. See the [release smoke tests](RELEASING.md#release-smoke-tests).

Keep ingestion deterministic and idempotent. Use synthetic records for parser regressions and published screenshots. Normalized databases are derived from source history. Change the canonical schema directly and document a rebuild when the schema changes. Keep local reference repositories outside the crate, CI inputs, and release archives.
