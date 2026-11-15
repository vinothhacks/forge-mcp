# Contributing to Forge

## Build and test

```bash
cargo build --release
cargo test -- --test-threads=1
cd npm && npm test
```

The npm tests locate the binary in `target/release/`; build it first or set `FORGE_MCP_BIN`.

## Gates enforced by CI

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets -- -D warnings`
- `node scripts/check-version.mjs` (Cargo.toml and npm/package.json versions must match)
- Rust and npm test suites on Ubuntu, Windows and macOS

## Releasing

1. Bump `version` in `Cargo.toml` and `npm/package.json` to the same value and add a dated entry to `CHANGELOG.md`, all in one commit.
2. Tag it: `git tag -a vX.Y.Z -m "vX.Y.Z"` and `git push origin vX.Y.Z`.
3. The release workflow builds binaries for Windows, Linux and macOS, attaches them to a GitHub Release with `SHA256SUMS`, then publishes the crate and the npm package. Tags containing a hyphen (for example `v0.2.0-rc.1`) produce a pre-release with binaries only.

Required repository secrets: `CARGO_REGISTRY_TOKEN` and `NPM_TOKEN`.

## Commit messages

Commits made with Claude Code end with:

```
Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
```
