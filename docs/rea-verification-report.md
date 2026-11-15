# Forge — Capability Specification & REA MCP Verification Report

> **Superseded (2026-11-15).** This report describes the Oct 26 build. The code changed afterwards, and every feature was re-verified from the current source. See [verification/README.md](verification/README.md) for the current results, including the defects found and fixed.


**Project**: Forge (`forge-mcp` / `forge-codebase-mcp` v0.1.0)  
**Date**: 2026-10-26  
**Status**: Verified (100% of Rust unit/E2E tests, stdio/HTTP transport E2E tests, npm CLI wrapper E2E tests, and REA artifact inspections completed).

---

## 1. Executive Summary

Forge is a self-hosted Rust codebase-search and code-intelligence MCP server unifying capability layers from Serena, CodeGraphContext, Augment Context Engine, and Code Scalpel under a single snapshot-consistent architecture.

This document records:
1. **Implementation-Neutral Capability Specifications** across all 11 exposed MCP tools and 4 capability layers, including provenance, resolution classification (`compiler_resolved`, `heuristic`, `unresolved`), freshness policies, and language boundaries.
2. **REA MCP Artifact Verification Evidence** across the compiled Windows PE32+ binary (`target/release/forge-mcp.exe`), the persistent SQLite + FTS5 + Vector index database (`.forge/verify-index.db`), and the `forge-codebase-mcp` Node.js CLI/SDK wrapper (`npm/`).
3. **End-to-End Test Results** across Rust (`tests/e2e_test.rs`), local `stdio` (newline-delimited and `Content-Length` framed JSON-RPC 2.0), self-hosted Axum HTTP (`GET /health`, `POST /mcp` with Bearer authentication and workspace path allowlisting), and the npm CLI wrapper (`npm/test/e2e.test.js`).

---

## 2. Implementation-Neutral Capability Specification

### 2.1 Shared Evidence & Snapshot Envelope Contract

Every MCP tool returns a standardized `ResponseEnvelope<T>` (serialized both in `content[0].text` and `structuredContent`):

```json
{
  "snapshot_id": "snap-1-6b81d4c2",
  "freshness": "current",
  "results": [...],
  "evidence": [
    {
      "repository_id": "repo-94a01f2b8c31",
      "workspace_id": "ws-multilang",
      "snapshot_id": "snap-1-6b81d4c2",
      "file_path": "src/auth.rs",
      "source_range": {
        "start_line": 18,
        "start_column": 1,
        "end_line": 20,
        "end_column": 2,
        "start_byte": 356,
        "end_byte": 452
      },
      "content_hash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      "producer": "forge-tree-sitter-symbol-index",
      "resolution_kind": "heuristic",
      "observed_at": "2026-10-26T15:20:00Z"
    }
  ],
  "limitations": [
    "Symbols extracted syntactically via Tree-sitter."
  ],
  "truncated": false,
  "next_cursor": null
}
```

### 2.2 Tool Surface & Behavioral Contracts

