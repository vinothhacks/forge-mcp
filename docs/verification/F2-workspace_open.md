# F2: workspace_open

**Verdict: FAIL-FIXED.** Binary before fix `ae7389b8...9064e`; shadow binary with fix `9a64b0cc...8e5f`. Date: 2026-11-15.

| Criterion | Before fix | After fix |
|---|---|---|
| Fresh open: `reused_snapshot:false`, generation 1 | PASS | PASS |
| Re-open: `reused_snapshot:true`, same snapshot | PASS | PASS |
| `force_reindex`: new snapshot, generation 2 | PASS | PASS |
| Path outside `--allow-root` rejected ("is outside allowed roots") | PASS | PASS |
| `..` and `.` segments that resolve inside the root are accepted | PASS | PASS |
| No `--allow-root`: any path opens | PASS | PASS |
| Evidence producer: `forge-tree-sitter-indexer` fresh, `forge-snapshot-coordinator` reused | PASS | PASS |
| `.gitignore`d files not indexed (workspace is not a git repo) | **FAIL** (both indexed) | PASS |
| `node_modules/` skipped | PASS | PASS |
| Files over 2 MiB skipped | PASS | PASS |
| Files containing NUL bytes skipped | PASS | PASS |
| Hidden directories indexed | PASS | PASS |
| Default `workspace_id` shape `<slug>-<8hex>` | PASS | PASS |

**H1 confirmed.** `ignore::WalkBuilder` defaults to `require_git(true)`, so `.gitignore` was silently ignored for any directory without `.git/`. The tool description promises it respects `.gitignore`.

**Fix:** add `.require_git(false)` to the walker in `collect_workspace_files` (`src/store.rs`).

**Regression test:** `test_workspace_open_respects_gitignore_outside_git_repo` in `tests/e2e_test.rs`.

Schema vs `WorkspaceOpenRequest`: match exactly.
