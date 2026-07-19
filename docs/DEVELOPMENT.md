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
test whenever a local agent format changes. Storage migrations must be
backward-compatible and must not delete existing usage data.

The Rust crate is rooted at this directory. Local reference material is not a
Cargo workspace member and must not be added to package includes, CI checkout
inputs, or release archives.
