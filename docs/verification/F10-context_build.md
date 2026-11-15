# F10: context_build

**Verdict: FAIL-FIXED.** Binary before fix `ae7389b8...9064e`; shadow binary with fix `b27b20bb...2301`. Date: 2026-11-15. Runtime checks: 19/25 before, 25/25 after.

| # | Criterion | Before fix | After fix |
|---|---|---|---|
| 1 | `token_budget: 64` stays within budget, returns at least one snippet, `truncated: true` | PASS | PASS |
| 2 | `token_budget: 4000` expands the graph, no duplicate ranges or symbols, token sum matches, one evidence per snippet | PASS | PASS |
| 3 | Budget clamped to 64..128000 without error | PASS | PASS |
| 4 | `include_tests: false` excludes `tests/` | PASS | PASS |
| 5 | `language` and `path_prefix` filters also apply to graph-expanded snippets | **FAIL** (`path_prefix: "src/"` leaked `tests/auth_test.rs` through 1-hop expansion) | PASS |
| 6 | `require_current` on a stale snapshot returns an error | **FAIL** (proceeded) | PASS |
| 6 | `require_current_strict` on a stale snapshot returns an error | PASS | PASS |
| 6 | `allow_stale` or omitted proceeds with a staleness limitation | PASS | PASS |
| 6 | Unknown `snapshot_policy` rejected | **FAIL** (silently accepted) | PASS |
| 6 | Schema lists `snapshot_policy` values as an enum | **FAIL** | PASS |
| 7 | Missing `query` returns a serde error | PASS | PASS |

**H2 confirmed.** The schema documented `require_current` and `allow_stale`, but the code only acted on the undocumented `require_current_strict`.

**Fix (`src/search.rs`, `src/mcp.rs`):** `require_current` and `require_current_strict` error when stale, `allow_stale` (default) proceeds with a limitation, anything else is rejected; the schema has the enum and the budget range. Graph expansion now applies the same `language` and `path_prefix` filters as the primary hits. Budget limits are named constants.

**Regression tests:** `test_context_build_snapshot_policy_values`, `test_context_build_graph_expansion_respects_filters`.

Observation: ranking ties make the order of equal-score hits vary between runs.
