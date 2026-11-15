# F4: code_search hybrid ranking

**Verdict: PASS** (no fix, no rebuild). Binary SHA-256 `ae7389b81b8e5db4041d0b4267b3cfe27a9e66fdc62ba0a4e023e954b3e9064e`. Date: 2026-11-12.

Setup: standard multi-language fixture plus three decoy Rust files (`compute_hmac_digest_v2`, "hmac" in a comment only, unrelated). 8 files, 24 symbols, 24 chunks indexed.

| # | Criterion | Result |
|---|---|---|
| 1 | Exact identifier `compute_hmac_digest` ranks first with `ast_symbol_match` + `bm25_lexical` | PASS (rrf 0.0603 vs decoy 0.0594) |
| 2 | Natural-language query "sanitize bearer authorization header" puts `src/gateway.ts#sanitizeBearerHeader` in top 3 with a vector score | PASS (#1, vector_score 0.661) |
| 3 | `language:"go"` / `"rust"` filters | PASS |
| 4 | `path_prefix:"tests/"` | PASS |
| 5 | `include_tests:false` excludes `tests/auth_test.rs` | PASS |
| 6 | `limit:1` yields one result and `truncated:true` | PASS |
| 7 | FTS special characters and operators do not error | PASS (tokens quoted before MATCH) |
| 8 | Stopword query "the" does not panic | PASS (0 results) |
| 9 | Empty query `""` | Recorded: empty results, no error |
| 10 | `rrf_score` monotone; `evidence.len == results.len`; `content_hash == sha256(content)` | PASS |
| 11 | Unknown workspace_id | PASS (JSON-RPC -32000 "Workspace ... not found") |

Schema vs `CodeSearchRequest`: fields match 1:1. Notes: `path_prefix` also matches as a substring (`file_path.contains`), and `limit` is clamped to 1..100 without the schema saying so.

Hand-off to F9: `is_test_path_or_symbol` uses `contains("test_")`, so `src/latest_build.rs` would be treated as a test file (H5).
