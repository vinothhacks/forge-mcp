# F14: npm wrapper

**Verdict: FAIL-FIXED.** Date: 2026-11-15. Baseline binary `ae7389b8...9064e`. Suite before fix: 3/3 (original tests); after fix: 10/10.

| # | Criterion | Before fix | After fix |
|---|---|---|---|
| 1 | `forge-mcp --help`, `--version`, `--transport bogus` through the wrapper | PASS | PASS |
| 2 | Resolution order: explicit path, bundled, release, debug, PATH; clear error listing every location; never builds when `autoBuild:false` | PASS | PASS |
| 3 | `findBinaryInPath` / `getBinaryName` per platform | PASS | PASS |
| 4 | `normalizeToolResult` exposes envelope fields for every response shape | **FAIL** (`structuredContent`-only results returned untouched) | PASS |
| 5 | Stdio client: initialize, 11 tools, tool call, errors carry `code -32000`, `close()` lets the server exit cleanly | **FAIL** (`close()` killed the server immediately; `exitCode` null) | PASS (exit code 0) |
| 6 | HTTP option parsing (`url`, `host`, `port`, token aliases) | PASS | PASS |
| 7 | `parseHttpRpcBody` with JSON and SSE bodies | PASS | PASS (now exported) |
| 8 | Repository e2e suite | PASS | PASS |
| 9 | Package contents | PASS | PASS |
| 10 | Ending stdin exits the wrapper with code 0 and leaves no orphan process | PASS | PASS |

**Fix (`npm/index.js`):** `normalizeToolResult` falls back to `structuredContent` whenever present; `close()` ends stdin first, then sends SIGTERM after `closeGraceMs` (default 1000 ms), then SIGKILL; `parseHttpRpcBody` exported. The first e2e test now honours `FORGE_MCP_BIN` so the suite runs outside the Rust workspace.

**Regression tests:** 7 offline tests in `npm/test/e2e.test.js` (resolution order, platform helpers, all normalize shapes, SSE parsing, graceful and stubborn `close()`, real-binary `close()`). Four of them fail on the old code.

Release-phase additions made afterwards (not part of this phase's verdict): package renamed to `forge-codebase-mcp`; `lib/platform.js` and `lib/download.js` download the release binary for the platform, verify it against `SHA256SUMS`, and cache it; `test/download.test.js` covers that path offline.
