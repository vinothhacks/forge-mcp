# Forge — Self-Hosted Rust Code-Intelligence MCP

**A build plan for a Rust MCP server that unifies the capabilities of Serena, CodeGraphContext, Augment Context Engine, and Code Scalpel — investigated with [REA](https://github.com/morluto/rea) — and served to coding agents in a fully self-hosted deployment.**

Working name: **Forge**. Status: planning document. Date: 2026-10-26.

---

## 1. Define What You Are Building

### Product goal

A self-hosted code-intelligence MCP server that allows coding agents to:

1. Find relevant implementation from natural-language requests.
2. Navigate symbols, definitions, references, and implementations.
3. Trace calls, imports, inheritance, and dependencies.
4. Plan and apply controlled cross-file changes.
5. Inspect diagnostics, run selected checks, and investigate security findings.
6. Retrieve compact, source-backed context across authorized repositories.
7. Preserve project knowledge without confusing notes with verified code facts.

The coding agent remains responsible for reasoning and orchestration. Your server supplies evidence, operations, and validation — not an autonomous replacement for the agent.

### What to learn from each reference

| Reference | Capabilities to investigate | Important boundary |
|---|---|---|
| [Serena](https://github.com/oraios/serena) | Symbol retrieval, LSP integration, editing, refactoring, configuration, project memory | Some advanced refactoring and interactive debugging are provided through its JetBrains backend, not its default LSP backend. |
| [CodeGraphContext](https://github.com/CodeGraphContext/CodeGraphContext) | Parsing, graph construction, incremental updates, relationship queries, optional SCIP indexing | Its documentation distinguishes Tree-sitter heuristics from more accurate compiler-backed SCIP indexing. Preserve that distinction. |
| [Augment Context Engine](https://docs.augmentcode.com/context-services/mcp/overview) | Semantic retrieval, relationship-aware context, cross-repository search, context curation | Its documented interface does not expose the complete hosted engine implementation. Treat it as a behavioral reference, not available source to port. |
| [Code Scalpel](https://github.com/3D-Tech-Solutions/code-scalpel) | Symbol extraction, controlled modifications, security findings, dependency analysis, bounded symbolic analysis | Its own documentation describes uneven language coverage, including Python-first graph features. Do not equate a feature list with universal language parity. |

Your goal should be feature-category coverage with measured quality — not a claim that the first release matches every feature of all four products.

---

## 2. Recommended Architecture

```text
                  Coding agents
          OpenCode / OMP / other MCP clients
                         │
                 stdio or HTTPS
                         │
┌────────────────────────▼──────────────────────────┐
│ Rust MCP gateway                                   │
│ Typed schemas • auth • authorization • budgets     │
│ cancellation • job tracking • structured errors    │
└────────────────────────┬──────────────────────────┘
                         │
┌────────────────────────▼──────────────────────────┐
│ Workspace and snapshot coordinator                 │
│ Repo identity • worktrees • file hashes • versions │
│ dirty-file overlays • locks • index generations    │
└──────┬────────────┬─────────────┬─────────────┬────┘
       │            │             │             │
┌──────▼─────┐ ┌────▼──────┐ ┌────▼───────┐ ┌───▼──────────┐
│ Syntax     │ │ Semantic  │ │ Retrieval  │ │ Analysis     │
│ Tree-sitter│ │ LSP / SCIP│ │ Lexical    │ │ Data flow    │
│ outlines   │ │ compiler  │ │ vectors    │ │ taint        │
│ extraction │ │ adapters  │ │ reranking  │ │ diagnostics  │
└──────┬─────┘ └────┬──────┘ └────┬───────┘ └───┬──────────┘
       └────────────┴─────────────┴─────────────┘
                         │
┌────────────────────────▼──────────────────────────┐
│ Shared code model and evidence store               │
│ Files • symbols • references • graph edges         │
│ source ranges • provenance • certainty • freshness │
└────────────────────────┬──────────────────────────┘
                         │
┌────────────────────────▼──────────────────────────┐
│ Change and verification engine                     │
│ Preview → authorize → stage → validate → apply     │
│ conflict checks • recovery journal • audit         │
└────────────────────────┬──────────────────────────┘
                         │
                 Isolated check workers
              builds / tests / approved tools
```

REA belongs in your development investigation environment, not in the production request path.

### Why a shared engine matters

Proposed rule: retrieval, graph analysis, and edits must refer to the same workspace snapshot.

Otherwise, an agent might retrieve yesterday's symbol, traverse today's graph, and edit a file changed five seconds ago. A fast server with inconsistent snapshots would be less useful than a slower, reliable one.

---

## 3. Rust Implementation Strategy

Use Rust for your server, orchestration, indexing, storage interfaces, query logic, policies, and editing engine.

For MCP, use the official Rust SDK, [`rmcp`](https://github.com/modelcontextprotocol/rust-sdk), which provides stdio and Streamable HTTP transports. Pin the SDK version and test against the actual clients you intend to support.

### Proposed technology choices

These are implementation recommendations, not requirements imposed by the reference projects.

| Component | Initial choice | Design reason |
|---|---|---|
| MCP | `rmcp` | Avoid building protocol plumbing yourself |
| Async runtime | Tokio | One runtime for server and worker orchestration |
| HTTP service | Axum-compatible gateway | Keep remote deployment separate from core logic |
| Parsing | Tree-sitter through Rust bindings | Structural extraction across initial languages |
| Semantic resolution | LSP subprocess manager; optional SCIP importer | Reuse language intelligence instead of writing compilers |
| Metadata and graph persistence | SQLite initially | Simple deployment and transactional metadata |
| Graph traversal | Indexed adjacency tables plus bounded Rust traversal | Avoid requiring a graph database for the MVP |
| Lexical retrieval | Tantivy candidate | Keep exact identifiers and text search first-class |
| Semantic retrieval | Local embedding endpoint and replaceable vector backend | No mandatory hosted inference |
| File changes | Watcher plus periodic reconciliation | Recover from missed file events |
| Observability | Structured logs, metrics, traces | Measure indexing, query, and edit failures |

Validate library versions, licenses, platform support, and operational characteristics during the foundation phase.

### Clarify "written in Rust"

Recommended interpretation:

- All first-party production code is Rust.
- Language servers, SCIP indexers, embedding runtimes, and solvers may run as local subprocesses or services.
- None of the four original MCP servers is required in production.
- Every external component has a pinned version and documented license.

If you require every dependency and language analyzer to be implemented in Rust too, that becomes a substantially larger compiler and inference-runtime project.

---

## 4. Use REA Through a Source-First Investigation Process

[REA](https://github.com/morluto/rea) supports local JavaScript/Electron inspection, native analysis through configured providers, and runtime observations. Its documented JavaScript analysis does not require a native decompiler; native analysis does. It also warns that runtime capture executes or interacts with the target using your permissions.

### Investigation workflow

```text
Pin reference revision
          │
          ▼
Read source, docs, tests, and license
          │
          ▼
Trace one capability end to end
          │
          ▼
Use REA where it adds evidence
          │
          ▼
Write an implementation-neutral specification
          │
          ▼
Create input/output and failure fixtures
          │
          ▼
Implement in Rust
          │
          ▼
Run differential and independent correctness tests
```

Do not translate Python or TypeScript line by line.

### Where REA helps

| Target | Primary method | REA's useful role |
|---|---|---|
| Serena | Trace source, LSP interactions, tests | Investigate supported runtime behavior where necessary |
| CodeGraphContext | Trace parsers, graph schema, update logic, tests | Supplement source findings with authorized runtime observations |
| Augment | Public documentation and permitted interface experiments using synthetic code | Inspect authorized local JavaScript artifacts and visible client behavior; do not infer hidden server algorithms as facts |
| Code Scalpel | Trace tool dispatch, language adapters, analysis implementation, tests | Supplement observations and inspect supported artifacts |

For Augment, implement your own retrieval architecture based on documented capabilities and measured behavior. Its [published material](https://www.augmentcode.com/blog/context-engine-mcp-now-live) describes cloud-side indexing and search infrastructure; a locally running MCP adapter does not mean the entire engine is self-hosted.

### Required investigation deliverables

For every feature, create:

```text
Capability specification
  - target revision
  - source references
  - observed inputs and outputs
  - errors and cancellation behavior
  - state and cache dependencies
  - language limitations
  - evidence and uncertainty
  - license/provenance notes
  - acceptance fixtures
```

Example agent instruction:

> Investigate symbol rename in the pinned reference revision. Trace the MCP handler, backend request, edit construction, and failure paths. Use source inspection first and REA only where it adds relevant evidence. Produce an implementation-neutral specification and fixtures covering name collisions, stale files, ambiguous symbols, and unsupported backend capabilities. Separate observations from hypotheses.

Run reference systems and REA in isolated research environments without production credentials.

---

## 5. Resolve Licensing Before Implementation

This is a real project gate, not final-release paperwork.

- [Serena](https://github.com/oraios/serena) identifies SolidLSP as MIT and the rest of the Serena application as GPL-3.0-or-later; its repository states combined distributions are subject to GPL. Do not assume rewriting copied implementation into Rust removes its license obligations.
- [CodeGraphContext](https://github.com/CodeGraphContext/CodeGraphContext) identifies its project as MIT. Verify the pinned revision and relevant dependencies before reuse.
- [Code Scalpel](https://github.com/3D-Tech-Solutions/code-scalpel) describes MIT Community licensing alongside Pro/Enterprise tiers. Inspect actual files and terms rather than assuming every advertised capability is freely reusable.
- [REA](https://github.com/morluto/rea) identifies its own code as MIT while preserving third-party licenses and external-tool terms.

Proposed policy:

1. Maintain a component-level license inventory.
2. Distinguish copied/adapted code from independently implemented behavior.
3. Preserve required notices.
4. Use only authorized access and permitted experiments.
5. Obtain legal review before choosing distribution terms or reusing restricted components.

Source-first research is not automatically a legally "clean-room" process. If that separation is required, establish the process before investigators and implementers share materials.

---

## 6. Build the Shared Code Model First

### Core entities

```text
Repository
Workspace / worktree
Snapshot
FileVersion
Symbol
Reference
Relationship
CodeChunk
Diagnostic
Finding
ChangeSet
KnowledgeEntry
```

### Essential evidence fields

Every returned symbol, graph edge, or finding should include:

```text
repository_id
workspace_id
snapshot_id
file_path
source_range
content_hash
producer
resolution_kind
observed_at
```

For graph relationships, add:

- Compiler-resolved, heuristic, or unresolved classification.
- Source evidence.
- Relevant indexing configuration.
- Limitations.

Do not turn Tree-sitter name matching into an "exact call graph." CodeGraphContext itself documents the accuracy distinction between heuristic parsing and optional compiler-backed indexing.

### Multi-agent correctness

Design these rules before enabling writes:

- Workspaces are explicit in every request.
- Different worktrees do not share mutable index state.
- Writes check expected file hashes.
- Conflicting changes fail rather than overwrite.
- Background jobs publish complete index generations.
- Queries report whether their snapshot is current.
- Unsaved editor buffers are unsupported until an explicit overlay integration exists.

---

## 7. Expose a Compact MCP Interface

Avoid exposing hundreds of overlapping tools. Start with a small, typed surface.

### Proposed initial tools

| Group | Tools |
|---|---|
| Workspace | `workspace_open`, `workspace_status` |
| Retrieval | `code_search`, `context_build` |
| Symbols | `symbol_find`, `symbol_read`, `symbol_references` |
| Relationships | `graph_query`, `impact_analyze` |
| Changes | `change_preview`, `change_apply` |
| Validation | `checks_run` |
| Long-running work | `job_status`, `job_cancel` |

Add later:

- `security_scan`
- `dataflow_trace`
- `knowledge_read` / `knowledge_write`
- `debug_session`, if debugging becomes a validated requirement

### Tool contract principles

Proposed request:

```json
{
  "workspace_id": "backend-worktree",
  "query": "where is password reset token expiry checked?",
  "snapshot_policy": "require_current",
  "token_budget": 5000,
  "include_tests": true
}
```

Proposed response envelope:

```json
{
  "snapshot_id": "snapshot-1042",
  "freshness": "current",
  "results": [],
  "evidence": [],
  "limitations": [],
  "truncated": false,
  "next_cursor": null
}
```

Use bounded graph operations such as callers, callees, paths, imports, and inheritance. Do not expose arbitrary database queries as the default agent interface.

---

## 8. Implement the Four Major Capability Layers

### A. Symbol navigation and editing

Build:

- File outlines and symbol extraction.
- Definitions and references.
- Implementations where the backend supports them.
- Diagnostics.
- Symbol body replacement and insertion.
- LSP-backed rename.

Keep a language capability matrix. Serena's own documented functionality varies by language server and backend; your interface should make similar limitations explicit.

### B. Repository graph

Build:

- Imports and module relationships.
- Symbol containment.
- Calls and references.
- Inheritance and implementation relationships.
- Bounded paths and neighborhoods.
- Impact analysis.
- Incremental invalidation.

Report unresolved dynamic dispatch rather than silently dropping it. Label "possible dead code" as a candidate, not a certainty.

### C. Self-hosted context engine

Proposed pipeline:

```text
Natural-language request
          │
          ├── lexical search
          ├── symbol search
          └── local vector search
                     │
                     ▼
             candidate merging
                     │
                     ▼
       graph expansion + authorization
                     │
                     ▼
         optional local reranking
                     │
                     ▼
     deduplication + budgeted extraction
                     │
                     ▼
        source-backed context package
```

Start with raw, relevant source spans. Add generated summaries later, clearly labeled and linked to their source snapshots.

Expand eventually to:

- Git history.
- Documentation.
- Approved tickets and architecture notes.
- Cross-repository relationships.

Enforce repository authorization before candidate expansion and ranking — not just before returning final snippets.

### D. Analysis and security

Implement in increasing difficulty:

1. Syntax and complexity analysis.
2. Diagnostics and rule-based findings.
3. Source/sink identification.
4. Intra-procedural data-flow analysis.
5. Bounded inter-procedural taint analysis.
6. Restricted symbolic execution for a documented subset.
7. Counterexample-derived test inputs.

Do not promise general behavioral equivalence or "all paths safe." Code Scalpel advertises symbolic execution and refactoring verification, but your implementation must document supported semantics, bounds, and unknown outcomes rather than inherit marketing claims.

---

## 9. Make Editing a Controlled Transaction

Proposed workflow:

```text
Request change
      │
      ▼
Resolve symbols against a snapshot
      │
      ▼
Create immutable change set
      │
      ▼
Return diff, affected files, risks, check plan
      │
      ▼
Authorize exact change-set hash
      │
      ▼
Stage in isolated worktree
      │
      ▼
Parse → type-check → selected tests → policy checks
      │
      ▼
Recheck destination hashes and acquire locks
      │
      ▼
Apply with recovery journal
      │
      ▼
Invalidate indexes and record audit event
```

Important implementation requirements:

- "Preview" never changes the user's workspace.
- Approval binds to an immutable change set.
- Validation commands run only from approved configurations.
- Concurrent edits produce conflicts.
- Multi-file application has a crash-recovery mechanism.
- Rollback must not overwrite unrelated developer edits.

Passing checks supports confidence; it does not prove arbitrary behavior preservation.

---

## 10. Self-Hosted Deployment and Security

### Local developer mode

```text
Agent
  └── Forge stdio process
       ├── workspace files
       ├── local metadata/search storage
       ├── local language servers
       └── optional local embedding runtime
```

### Team mode

```text
Agents
   │
HTTPS ingress + identity
   │
Rust MCP gateway
   ├── workspace authorization
   ├── isolated workspace workers
   ├── private index storage
   ├── local inference service
   └── isolated validation workers
```

Proposed defaults:

- No outbound network access unless explicitly configured.
- No source content in routine logs.
- Read-only repository access until editing is enabled.
- Separate read, edit, index-management, and execution permissions.
- Canonicalized, allowlisted workspace paths.
- Protection against symlink escapes.
- Resource limits for parsing, queries, language servers, and tests.
- Audit records for writes and privileged operations.
- Backup and deletion procedures covering source-derived indexes.

Self-hosting this MCP does not make a cloud coding agent offline: code returned to the agent can still reach its model provider. [REA](https://github.com/morluto/rea) documents the same distinction for its locally produced analysis results.

---

## 11. Delivery Roadmap

The following is a planning estimate for a small experienced team, not a delivery guarantee. Choose two initial target languages in discovery; TypeScript and Python are a reasonable proposed starting pair if they match your pilot repositories.

| Phase | Estimated duration | Deliverables | Exit gate |
|---|---:|---|---|
| 0. Discovery and evidence | 2 weeks | Pinned references, license inventory, REA investigations, capability matrix, benchmark repositories | Every MVP capability has a specification and test fixture |
| 1. Rust foundation | 2–3 weeks | MCP gateway, workspace identity, storage, snapshots, jobs, policy skeleton | Agents can connect and index a repository reliably |
| 2. Read-only intelligence | 3–4 weeks | Parsing, symbols, references, basic graph, lexical search | Ground-truth navigation and graph tests pass |
| 3. Context retrieval | 3–4 weeks | Local embeddings, hybrid retrieval, graph expansion, context budgets | Meets predefined retrieval quality and latency targets |
| 4. Controlled editing | 3–4 weeks | Preview/apply, rename, staging, checks, conflict handling, recovery | No silent overwrite in concurrency and crash tests |
| 5. Analysis | 4–6 weeks | Initial security rules, bounded taint, policy checks, findings evidence | Supported-language benchmark quality is documented |
| 6. Team hardening | 3–4 weeks | Authentication, repository isolation, backup/restore, load tests, deployment docs | Security and operational acceptance tests pass |

Some work can overlap after the data model stabilizes.

A useful read-only pilot should precede the complete platform. Broad language parity, advanced refactoring, debugging, and symbolic analysis belong on a longer-term roadmap.

### Suggested ownership

- Rust/platform engineer: MCP, lifecycle, storage, deployment.
- Language-intelligence engineer: parsing, LSP, symbols, graph.
- Retrieval engineer: indexing, search, ranking, evaluation.
- Verification/security engineer: change engine, isolation, analysis.
- Shared QA and product owner: fixtures and real agent workflows.

With fewer people, reduce scope rather than weakening snapshot consistency or edit safety.

---

## 12. Prove "Powerful" with Benchmarks

Do not define success by feature count.

Build a fixed evaluation suite using synthetic fixtures, authorized real repositories, and selected historical tasks.

| Area | What to measure |
|---|---|
| Retrieval | Relevant evidence found, irrelevant context, source traceability, token usage |
| Symbol intelligence | Definition/reference precision and recall |
| Graph | Edge correctness, unresolved-call reporting, path correctness |
| Freshness | Correctness after edits, renames, deletes, and branch switches |
| Editing | Correct diffs, stale-change rejection, conflict handling, recovery |
| Security | Precision and recall on labeled cases; unsupported cases reported |
| Agent outcomes | Task completion, tests passed, elapsed time, model tokens |
| Isolation | Unauthorized paths, repos, caches, and snippets never exposed |
| Performance | Cold indexing, incremental indexing, p50/p95 query latency, memory |

Compare:

1. Agent without the new MCP.
2. Agent with Serena.
3. Agent with the relevant specialized reference.
4. Agent with Forge.

Keep the agent model, task, starting commit, and test criteria consistent. Run repeated trials for agent-level outcomes.

Only claim superiority for the tasks and conditions your results actually support.

---

## 13. First Two-Week Execution Plan

### Week 1: establish evidence and contracts

1. Create the Rust workspace and architecture decision log.
2. Pin the reference revisions and capture licenses.
3. Prepare isolated reference environments.
4. Register REA in the development agent environment.
5. Choose two target languages and benchmark repositories.
6. Trace four vertical capabilities:
   - Symbol lookup.
   - Caller tracing.
   - Natural-language retrieval.
   - Symbol update preview.
7. Write fixtures for success, ambiguity, stale state, and failure.

### Week 2: build one end-to-end Rust slice

Implement:

```text
workspace_open
      ↓
index one repository
      ↓
symbol_find
      ↓
symbol_read
      ↓
graph_query: direct callers
      ↓
context_build with source ranges and snapshot IDs
```

Deliver:

- A functioning read-only MCP connected to your coding agent.
- A shared evidence model.
- Reproducible test fixtures.
- A small benchmark report.
- A license/provenance inventory.
- A documented backlog based on observed gaps.

That is the right first milestone: one trustworthy Rust path from repository to agent-visible evidence. Expand it into safe editing, stronger retrieval, and security analysis only after that foundation is measurable and reliable.

---

## Sources

- REA — https://github.com/morluto/rea
- Serena — https://github.com/oraios/serena
- CodeGraphContext — https://github.com/CodeGraphContext/CodeGraphContext and https://codegraphcontext.github.io/CodeGraphContext/
- Augment Context Engine MCP — https://docs.augmentcode.com/context-services/mcp/overview and https://www.augmentcode.com/product/context-engine-mcp
- Code Scalpel — https://github.com/3D-Tech-Solutions/code-scalpel
- Official MCP Rust SDK (rmcp) — https://github.com/modelcontextprotocol/rust-sdk
