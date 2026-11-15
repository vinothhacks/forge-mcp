# F9: impact_analyze

**Verdict: FAIL-FIXED.** Binary before fix `ae7389b8...9064e`; shadow binary with fix `4fb299ff...895b`. Date: 2026-11-15.

Fixture adds `src/latest_build.rs` (non-test file that calls `compute_hmac_digest`) and `src/contest_rules.rs`.

| # | Criterion | Before fix | After fix |
|---|---|---|---|
| 1 | `compute_hmac_digest`, depth 3: direct `[verify_hmac_token, build_latest]`, transitive includes `verify` and the test fn, `impacted_test_files == [tests/auth_test.rs]` | **FAIL** (`src/latest_build.rs` listed as a test file) | PASS |
| 2 | By `file_path: src/auth.rs` | PASS | PASS |
| 3 | By `symbol_id` | **FAIL** (same as 1) | PASS |
| 4 | Stale `symbol_id` + valid name: name wins; bogus id alone gives a clear error | PASS / misleading error | PASS |
| 5 | `max_depth: 1` gives no transitive dependents | PASS | PASS |
| 6 | No target gives a clear error | PASS | PASS |
| 7 | One evidence entry per dependent, `heuristic`, limitation present | PASS | PASS |
| extra | `code_search include_tests:false` keeps `src/latest_build.rs` | **FAIL** | PASS |

**H5 confirmed.** `is_test_path_or_symbol` used `contains("test_")`, which matches "la**test_**build".

**Fix:** `is_test_path_or_symbol` (`src/search.rs`) now matches test directories by path segment (`tests`, `test`, `__tests__`, `spec`), test file names by stem (`test_*`, `*_test`, `*.test`, `*.spec`, `*_spec`), and test symbols by `test_` prefix or Go `TestXxx`. `impact_analyze` (`src/graph.rs`) returns a specific error for an unknown `symbol_id`.

**Regression tests (`src/search.rs`):** `test_is_test_path_does_not_match_latest_prefix`, `test_is_test_path_matches_test_dirs_and_files`, `test_is_test_symbol_prefixes`.

Routed to F8: `matches_import_target` used a substring test, so `./auth_client` matched `src/auth.rs`.
