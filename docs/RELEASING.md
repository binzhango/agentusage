# Releasing agentusage

## Local verification

Run the same checks used by pull requests:

```bash
make ci
```

Build and smoke-test the optimized binary:

```bash
cargo build --release --locked
./target/release/agentusage --help
./target/release/agentusage daily --provider codex
```

The report command may initialize a provider-specific SQLite database on first
use. Do not put real databases, credentials, or local agent logs in the Git
repository.

## Automatic release

Releases are driven by pushes to `main`; no manual version edit or tag is
needed. The workflow serializes releases and, for each main-branch push:

1. Bumps the patch version with `cargo set-version`.
2. Generates a dated `CHANGELOG.md` section from commits since the previous
   release tag and prepares the same commit list as the GitHub release body.
3. Runs formatting, compilation, tests, Clippy, and package checks.
4. Commits the version and changelog update and creates a `vX.Y.Z` tag.
5. Publishes `agentusage` to crates.io.
6. Builds archives and attaches them, plus SHA-256 checksums, to a GitHub
   release. The release body is generated from `RELEASE_NOTES.md`; GitHub's
   pull-request summary is not used.

GitHub repository setup:

1. Create a crates.io API token with permission to publish `agentusage`.
2. Add it as an Actions secret named `CARGO_REGISTRY_TOKEN`.
3. Keep Actions enabled with workflow write permission for the repository.

The workflow builds these targets:

   - `aarch64-apple-darwin`
   - `x86_64-unknown-linux-gnu`
   - `aarch64-unknown-linux-gnu`
   - `x86_64-pc-windows-msvc`

7. Download one archive for the target platform and verify its checksum before
   publishing installation instructions.

The release workflow uses sparse checkout and Cargo package exclusion. The
local reference subtree is not packaged, compiled, uploaded, or included in
release archives.

## Release smoke tests

On each target, verify:

```bash
agentusage --help
agentusage daily --provider codex
agentusage daily --provider copilot
```

For a clean machine, confirm the first-run storage prompt rejects report access
until SQLite or PostgreSQL is initialized, then test provider ingestion.
