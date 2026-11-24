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
3. Refresh the affected guide images with sample data. The [browser capture](images/browser-dashboard.jpg) is JPEG and the [terminal capture](images/terminal-dashboard.png) is PNG. Generate the [provider report](images/svg-export.svg) using **Download SVG**. Open the images and validate SVG assets with `xmllint --noout`.
4. Use sanitized fixtures and examples. Never include real prompt text, source
   paths, credentials, raw provider history, or normalized databases in release
   documentation or artifacts.
5. Run `make ci` and smoke-test the optimized binaries before pushing the release candidate.

## Automatic release

The [release workflow](../.github/workflows/release.yml) publishes to crates.io automatically when a push to `main` changes `src/`, `Cargo.toml`, or `Cargo.lock`. It bumps the minor version, verifies the source, commits the version and changelog, and tags the revision. Platform jobs build that revision and attach archives and SHA-256 checksums to the GitHub release. Pull-request CI still runs for documentation and workflow changes.

### Recover an existing GitHub release

If the crate publish succeeded and a platform build failed, open **Actions → Automatic Release → Run workflow**. Select `main` to use the current runners. Enter the full SHA of the existing `chore(release): v… [skip ci]` commit on `main`. The manual run builds all four archives from that commit and reads its dated changelog entry. It skips the version bump and crates.io publish. If the original run never pushed a tag, the GitHub release creates it at the supplied commit.

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

### Dashboard and API checks

Use a separate Agentusage state directory and synthetic provider history for documentation captures. For an isolated browser preview, initialize the test database with a manual sync and start the server with a test config containing `[sync]` and `auto_sync = false`. This keeps the fixture data stable during capture.

- **Browser activity.** Check Today against hourly buckets and a longer window against daily buckets. Inspect the peak, select a provider row, then open its model page. Check that All time labels the 90-day chart separately from full-history totals.
- **Codex cache.** Test a period with input and zero writes, a period with positive writes for one model, and a period with no input. Compare the displayed percentage, exact model reads, and presence of the write column with the same period’s summary. Download an SVG and verify its cache metrics and columns.
- **Prompt history.** Exercise **Prompt history** beside a provider and **Show prompts** on its page. Verify that the first prompt request follows the opening action. Test search, expansion, pagination, and an empty result. In the TUI, select a provider, press `p`, and use `Enter` to read a prompt.
- **Terminal navigation.** Check the provider and request panes at 110 columns or wider and request navigation at a narrower width. Exercise the date keys, `v`, `?`, and back navigation.

With the test server running, check the HTTP responses:

```bash
curl -i 'http://127.0.0.1:8787/api/trend?provider=codex&window=today&interval=hour'
curl -i 'http://127.0.0.1:8787/api/trend?provider=codex&window=7d&interval=hour'
curl -i 'http://127.0.0.1:8787/api/prompts?provider=codex&window=all&limit=2'
```

The first request returns hourly buckets, the second returns HTTP 400 with `invalid_interval`, and the third returns a prompt page. Confirm `Cache-Control: no-store` on JSON responses and verify the server rejects a non-loopback bind address.
