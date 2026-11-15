# F6: symbol_read

**Verdict: FAIL-FIXED.** Binary before fix `ae7389b8...9064e`; shadow binary with fix `3d2b1051...41ae`. Date: 2026-11-15.

Fixture adds a Python `def verify(token)` so `verify` exists as a Rust trait declaration, a Rust impl method and a Python function.

| # | Criterion | Before fix | After fix |
|---|---|---|---|
| 1 | Read by `symbol_id`: body, doc comment, signature, line range | PASS | PASS |
| 2 | Read by `name` returns the same symbol | PASS | PASS |
| 3 | `file_path` disambiguates Rust vs Python (backslashes accepted) | PASS, no ambiguity notice | PASS, ambiguity listed in `limitations` |
| 4 | `name:"verify", file_path:"src/auth.rs"` returns `TokenValidator::verify` with parent `TokenValidator` | **FAIL** (returned trait declaration `Verifier::verify`) | PASS |
| 5 | Body equals the file's lines and byte range | PASS | PASS |
| 6 | `context_lines` 0, default 3, and 999 (clamped) | PASS | PASS |
| 7 | Unknown id gives -32000 "not found" | PASS | PASS (message names the id and snapshot) |
| 8 | No id and no name gives a clear error | weak message | PASS |
| 9 | `content_hash` | PASS with caveat | PASS with caveat |

**Cause:** name lookup took the first match by `(file_path, start_line)`, so a one-line trait declaration always beat the impl that defines the method.

**Fix (`src/store.rs`):** new `resolve_symbol_candidates` ranks multi-line definitions before one-line declarations, filters by `file_path` first, returns alternatives, and gives specific errors. `read_symbol` reports ambiguity in `limitations`. `resolve_symbol_in_snapshot` keeps its signature for other callers.

**Regression test:** `test_symbol_read_ambiguous_name_disambiguation_and_errors` in `tests/e2e_test.rs`.

Caveats documented, not changed: `content_hash` covers the parsed node's byte range, so it differs from `sha256(body)` for indented symbols; `enclosing_scope` is ±N lines, not the enclosing impl/class.
