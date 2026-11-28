# Contributing to Forge

## Build and test

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --locked -j 2 -- -D warnings
cargo test --locked -j 2 --all-targets -- --test-threads=1
cargo build --release --locked -j 2
node scripts/check-version.mjs
node --test scripts/check-version.test.mjs
(cd npm && FORGE_MCP_SKIP_DOWNLOAD=1 npm test)
```

The npm wrapper has no dependencies, so the tests run without an install step. The tests locate the
native binary in `target/release/`. If you use a different Cargo target directory, set `FORGE_MCP_BIN`
to that build's binary.

## Gates enforced by CI

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --locked -j 2 -- -D warnings`
- Version consistency and release tag regression tests
- Rust and npm test suites on Ubuntu, Windows and macOS
- `cargo install cargo-audit --version '=0.23.0' --locked -j 2` then `cargo audit --file Cargo.lock`

Set `release-gate` as the required PR check so formatting, version checks, tests and the audit all block a merge.

## Releasing

1. Bump `version` in `Cargo.toml` and `npm/package.json` together, update `Cargo.lock` and add a dated
   entry to `CHANGELOG.md`. Merge the reviewed change through the `release-gate` check.
2. After owner approval, tag the merged revision `vX.Y.Z`. The tag-triggered workflow verifies that
   the tag is reachable from `main` and agrees with the versions in source. It builds, extracts and checks
   `--version` and stdio initialization for each Windows, Linux and macOS archive before attaching all
   five archives and `SHA256SUMS` to the GitHub release. Tags such as `v0.2.0-rc.1` create a prerelease.
3. Download the published archives, verify their hashes and run the fixture check on every platform
   you intend to certify. [Install and first use](README.md#install)

Registry publication is separate. After owner approval and publication of the stable GitHub release,
dispatch the Release workflow on the tag with `gh workflow run release.yml --ref vX.Y.Z`. This ties npm
provenance to the tagged source. Configure required reviewers on both the `crates-publish` and
`npm-publish` GitHub environments before dispatching. Store `CARGO_REGISTRY_TOKEN` in `crates-publish`
and `NPM_TOKEN` in `npm-publish`. The npm job publishes `forge-codebase-mcp` with provenance.

## Commit messages

Commits made with Claude Code end with:

```
Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
```
