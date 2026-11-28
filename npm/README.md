# `forge-codebase-mcp`

Node.js CLI wrapper and programmatic SDK for the **Forge Codebase Search and Intelligence MCP Server**.

`forge-codebase-mcp` requires **Node.js >= 18.0.0** and has **zero external runtime dependencies**.

---

Install Forge from a verified GitHub release archive. The native binary runs on its own, and the Node.js
wrapper can launch it from a local source checkout or provide a programmatic client.

## Install

Follow the Windows PowerShell or Linux/macOS sh commands in the
[Forge installation guide](https://github.com/vinothhacks/forge-mcp#install) to verify the archive against the
same release's `SHA256SUMS` before extraction. Windows x64 passed clean-install first use. The Linux x64,
Linux arm64, macOS x64 and macOS arm64 archives have checksum verification only.

The `forge-codebase-mcp` wrapper and `forge-mcp` crate are unpublished. The npm package named `forge-mcp`
belongs to the unrelated Hermes Forge project. The wrapper has no npm dependencies to install.

## Use the wrapper from a local checkout

### Local stdio

For a client such as Cursor or Claude Desktop, give Node.js the absolute path to the wrapper and set
`FORGE_MCP_BIN` to the verified native binary. Run the [first-use fixture check](https://github.com/vinothhacks/forge-mcp#first-use)
to confirm the connection and search.

```json
{
  "mcpServers": {
    "forge": {
      "command": "node",
      "args": [
        "/abs/path/to/forge-mcp/npm/bin/forge-mcp.js",
        "--transport",
        "stdio",
        "--allow-root",
        "/abs/path/to/fixture",
        "--db-path",
        "/abs/path/to/forge-index.db"
      ],
      "env": {
        "FORGE_MCP_BIN": "/abs/path/to/forge-mcp-binary",
        "FORGE_MCP_SKIP_DOWNLOAD": "1"
      }
    }
  }
}
```

### Self-hosted HTTP

Set `FORGE_AUTH_TOKEN` in the process environment. With `FORGE_MCP_BIN` pointing to the verified binary,
run this from the checkout root:

```bash
node ./npm/bin/forge-mcp.js \
  --transport http \
  --host 127.0.0.1 \
  --port 8080 \
  --allow-root /srv/repositories \
  --db-path /var/lib/forge/index.db
```

`POST /mcp` with an initialize request returns 401 without a Bearer token and 200 with the configured
token. `GET /health` is unauthenticated. Always configure a token before binding off-loopback. The
unreleased native binary enforces this, including for blank tokens. The published v0.1.0 binary relies
on the operator to set the token.

---

## Binary Resolution Order

When `forge-mcp` or `resolveBinaryPath()` runs, it locates `forge-mcp` (or `forge-mcp.exe` on Windows) in the following order:

1. `FORGE_MCP_BIN` environment variable (if set)
2. Bundled binary at `npm/bin/native/forge-mcp[.exe]`
3. Workspace build output at `target/release/forge-mcp[.exe]` or `target/debug/forge-mcp[.exe]`
4. A previously downloaded release binary in `~/.forge-mcp/bin/<version>/<target>/` (override with `FORGE_MCP_CACHE_DIR`)
5. System `PATH`
6. Download the matching binary from [GitHub Releases](https://github.com/vinothhacks/forge-mcp/releases), verify it against the release `SHA256SUMS`, and cache it. Set `FORGE_MCP_SKIP_DOWNLOAD=1` to disable; `FORGE_MCP_RELEASE_BASE_URL` and `FORGE_MCP_RELEASE_TAG` point it at a mirror or another release.
7. Automatic source build via `cargo build --release --locked` if `Cargo.toml` exists in the parent directory (compiler output goes to `stderr` to keep `stdout` available for MCP)

For another platform, try `cargo build --release --locked` from the tagged source checkout.

---

## Programmatic Node.js API

Run this example in an async function from the checkout root. Set `FORGE_MCP_BIN` to the verified binary.

```js
const {
  resolveBinaryPath,
  spawnForgeServer,
  ForgeMcpClient
} = require('./npm');

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

const workspaceId = openRes.results[0].workspace_id;
const searchRes = await stdioClient.callTool('code_search', {
  workspace_id: workspaceId,
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
const httpOpenRes = await httpClient.callTool('workspace_open', { path: process.cwd() });
const statusRes = await httpClient.callTool('workspace_status', {
  workspace_id: httpOpenRes.results[0].workspace_id
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