| Tool Name | Layer | Input Parameters | Output / Behavior | Freshness & Error Semantics |
|---|---|---|---|---|
| `workspace_open` | Workspace & Snapshot Coordinator | `path` (req), `workspace_id`, `force_reindex` | Canonicalizes path (stripping `\\?\` on Windows), enforces `--allow-root` policy, walks files respecting `.gitignore`, computes SHA-256 per file and root Merkle hash, extracts AST symbols/refs/edges/chunks via Tree-sitter, embeds chunks, and commits atomically to SQLite. Reuses active snapshot if `root_hash` is unchanged and `force_reindex != true`. | Rejects paths outside `--allow-root` with JSON-RPC error `-32000`. Returns `freshness: "current"`. |
| `workspace_status` | Workspace & Snapshot Coordinator | `workspace_id` (req) | Compares indexed `file_versions` SHA-256 hashes against live filesystem state. Reports `file_count`, `symbol_count`, `reference_count`, `relationship_count`, `chunk_count`, `index_generation`, and `changed_files`. | Transitions `freshness` from `"current"` to `"stale"` immediately upon any file edit, addition, or deletion on disk. |
| `code_search` | Hybrid Retrieval Engine | `workspace_id` (req), `query` (req), `language`, `path_prefix`, `limit`, `include_tests` | Combines SQLite FTS5 BM25 lexical scoring (`chunks_fts`), AST symbol name/signature/doc matching, and 128-dim L2-normalized vector cosine similarity via Reciprocal Rank Fusion ($k = 60$). | Reports per-hit `rrf_score`, `lexical_score`, `symbol_score`, `vector_score`, and `match_reasons`. |
| `symbol_find` | Symbol Intelligence | `workspace_id` (req), `name` (req), `match_mode` (`exact`, `prefix`, `substring`), `kind`, `file_path`, `language`, `limit` | Queries indexed symbols (`function`, `struct`, `class`, `interface`, `method`, `trait`, `module`) with exact line/column/byte spans, signature, visibility, and doc comments. | Preserves `ResolutionKind::Heuristic` for Tree-sitter syntactic extraction. |
| `symbol_read` | Symbol Intelligence | `workspace_id` (req), `symbol_id`, `name`, `file_path`, `context_lines` | Retrieves full symbol body from snapshot `file_versions`, surrounding `enclosing_scope`, and resolved `parent_symbol` metadata. | Fails with structured error if symbol is absent from active snapshot. |
| `symbol_references` | Symbol Intelligence | `workspace_id` (req), `symbol_id`, `symbol_name`/`name`, `file_path`, `include_declarations`, `limit` | Returns all call, import, and inheritance reference sites across the workspace with surrounding context lines and enclosing symbol qualification. | Distinguishes workspace-matched references (`heuristic`) from external symbols (`unresolved`). |
| `graph_query` | Relationship & Graph Engine | `workspace_id` (req), `query_kind` (`callers`, `callees`, `imports`, `importers`, `children`, `implements`, `path`), `symbol_id`, `symbol_name`, `file_path`, `target_symbol_name`, `max_depth`, `limit` | Performs bounded BFS traversal ($1 \le \text{max\_depth} \le 8$) over `relationships` adjacency indices, or shortest-path BFS when `query_kind == "path"`. | Explicitly reports unresolved dynamic/external targets in `limitations`. |
| `impact_analyze` | Blast-Radius Analyzer | `workspace_id` (req), `symbol_id`, `symbol_name`, `file_path`, `max_depth` | Computes direct ($d = 1$) and transitive ($d > 1$) dependents, deduplicated `affected_files`, and `impacted_test_files` (detected via path/symbol test heuristics). | Separates direct vs. transitive blast radius and highlights affected test suites. |
| `context_build` | Token-Budgeted Context Engine | `workspace_id` (req), `query` (req), `token_budget`, `include_tests`, `snapshot_policy`, `language`, `path_prefix` | Three-pass context assembly: (1) seeds top primary hybrid hits, (2) expands 1-hop callers/callees/implements neighbors, (3) fills remaining budget with additional primary hits while strictly enforcing `total_tokens_used <= token_budget`. | Enforces `snapshot_policy = "require_current_strict"` by rejecting stale snapshots when requested. |
| `job_status` | Async Job Tracker | `job_id`, `workspace_id` | Returns `JobRecord` (`queued`, `running`, `completed`, `cancelled`, `failed`), `progress_pct`, `processed_files`, `total_files`, and linked `snapshot_id`. | Tracks both initial indexing and snapshot reuse jobs. |
| `job_cancel` | Async Job Tracker | `job_id` (req) | Marks the job ID in the cancellation set and transitions active jobs to `cancelled`. | Checked between file parse iterations during indexing. |

### 2.3 Language Coverage Matrix

| Language | Extensions | Grammar Crate | Extracted Symbol Kinds | Extracted Relationships |
|---|---|---|---|---|
| **Rust** | `.rs` | `tree-sitter-rust` `0.23.3` | `function`, `method` (in `impl`), `struct`/`enum`, `trait`, `module` | `calls`, `imports` (`use`), `contains`, `implements` (`impl Trait for Type`) |
| **TypeScript / JS** | `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs` | `tree-sitter-typescript` `0.23.2`, `tree-sitter-javascript` `0.23.1` | `function` (decl & arrow/var), `class`, `interface`, `method`, `module` | `calls`, `imports` (`import ... from`), `contains`, `implements`, `inherits` (`extends`) |
| **Python** | `.py`, `.pyi` | `tree-sitter-python` `0.23.6` | `function`, `method` (in `class`), `class` | `calls`, `imports` (`import` / `from ... import`), `contains`, `implements` (superclasses) |
| **Go** | `.go` | `tree-sitter-go` `0.23.4` | `module` (`package`), `function`, `method` (with receiver linking), `struct`, `interface` | `calls`, `imports` (`import_spec`), `contains` |

---

## 3. REA MCP Artifact Verification Evidence

All production artifacts built in `D:\research 1\mcp\codebase` were inspected using the `user-rea` MCP server.

### 3.1 Compiled Rust Binary Verification (`target/release/forge-mcp.exe`)

- **Artifact Path**: `D:\research 1\mcp\codebase\target\release\forge-mcp.exe`
- **SHA-256 Digest**: `f391592e44aded713275a593067e601b00fd51c97acbe5a4023be71885e0f1f0`
- **Format & Architecture**: `pe32-plus` (`amd64` / `x86_64`, 64-bit, little-endian, machine `0x8664` / `34404`)
- **File Size**: `24,409,771` bytes (~23.28 MiB, including statically linked bundled SQLite + FTS5 and 6 Tree-sitter C grammars)

#### A. PE Import Table (`user-rea:inspect_pe_imports`)
- **Evidence ID**: `ev_c5630f64c41f3a85fbe8710bc9f89803dfc22cbdec60d9d2f7d37601acf4db12`
- **Coverage**: `complete` (`15` import descriptors across `9` unique system DLLs, `278` total imported functions, `0` delay-loaded DLLs)
- **PE Imphash (MD5)**: `164df3a2b7e2cdcccf672cef3cad5be3`
- **Import Directory**: RVA `0x113b000` (`18,067,456`), file offset `18,046,976`, size `11,188` bytes
- **Observed System DLL Dependencies**:
  - `KERNEL32.dll` / `kernel32.dll` (filesystem I/O, threads, synchronization, console handles, memory mapping for SQLite & Tokio)
  - `ws2_32.dll` / `WS2_32.dll` (`WSASocketW`, `WSAPoll`, `WSARecv`, `WSASend`, `bind`, `listen`, `accept`, `getaddrinfo` for Axum HTTP server & Reqwest client)
  - `bcrypt.dll` (`BCryptGenRandom`) & `bcryptprimitives.dll` (`ProcessPrng`) (cryptographic RNG for UUID v4 and Rustls TLS)
  - `ntdll.dll` (`NtCreateFile`, `NtReadFile`, `NtWriteFile`, `NtCancelIoFileEx`, `NtCreateNamedPipeFile` for Tokio async I/O)
  - `api-ms-win-core-synch-l1-2-0.dll` (`WaitOnAddress`, `WakeByAddressAll`, `WakeByAddressSingle` for Rust `std::sync` primitives)
  - `ADVAPI32.dll` (`SystemFunction036`)
  - `USERENV.dll` (`GetUserProfileDirectoryW`)
  - `msvcrt.dll` (C runtime intrinsics for bundled `libsqlite3` and `tree-sitter` C parsers)
- **Verification Conclusion**: Zero third-party or non-system dynamic libraries are required at runtime; SQLite (`rusqlite` bundled) and all Tree-sitter grammars are self-contained inside the single executable.

#### B. Native Export Table (`user-rea:inspect_native_exports`)
- **Evidence ID**: `ev_df3cb90296665e7bb92bf20c4995295227050e7d3a2834d2a0e627e73b7dd1a0`
- **Coverage**: `complete` (`6` exported symbols, ordinal base `1`, RVA `18,063,360`)
- **Verified Tree-sitter Language Entrypoints**:
  1. Ordinal `1` (`0x1409388f0`): `tree_sitter_go`
  2. Ordinal `2` (`0x140938900`): `tree_sitter_javascript`
  3. Ordinal `3` (`0x140938e60`): `tree_sitter_python`
  4. Ordinal `4` (`0x140939ac0`): `tree_sitter_rust`
  5. Ordinal `5` (`0x14093a160`): `tree_sitter_tsx`
  6. Ordinal `6` (`0x14093a720`): `tree_sitter_typescript`

#### C. Rust Compiler Metadata & PE Load Config (`user-rea:inspect_rust_binary`, `user-rea:inspect_pe_load_config`, `user-rea:open_binary` -> `close_binary`)
- **Rust Binary Evidence ID**: `ev_664f91a11da9d69504e5f51f4b4a25350efaf4480f02706cbf9436d5f2666ebd`
  - **Inferred `rustc` Commit Hash**: `48a229ceaefd4985c50990b14116b6d856af0985` at file offset `10,174,808` (matches `rustc 1.98.1 (48a229cea 2026-09-01)`).
- **PE TLS / Load Config Evidence ID**: `ev_111d01b8216f25c479d1839ec65e9f645d080da30a96da6aa9316110957c8030`
  - **TLS Directory**: RVA `12,102,400` (file offset `12,094,208`, size `40`), `.tls` raw data at offset `18,058,752`, with `2` registered `.text` TLS callbacks (`0x1407be3f0`, `0x1407be3c0`).
- **Binary Session Lifecycle (`open_binary` -> `binary_overview` -> `close_binary`)**:
  - `open_binary` validated the PE executable identity (`sha256: f391592e44aded713275a593067e601b00fd51c97acbe5a4023be71885e0f1f0`).
  - `binary_overview` confirmed that external disassemblers (`ghidra`, `ida`, `hopper`, `kuna`) are not configured on this host, as expected in a lightweight local environment, and `close_binary` cleanly released the session.

---

### 3.2 SQLite Index Database Verification (`.forge/verify-index.db`)

A persistent self-index of the entire Forge codebase (`workspace_id = "forge-self-index"`) was generated and WAL-checkpointed at `D:\research 1\mcp\codebase\.forge\verify-index.db`.

#### A. REA Byte Entropy & File Integrity (`user-rea:inspect_byte_entropy`)
- **Evidence ID**: `ev_6be2b0a4601e41375757f48235eb92a5d4e3b22f185273549052fad15e349fee` (parent `ev_2124b6cfe9317f9437998bcaf2d7351aa739d18d9202d2383ebbe58caf529e83`)
- **Artifact Path**: `D:\research 1\mcp\codebase\.forge\verify-index.db`
- **SHA-256 Digest**: `e3229e495a504bf9f4cdca02714e9955ad46bf46f4880e0a52c3c1c7b3756ee8`
- **Database Size**: `7,761,920` bytes (`1,895` 4-KiB SQLite pages)
- **File-Wide Shannon Entropy**: `5.6769` bits/byte (characteristic of structured UTF-8 source text, JSON float32 embedding vectors, and B-tree/FTS5 inverted index pages).
- **Note on `user-rea:inspect_sqlite_database`**: REA's `inspect_sqlite_database` worker reported `capability_unavailable` because the MCP server process was launched under an earlier Node runtime in PATH lacking `DatabaseSync.limits`, whereas the workspace Node `v24.19.0` (`C:\nvm4w\nodejs\node.exe`) provides native `node:sqlite` (`DatabaseSync` with `limits: true`). Direct verification with `node:sqlite` on `.forge/verify-index.db` confirmed all 12 core tables and 11 indices.

#### B. Verified Schema Tables & Populated Row Counts

| Table Name | Table Kind | Row Count | Purpose |
|---|---|---:|---|
| `repositories` | Relational Table | `1` | Canonical repository identity (`repo-2e06fd8d9676`) |
| `workspaces` | Relational Table | `1` | Active workspace state (`forge-self-index`, `index_generation = 2`) |
| `snapshots` | Relational Table | `2` | Immutable content-hashed workspace snapshots |
| `file_versions` | Relational Table | `28` | Indexed source files (`src/*.rs`, `tests/e2e_test.rs`, `npm/*.js`) with SHA-256 hashes |
| `symbols` | Relational Table | `408` | Extracted Rust & JavaScript AST symbols with byte/line spans and signatures |
| `references` | Relational Table | `4,236` | Call, import, and inheritance reference sites across snapshots |
| `relationships` | Relational Table | `4,404` | Directed graph edges (`calls`, `imports`, `contains`, `implements`) |
| `chunks` | Relational Table | `408` | Source-backed retrieval chunks with token estimates |
| `chunks_fts` | FTS5 Virtual Table | `408` | `unicode61` BM25 lexical index over chunk content and symbol names |
| `symbols_fts` | FTS5 Virtual Table | `408` | `unicode61` BM25 lexical index over symbol names, signatures, and doc comments |
| `vector_embeddings` | Relational Table | `408` | 128-dimensional L2-normalized vectors (`forge-local-hash-v1`) |
| `jobs` | Relational Table | `3` | Completed indexing job records (`progress_pct = 100`) |

All 11 secondary B-tree indices (`idx_file_versions_snapshot`, `idx_symbols_snapshot_name`, `idx_symbols_snapshot_file`, `idx_symbols_snapshot_kind`, `idx_references_snapshot_name`, `idx_references_snapshot_target`, `idx_relationships_from`, `idx_relationships_to`, `idx_relationships_names`, `idx_chunks_snapshot_file`, `idx_vectors_snapshot`) and FTS5 shadow tables (`*_config`, `*_content`, `*_data`, `*_docsize`, `*_idx`) are present and verified.

---

### 3.3 NPM JavaScript Application Verification (`npm/`)

The `forge-codebase-mcp` Node.js package (`D:\research 1\mcp\codebase\npm`) was reconstructed and verified using `user-rea:analyze_javascript_application` and `user-rea:inspect_analysis_view`.

- **Summary Evidence ID**: `ev_c7320e96df88960bc799a273d82d54e9bbe28a9da2372109f62fd8a086d08ab1`
- **Parent Evidence ID**: `ev_6222672f8ab2f75c63405fd1607ef255bae75874fb700ddfb65a58c5bbb7e11d`
- **Module View Evidence ID**: `ev_922512ee1d72f3f0c9a4d3c650c9d91f7a80f8535b1dd76fe6712c1f6cf0babc`
- **Artifact Digest (SHA-256)**: `4d12f226a5f30d20e7e7df7e1c921590825aaef8e6bcff9865f2d146f44d1b19`
- **Static Parsing & Integrity Metrics**:
  - **Relevant Files Examined**: `4` (`package.json`, `index.js`, `bin/forge-mcp.js`, `test/e2e.test.js`)
  - **Parsed JavaScript Files**: `3` (`parse_failures: 0`, `invalid_utf8_files: 0`, `skipped_javascript_files: 0`)
  - **Total Text Bytes Read**: `45,574` bytes
  - **Visited AST Nodes**: `5,389`
  - **Recovered Module Graph Nodes**: `36` (`complete-within-view`)
- **Observed File Digests & Exports**:
  - `npm/index.js` (`sha256: d63b7ea1d33a78f89d3f9555acae6491e809bd1b053ceba4b40be0a677b4e2e2`):
    - Exports: `getBinaryName`, `findBinaryInPath`, `resolveBinaryPath`, `spawnForgeServer`, `normalizeToolResult`, `ForgeMcpClient`.
    - Verified built-in imports: `node:fs`, `node:path`, `node:child_process`, `node:readline`, `node:events`.
  - `npm/bin/forge-mcp.js` (`sha256: 2b8d73fd2b3777a3f1dfa2e378c260af0e35259a22c2f5d56e98ded0616d2251`):
    - Exports: `main`.
    - Verified imports: `node:path`, `../index.js` (`resolveBinaryPath`, `spawnForgeServer`).
  - `npm/test/e2e.test.js` (`sha256: 3a3c472acd13306a6ab9fc3a1fd74454353358b2624cc8365405c164e025061b`):
    - Verified imports: `node:test`, `node:assert/strict`, `node:fs`, `node:os`, `node:path`, `node:net`, `node:child_process`, `../index.js`.
- **Recovered Semantic Relation Families**:
  - `data-flow`: `2,271` retained relations
  - `object-flow`: `376` retained relations
  - `closure`: `186` retained relations
  - `call-flow`: `140` retained relations
  - `promise-ownership`: `41` retained relations
  - `event`: `24` retained relations (`EventEmitter` handlers on child process and readline)
  - `request`: `18` retained relations (`fetch` calls in `ForgeMcpClient` HTTP transport)
  - `boundary`: `9` retained relations (`JSON.parse` / `JSON.stringify` framing)
  - `timer`: `6` retained relations (`setTimeout` / `clearTimeout` RPC deadline guards)
  - `child-process`: `5` retained relations (`spawn` / `spawnSync` for `forge-mcp` binary and `cargo build --release`)

---

## 4. End-to-End Test Suite Results

### 4.1 Rust E2E Test Suite (`cargo test`)

All 5 integration/E2E tests in `tests/e2e_test.rs` passed (`0` failures):

1. `test_multilang_indexing_and_snapshot_freshness_lifecycle` — **PASS**
   - Indexes a 5-file workspace across Rust (`src/auth.rs`), TypeScript (`src/gateway.ts`), Python (`src/pipeline.py`), Go (`src/router.go`), and Rust tests (`tests/auth_test.rs`).
   - Verifies snapshot reuse when unchanged (`reused_snapshot: true`), exact symbol extraction per language, immediate transition to `freshness: "stale"` when `src/auth.rs` is modified on disk, and transition back to `freshness: "current"` (`index_generation = 2`) upon re-indexing.
2. `test_hybrid_search_symbol_intelligence_graph_impact_and_context_budget` — **PASS**
   - Exercises `code_search` (BM25 + AST symbol + vector RRF), `symbol_find`, `symbol_read` (with doc comments), `symbol_references` (cross-file source + test references), `graph_query` (`callers`, `callees`, `children`, `implements`, `imports`, `importers`, `path` shortest path), `impact_analyze` (direct + transitive dependents and `impacted_test_files`), `context_build` (both strict 64-token budget cap and generous 4000-token 1-hop graph expansion), `job_status`, and `job_cancel`.
3. `test_stdio_jsonrpc_e2e_subprocess` — **PASS**
   - Spawns the compiled `forge-mcp` binary over `stdio`, verifies both newline-delimited JSON-RPC (`initialize`, `tools/call`) and `Content-Length: <N>\r\n\r\n` framed JSON-RPC (`tools/list`), and asserts all 11 tools are advertised and functional.
4. `test_http_server_auth_allowlist_and_full_tool_invocation` — **PASS**
   - Launches the Axum HTTP server on an ephemeral port with Bearer token authentication and `--allow-root` restriction.
   - Verifies `GET /health` returns `200 OK`, unauthenticated `POST /mcp` returns `401 Unauthorized`, invalid Bearer token returns `401 Unauthorized`, disallowed workspace path returns a structured JSON-RPC error (`outside allowed roots`), and authenticated requests execute `workspace_open` and `code_search` end-to-end.
5. `test_generate_persistent_verification_sqlite_db` — **PASS**
   - Indexes the Forge repository into `.forge/verify-index.db` and checkpoints the WAL for offline REA verification.

### 4.2 NPM CLI Wrapper & SDK E2E Suite (`node --test npm/test/e2e.test.js`)

All 3 E2E tests in `npm/test/e2e.test.js` and direct CLI invocation (`node npm/bin/forge-mcp.js --help`) passed (`0` failures):

1. `resolveBinaryPath honors FORGE_MCP_BIN and resolves workspace binary` — **PASS** (`4.49ms`)
2. `ForgeMcpClient over stdio exercises all MCP tools end-to-end` — **PASS** (`948.34ms`)
   - Spawns `npm/bin/forge-mcp.js` over `stdio` and executes `initialize`, `listTools`, `workspace_open`, `workspace_status`, `code_search`, `symbol_find`, `symbol_read`, `symbol_references`, `graph_query`, `impact_analyze`, `context_build`, and `job_status`.
3. `ForgeMcpClient over HTTP enforces Bearer auth and exercises all MCP tools` — **PASS** (`1739.68ms`)
   - Spawns `forge-mcp` in `--transport http` mode with `--auth-token` and `--allow-root`, verifies `401` rejection for missing and invalid tokens, and exercises all 10+ tool workflows over HTTP JSON-RPC.
