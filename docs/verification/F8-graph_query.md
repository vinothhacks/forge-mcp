# F8: graph_query

**Verdict: FAIL-FIXED.** Binary before fix `ae7389b8...9064e`; shadow binary with fix `dd275fb0...33ae9`. Date: 2026-11-15. The Go `children` case was handed to F15 (parser) and is tracked there.

| # | Criterion | Before fix | After fix |
|---|---|---|---|
| 1 | `callers` of `compute_hmac_digest` at depth 1 and 2, each hop with `depth` | PASS | PASS |
| 2 | `callees` of `verify_hmac_token` | PASS | PASS |
| 3 | `imports` of `src/gateway.ts` and `tests/auth_test.rs` | PASS | PASS |
| 4 | `importers` of `src/auth.rs` is only `tests/auth_test.rs` | **FAIL** (`src/gateway.ts` matched via substring "auth" in `./auth_client`) | PASS |
| 5 | `children` of `TokenValidator` and `ApiGateway` | PASS | PASS |
| 5 | `children` of Go `HttpRouter` | FAIL | Fixed in F15 (parser) |
| 6 | `implements` for Rust, TypeScript and Python | PASS | PASS |
| 7 | `path` found (2 hops) and not found (empty) | PASS | PASS |
| 8 | Aliases `direction`, `kind`, `relation`, `name`, `symbol` | PASS | PASS |
| 9 | `max_depth` clamped to 1..8 | PASS | PASS |
| 10 | `limit: 1` sets `truncated` | PASS | PASS |
| 11 | Unknown `query_kind` | **FAIL** (silently ran `callers`) | PASS (-32000 "unknown query_kind") |
| 12 | No symbol and no file gives a clear error | PASS | PASS |
| 13 | `limitations` mention unresolved targets | **FAIL** | PASS |

**H9 confirmed.** `relation` and `symbol` were accepted but not in the schema; both are now advertised.

**Fix (`src/graph.rs`, `src/mcp.rs`):** `parse_query_kind` rejects unknown kinds; a hop counts as unresolved when its target symbol is absent from the snapshot; `matches_import_target` compares the import specifier's module segments against the file stem exactly instead of by substring (also used by `impact_analyze`).

**Regression test:** `test_graph_query_strict_kind_aliases_and_importers` in `tests/e2e_test.rs`.
