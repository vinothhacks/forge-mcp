# `forge-codebase-mcp`

Node.js CLI wrapper and programmatic SDK for the **Forge Codebase Search and Intelligence MCP Server**.

`forge-codebase-mcp` requires **Node.js >= 18.0.0** and has **zero external runtime dependencies**.

---

## Features

- **Seamless CLI Launcher (`forge-mcp`)**: Resolves, downloads, or builds the native Rust `forge-mcp` binary and forwards all CLI flags, environment variables, `stdio` streams, and signals (`SIGINT`, `SIGTERM`).
- **Local `stdio` Transport**: Launch directly from Cursor, Claude Desktop, OpenCode, or any MCP host via `npx forge-codebase-mcp`.
- **Self-Hosted `http` Transport**: Run a multi-workspace HTTP JSON-RPC server with optional Bearer token authentication and canonicalized workspace root allowlists.
- **Programmatic Node.js Client (`ForgeMcpClient`)**: Built-in client supporting both `stdio` and `http` transports with helpers for `initialize()`, `listTools()`, `callTool()`, and `close()`.

---

## Quick Start (`npx` / MCP Client Configuration)

### 1. Local `stdio` Mode

Add Forge to your MCP configuration (e.g. `.cursor/mcp.json` or `claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "forge": {
      "command": "npx",
      "args": [
        "-y",
        "forge-codebase-mcp",
        "--transport",
        "stdio",
        "--allow-root",
        "/path/to/your/workspace"
      ]
    }
  }
}
```

Or run directly from a local checkout:

```bash
node ./npm/bin/forge-mcp.js --transport stdio --allow-root /path/to/your/workspace
```

---

### 2. Self-Hosted HTTP Mode

Start the Forge HTTP server with Bearer authentication and an allowlisted workspace root:

```bash
npx forge-codebase-mcp \
  --transport http \
  --host 0.0.0.0 \
  --port 8080 \
  --auth-token "your-secret-bearer-token" \
  --allow-root /srv/repositories
```

Endpoints exposed in HTTP mode:
- `POST http://<host>:<port>/mcp` — MCP JSON-RPC 2.0 / Streamable HTTP endpoint
- `GET http://<host>:<port>/health` — Health check endpoint

---

## Binary Resolution Order

When `forge-mcp` or `resolveBinaryPath()` runs, it locates `forge-mcp` (or `forge-mcp.exe` on Windows) in the following order:

1. `FORGE_MCP_BIN` environment variable (if set)
2. Bundled binary at `npm/bin/native/forge-mcp[.exe]`
3. Workspace build output at `target/release/forge-mcp[.exe]` or `target/debug/forge-mcp[.exe]`
4. A previously downloaded release binary in `~/.forge-mcp/bin/<version>/<target>/` (override with `FORGE_MCP_CACHE_DIR`)
5. System `PATH`
6. Download the matching binary from [GitHub Releases](https://github.com/vinothhacks/forge-mcp/releases), verify it against the release `SHA256SUMS`, and cache it. Set `FORGE_MCP_SKIP_DOWNLOAD=1` to disable; `FORGE_MCP_RELEASE_BASE_URL` and `FORGE_MCP_RELEASE_TAG` point it at a mirror or another release.
7. Automatic source build via `cargo build --release` if `Cargo.toml` exists in the parent directory (all compiler output is directed to `stderr` so `stdio` JSON-RPC `stdout` is never polluted)

Prebuilt binaries exist for Windows x64, Linux x64/arm64 and macOS x64/arm64. On other platforms, install with `cargo install forge-mcp --locked`.

---

## Programmatic Node.js API

```js
const {
  resolveBinaryPath,
  spawnForgeServer,
  ForgeMcpClient
} = require('forge-codebase-mcp');

// 1. Resolve native binary path
const binPath = resolveBinaryPath();

// 2. Connect via stdio
const stdioClient = new ForgeMcpClient({
  transport: 'stdio',
  allowRoot: process.cwd()
});

await stdioClient.initialize();
const { tools } = await stdioClient.listTools();

const openRes = await stdioClient.callTool('workspace_open', {
  path: process.cwd()
});
console.log('Snapshot ID:', openRes.snapshot_id);

const searchRes = await stdioClient.callTool('code_search', {
  workspace_id: openRes.results.workspace_id,
  query: 'authentication middleware'
});
console.log('Search results:', searchRes.results);

await stdioClient.close();

// 3. Connect via HTTP with Bearer token
const httpClient = new ForgeMcpClient({
  transport: 'http',
  url: 'http://127.0.0.1:8080/mcp',
  token: process.env.FORGE_AUTH_TOKEN
});

await httpClient.initialize();
const statusRes = await httpClient.callTool('workspace_status', {
  workspace_id: openRes.results.workspace_id
});
await httpClient.close();
```

---

## Available MCP Tools

| Tool | Description |
|---|---|
| `workspace_open` | Register and index a workspace directory into a content-hashed `Snapshot`. |
| `workspace_status` | Inspect indexed file/symbol/edge counts and live snapshot freshness (`current` vs `stale`). |
| `code_search` | Hybrid retrieval (BM25 lexical + AST symbol match + vector similarity with RRF). |
| `symbol_find` | Locate symbols by exact, prefix, or substring name, kind, and file path. |
| `symbol_read` | Read complete symbol source body, signature, line ranges, and content hash. |
| `symbol_references` | Find all recorded call and reference sites across the workspace. |
| `graph_query` | Query bounded relationships (`callers`, `callees`, `imports`, `importers`, `children`, `implements`, `path`). |
| `impact_analyze` | Compute transitive change blast radius (affected symbols, files, and test files). |
| `context_build` | Assemble a token-budgeted, deduplicated, source-backed context package. |
| `job_status` / `job_cancel` | Inspect or cancel background indexing jobs. |
