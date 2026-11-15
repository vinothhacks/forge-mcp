# F1: build and baseline

**Verdict: PASS.** Date: 2026-11-12. Toolchain: rustc/cargo 1.99.1, `stable-x86_64-pc-windows-msvc`; Node 24.22.

| Check | Result |
|---|---|
| `cargo build --release` | PASS (4m12s, MSVC linker) |
| Binary SHA-256 | `ae7389b81b8e5db4041d0b4267b3cfe27a9e66fdc62ba0a4e023e954b3e9064e` (15,150,080 bytes) |
| `forge-mcp --version` / `--help` | `forge-mcp 0.1.0`; flags `--transport --host --port --auth-token --allow-root --db-path --database --embedding-url --embedding-model` |
| `cargo test -- --test-threads=1` | 5 passed, 0 failed |
| `npm test` (`FORGE_MCP_BIN` set) | 3 passed, 0 failed |
| `cargo clippy --release --all-targets` | 14 warnings (report only; fixed before release) |
| `cargo fmt --check` | 20 diffs (report only; fixed before release) |

This binary is the baseline every feature phase (F2–F15) tested against.
