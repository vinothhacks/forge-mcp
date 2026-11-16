# Forge verification (v0.1.0)

Every feature was re-verified from the current source between 2026-11-12 and 2026-11-16. Each feature had its own phase, run by a separate agent against the same baseline binary, with its own database, fixture and ports. A phase fixed only the files it owned and added a regression test for each defect. All fixes were then merged and the whole suite was re-run (F16).

Baseline binary (F1): `ae7389b81b8e5db4041d0b4267b3cfe27a9e66fdc62ba0a4e023e954b3e9064e`
Integrated binary (F16, Windows x64 release): `ad6e633d9d3e83be8590bf844898ad9eb89cf4f71ecf8dd210c4a567efef3ada`

## Results

| Phase | Feature | Verdict | Defects fixed | Regression tests |
|---|---|---|---|---|
| [F1](F1-build_baseline.md) | Build and baseline | PASS | — | — |
| [F2](F2-workspace_open.md) | `workspace_open` | FAIL-FIXED | `.gitignore` ignored outside git repositories | `test_workspace_open_respects_gitignore_outside_git_repo` |
| [F3](F3-workspace_status.md) | `workspace_status` freshness | PASS | — | — |
| [F4](F4-code_search.md) | `code_search` hybrid ranking | PASS | — | — |
| [F5](F5-symbol_find.md) | `symbol_find` | FAIL-FIXED | Unknown `kind` silently returned functions | `test_symbol_find_rejects_or_aliases_unknown_kind` |
| [F6](F6-symbol_read.md) | `symbol_read` | FAIL-FIXED | Ambiguous name resolved to the trait declaration, not the method | `test_symbol_read_ambiguous_name_disambiguation_and_errors` |
| [F7](F7-symbol_references.md) | `symbol_references` | FAIL-FIXED | `include_declarations` ignored; `file_path` did not restrict hits | `test_symbol_references_include_declarations` |
| [F8](F8-graph_query.md) | `graph_query` | FAIL-FIXED | Importer substring match; unknown `query_kind` accepted; unresolved targets not reported | `test_graph_query_strict_kind_aliases_and_importers` |
| [F9](F9-impact_analyze.md) | `impact_analyze` | FAIL-FIXED | `latest_build.rs` classified as a test file | `test_is_test_path_does_not_match_latest_prefix` and 2 more |
| [F10](F10-context_build.md) | `context_build` | FAIL-FIXED | `snapshot_policy` values ignored; filters bypassed by graph expansion | `test_context_build_snapshot_policy_values`, `test_context_build_graph_expansion_respects_filters` |
| [F11](F11-jobs.md) | `job_status` / `job_cancel` | FAIL-FIXED | Late cancel reported success but indexing completed; silent cancel of finished jobs | `test_job_cancel_terminal_and_unknown_jobs` and 2 more |
| [F12](F12-stdio_transport.md) | stdio transport and protocol | FAIL-FIXED | No version negotiation; replies to notifications; crashes on invalid UTF-8 and huge `Content-Length` | `test_stdio_jsonrpc_e2e_subprocess` (extended) |
| [F13](F13-http_transport.md) | HTTP transport | PASS | — | — |
| [F14](F14-npm_wrapper.md) | npm wrapper | FAIL-FIXED | `structuredContent`-only results not normalized; `close()` hard-killed the server | 7 offline tests in `npm/test/e2e.test.js` |
| [F15](F15-language_coverage.md) | Language coverage | FAIL-FIXED | `.tsx`/`.jsx` excluded by language filters; Go methods lost their receiver type | `test_language_coverage_matrix` |
| F16 | Integration | PASS | Schema descriptions and aliases ([schema-vs-model.md](schema-vs-model.md)) | — |

## Integration gate (F16)

| Check | Result |
|---|---|
| All phase diffs applied in phase order | 10 Rust diffs and 1 npm diff; end-of-file test additions merged by hand |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --all-targets -- -D warnings` | clean |
| `cargo test --release -- --test-threads=1` | 19 passed (3 unit, 16 end-to-end), 0 failed |
| `npm test` | 17 passed, 0 failed |
| Every phase's own check script against the integrated binary | all pass (F12's only miss is an informational log probe) |
| `cargo publish --dry-run --locked` | clean; crate contains only `src/`, `Cargo.*`, `README.md`, `LICENSE`, `CHANGELOG.md` |
| `npm pack` | 6 files: `bin/`, `lib/`, `index.js`, `README.md`, `package.json` |
| Tarball installed in a clean folder, binary downloaded from a local release mirror | download verified against `SHA256SUMS`, cached, second run used the cache, MCP handshake succeeded |

## Continuous integration and release rehearsal

| Check | Result |
|---|---|
| GitHub Actions CI (fmt, clippy, version check, Rust and npm tests) | green on Ubuntu, Windows and macOS |
| Release rehearsal tag `v0.1.0-rc.1` | binaries built for all five targets, pre-release published with `SHA256SUMS`, registry publishes skipped as designed |
| npm tarball installed in a clean folder, binary downloaded from the real `v0.1.0-rc.1` release | download verified and cached; MCP handshake and all 11 tools listed |

## After the v0.1.0 release

| Check | Result |
|---|---|
| Real `v0.1.0` Windows binary downloaded by the npm wrapper from GitHub Releases | checksum verified; `--version` prints `forge-mcp 0.1.0`; `workspace_open`, `symbol_find`, `symbol_references`, `code_search` and `context_build` return results |
| npm resolution-order test (F14 scope) | FAIL-FIXED. Once the real release existed, the test used the home download cache and downloaded from GitHub, so it failed on Windows and on any machine with a cached binary. It now uses a private cache dir with downloads disabled, and also checks the cache step's position in the order. Proven by running old and new tests against a pre-filled home cache: old fails, new passes. |

## Not covered here

- Linux and macOS binaries were built and tested in CI; only the Windows binary was run interactively on this machine.
- The `@modelcontextprotocol/inspector` CLI could not be run (no network at the time); a Node client exercised the same handshake.
- REA binary inspection was unavailable; SQLite contents were checked with `node:sqlite`.

Known limitations that were documented rather than changed are listed in each phase report and in the root README's Security section.
