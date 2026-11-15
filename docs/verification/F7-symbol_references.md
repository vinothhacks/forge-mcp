# F7: symbol_references

**Verdict: FAIL-FIXED.** Binary before fix `ae7389b8...9064e`; shadow binary with fix `bf8dc63d...6567`. Date: 2026-11-15.

| # | Criterion | Before fix | After fix |
|---|---|---|---|
| 1 | Hits for `verify_hmac_token` in `src/auth.rs` and `tests/auth_test.rs` with context, enclosing symbol, kind, `heuristic` evidence | PASS | PASS |
| 2 | Aliases `name`, `symbol`, `symbol_id` give identical results | PASS | PASS |
| 3 | External symbols are `unresolved` (`hashlib`); TS imports are stored under the module specifier (`./auth_client`) | PASS | PASS |
| 4 | `include_declarations: true` adds the declaration site | **FAIL** (ignored) | PASS (`reference_kind: "declaration"` first) |
| 5 | `limit: 1` returns one hit, `truncated: true` | PASS | PASS |
| 6 | Unknown name returns empty results, not an error | PASS | PASS |
| 7 | No identifier returns a clear error | PASS | PASS |
| 8 | `file_path` restricts hits to that file | **FAIL** (hits from other files returned) | PASS |

**H3 confirmed.** `include_declarations` was declared in the schema and request struct but never read.

**Fix (`src/store.rs` `symbol_references`):** filter hits by normalized `file_path`; when `include_declarations` is true, prepend the declaration from the symbols table with producer `forge-tree-sitter-symbol-reader`. Schema descriptions in `src/mcp.rs` updated to match.

**Regression test:** `test_symbol_references_include_declarations` in `tests/e2e_test.rs`. Full Rust suite on the shadow crate: 6 passed, 0 failed.

Schema vs struct: the `symbol` alias is accepted but not advertised in the schema (tracked in `schema-vs-model.md`).
