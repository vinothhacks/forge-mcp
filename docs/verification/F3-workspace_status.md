# F3: workspace_status freshness

**Verdict: PASS** (no fix, no rebuild). Binary SHA-256 `ae7389b81b8e5db4041d0b4267b3cfe27a9e66fdc62ba0a4e023e954b3e9064e`. Date: 2026-11-12.

| # | Criterion | Result |
|---|---|---|
| 1 | After open: `freshness: current`, `changed_files: []` | PASS |
| 2 | Edit `src/auth.rs`: `stale`, file listed, limitation names the snapshot and file | PASS |
| 3 | Add `src/new.go`: listed | PASS |
| 4 | Delete `src/pipeline.py`: listed | PASS |
| 5 | Revert all three: `current` again (hash-based) | PASS |
| 6 | mtime-only touch: still `current` | PASS |
| 7 | While stale, `code_search` and `symbol_find` also report `stale` | PASS |
| 8 | Workspace root renamed away: `stale` with `<workspace_root_missing>`; renamed back: `current` | PASS |
| 9 | Re-open after edits: `index_generation` 1 to 2, `current` | PASS |
| 10 | Unknown workspace_id: JSON-RPC -32000 "Workspace ... not found" | PASS |
| - | No panics in stderr across all sessions | PASS |

Schema vs `WorkspaceStatusRequest` and `WorkspaceOpenRequest`: match exactly.

Notes (cosmetic, not changed): stale-limitation wording differs between tools (`workspace_status`, `code_search`, `symbol_find`); added and deleted files share one `changed_files` list; missing-root limitation reads "1 file(s) changed on disk (<workspace_root_missing>)".
