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

## Release documentation checklist

Before merging a release change to `main`:

1. Update `CHANGELOG.md` under `Unreleased` with user-visible additions,
   behavior changes, removals, security implications, and any required resync
   or derived-database rebuild.
2. Update the README plus the relevant usage, API, configuration, or development
   guide. Document defaults, limits, privacy behavior, and unsupported provider
   cases instead of implying data is universally available.
3. Refresh checked-in SVG screenshots when the browser, TUI, API, or export
   presentation changes. Validate every SVG with `xmllint --noout` and inspect a
   rasterized copy before committing it.
4. Use sanitized fixtures and examples. Never include real prompt text, source
   paths, credentials, raw provider history, or normalized databases in release
   documentation or artifacts.
5. Run `make ci`, an optimized binary smoke test, and `cargo package --locked
   --allow-dirty` before pushing the release candidate.

## Automatic release

Releases are driven by pushes to `main`; no manual version edit or tag is
needed. The workflow serializes releases and, for each main-branch push:

1. Bumps the minor version with `cargo set-version` (for example, `1.0.0` to
   `1.1.0`).
2. Promotes the detailed `Unreleased` changelog into a dated version section,
   resets `Unreleased`, and uses the promoted text as the GitHub release body.
3. Runs formatting, compilation, tests, Clippy, and package checks.
4. Commits the version and changelog update and creates a `vX.Y.Z` tag.
5. Publishes `agentusage` to crates.io.
6. Builds archives and attaches them, plus SHA-256 checksums, to a GitHub
   release. The release body is generated from `RELEASE_NOTES.md`; GitHub's
   pull-request summary is not used.

After the workflow completes, download an archive for one target platform and
verify its checksum before publishing installation instructions.

GitHub repository setup:

1. Create a crates.io API token with permission to publish `agentusage`.
2. Add it as an Actions secret named `CARGO_REGISTRY_TOKEN`.
3. Keep Actions enabled with workflow write permission for the repository.

The workflow builds these targets:

- `aarch64-apple-darwin`
- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-pc-windows-msvc`

The release workflow uses sparse checkout and Cargo package exclusion. Local
agent instruction files (`AGENTS.md`, `.agents/`, and `.codex/`) and the local
reference subtree are not packaged, compiled, uploaded, or included in release
archives.

## Release smoke tests

On each target, verify:

```bash
agentusage --help
agentusage daily --provider codex
agentusage daily --provider copilot
```

For a clean machine, confirm the first-run storage prompt rejects report access
until SQLite or PostgreSQL is initialized, then test provider synchronization.

For a release containing prompt-history changes, use sanitized provider
fixtures and additionally verify:

```bash
agentusage sync codex --sessions-dir /path/to/sanitized/sessions
agentusage dashboard
agentusage server --host 127.0.0.1 --port 8787
```

In another terminal while the server is running:

```bash
curl 'http://127.0.0.1:8787/api/prompts?provider=codex&window=all&limit=2'
```

In the TUI, open Codex, press `p`, select a prompt, and expand it with `Enter`.
In the browser, confirm no prompt request occurs until `Show prompts` is chosen,
then verify expansion, search, pagination, and an empty result. Confirm API
responses include `Cache-Control: no-store` and that a non-loopback host is
rejected.
