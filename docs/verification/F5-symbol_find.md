# F5: symbol_find

**Verdict: FAIL-FIXED.** Binary before fix `ae7389b8...9064e`; shadow binary with fix `db880089...b188`. Date: 2026-11-15.

| # | Criterion | Before fix | After fix |
|---|---|---|---|
| 1 | `exact` / `prefix` / `substring` matching | PASS | PASS |
| 2 | Case-insensitive matching | PASS | PASS |
| 3 | `kind` filter for function, method, struct, class, trait, interface, module returns only that kind | PASS | PASS |
| 4 | `kind:"enum"` aliases to struct; unknown kinds (`bogus`, `variable`) are rejected | **FAIL** (each silently returned the 6 functions) | PASS |
| 5 | `file_path` with `\` or `/` | PASS | PASS |
| 6 | `language:"typescript"` | PASS | PASS |
| 7 | `limit: 2` returns 2 and `truncated: true` | PASS | PASS |
| 8 | Ordered by file path, then start line | PASS | PASS |
| 9 | Every hit has signature, line range, 64-hex content hash, heuristic evidence; doc comment extracted | PASS | PASS |
| 10 | Missing `name` returns a serde error | PASS | PASS |

**H4 confirmed.** `SymbolKind::from_str_lossy` mapped every unknown kind to `Function`.

**Fix:** `SymbolKind::from_str_strict` (`src/model.rs`) returns `None` for unknown kinds and maps `enum` to `struct` (the parser indexes enums as structs). `find_symbols` (`src/store.rs`) returns `unknown symbol kind '<k>'; expected one of: ...` for unknown kinds and treats an empty `kind` as no filter.

**Regression test:** `test_symbol_find_rejects_or_aliases_unknown_kind` in `tests/e2e_test.rs`.

Schema notes (tracked in `schema-vs-model.md`): `kind` has no enum in the schema; `limit` is clamped to 1..500 silently; `file_path` is a substring match; matching also covers `qualified_name`.
