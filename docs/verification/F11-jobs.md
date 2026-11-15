# F11: job_status and job_cancel

**Verdict: FAIL-FIXED.** Binary before fix `ae7389b8...9064e`; shadow binary with fix `183ae7e8...990a`. Date: 2026-11-15.

| # | Criterion | Before fix | After fix |
|---|---|---|---|
| 1 | `job_status{job_id}` after open: completed, 100%, processed = total = file count, snapshot id | PASS | PASS |
| 2 | `job_status{workspace_id}` returns the latest job | PASS | PASS |
| 3 | Re-opening creates a second completed job | PASS | PASS |
| 4 | No arguments or unknown id give clear errors | PASS | PASS |
| 5 | `job_cancel` on a completed job explains that nothing was cancelled | **FAIL** (silent) | PASS ("Job already completed; nothing to cancel.") |
| 6 | `job_cancel{}` returns a serde error | PASS | PASS |
| B | Real cancel during parsing (3000 files over HTTP): open fails "was cancelled", job `cancelled`, no workspace or snapshot rows, server healthy | PASS | PASS |
| B2 | Cancel after parsing, during embedding/commit | **FAIL** (cancel reported `cancelled`, but indexing then completed and activated the snapshot) | PASS (rolled back, no snapshot) |

**H10 confirmed** on both parts: completed-job cancel was silent, and real cancellation had never been exercised through the tools.

**Fix (`src/store.rs`):** `cancel_job` records the cancel request before reading the database (the read can block behind the indexer's write transaction) and adds a limitation describing the outcome; the embedding loop and the commit check for cancellation; the job is marked completed inside the same transaction; new `abort_cancelled_job` helper.

**Regression tests:** `test_job_cancel_terminal_and_unknown_jobs`, `test_job_cancel_mid_index_aborts_open`, `test_job_cancel_after_parse_phase_rolls_back_snapshot` (fails on the old code).

Known limitations (not changed): progress fields stay 0 while a job runs; the file scan happens before the job row exists, so a very early cancel is not possible; a job interrupted by a crash stays `running`; HTTP `workspace_open` blocks until indexing finishes.
