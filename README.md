# Forge: codebase search and code intelligence for coding agents

[![CI](https://github.com/vinothhacks/forge-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/vinothhacks/forge-mcp/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Forge is a self-hosted [Model Context Protocol](https://modelcontextprotocol.io) server that gives coding agents
fast, source-backed answers about a codebase. It indexes your repository with Tree-sitter, keeps a local SQLite
index, and exposes eleven tools for search, symbol navigation, call graphs, change impact and token-budgeted context.

- One self-contained binary (bundled SQLite + Tree-sitter grammars). No database server, no Python, no cloud.
- Languages: Rust, TypeScript/TSX, JavaScript/JSX (`.js .mjs .cjs .jsx`), Python (`.py .pyi`), Go.
- Hybrid search: SQLite FTS5 BM25 + AST symbol matching + local vector similarity, fused with Reciprocal Rank Fusion.
- Every answer carries evidence: snapshot id, file path, exact line/byte range, content hash, and whether the index is still fresh.
- Runs over stdio (local agents) or HTTP (self-hosted, with Bearer auth and workspace allowlists).

## Install

Download the archive for your platform and `SHA256SUMS` from the same [GitHub release](https://github.com/vinothhacks/forge-mcp/releases).
Check the archive before extracting it. Release assets are available for Windows x64, Linux x64/arm64,
and macOS x64/arm64. A Rust toolchain can also build the tagged source.

```bash
# Linux x64 example. For macOS choose the matching Darwin asset and use shasum -a 256.
base=https://github.com/vinothhacks/forge-mcp/releases/download/v0.1.0
asset=forge-mcp-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
curl -fLO "$base/$asset" &&
  curl -fLO "$base/SHA256SUMS" &&
  grep -F "  $asset" SHA256SUMS | sha256sum --check &&
  tar -xzf "$asset" &&
  "./${asset%.tar.gz}/forge-mcp" --version
```

```powershell
$base = 'https://github.com/vinothhacks/forge-mcp/releases/download/v0.1.0'
$asset = 'forge-mcp-v0.1.0-x86_64-pc-windows-msvc.zip'
Invoke-WebRequest "$base/$asset" -OutFile $asset
Invoke-WebRequest "$base/SHA256SUMS" -OutFile SHA256SUMS
$expected = ((Select-String -Path SHA256SUMS -SimpleMatch "  $asset").Line -split '\s+')[0]
if ((Get-FileHash $asset -Algorithm SHA256).Hash -ne $expected) { throw 'Checksum mismatch' }
Expand-Archive $asset -DestinationPath .
& '.\forge-mcp-v0.1.0-x86_64-pc-windows-msvc\forge-mcp.exe' --version
```

## Connect your coding agent

Replace `/abs/path/to/repo` with the repository the agent may read, and keep the index outside the repo with an
absolute `--db-path`. Forge only opens workspaces under the `--allow-root` directories you pass.

### Claude Code

```bash
claude mcp add forge -- /abs/path/to/forge-mcp --transport stdio --allow-root /abs/path/to/repo --db-path ~/.forge-mcp/index.db
```

### Cursor (`.cursor/mcp.json`), Windsurf (`~/.codeium/windsurf/mcp_config.json`), Claude Desktop (`claude_desktop_config.json`)

```json
{
  "mcpServers": {
    "forge": {
      "command": "/abs/path/to/forge-mcp",
      "args": [
        "--transport", "stdio",
        "--allow-root", "/abs/path/to/repo",
        "--db-path", "/abs/path/to/forge-index.db"
      ]
    }
  }
}
```

On Windows, point `"command"` at the downloaded `forge-mcp.exe` and escape backslashes in the JSON.

### VS Code (`.vscode/mcp.json`)

```json
{
  "servers": {
    "forge": {
      "type": "stdio",
      "command": "/abs/path/to/forge-mcp",
      "args": ["--transport", "stdio", "--allow-root", "${workspaceFolder}", "--db-path", "${userHome}/.forge-mcp/index.db"]
    }
  }
}
```

### First call

Ask the agent to call `workspace_open` with the repository path. It returns a `workspace_id`; every other tool takes it.

## Self-hosted HTTP

```bash
# Set FORGE_AUTH_TOKEN in the process environment before starting the server.
forge-mcp --transport http --host 127.0.0.1 --port 8080 \
  --allow-root /srv/repos \
  --db-path /var/lib/forge/index.db
```

| Endpoint | Purpose |
|---|---|
| `POST /mcp` | MCP JSON-RPC 2.0 (single or batch) |
| `GET /sse` + `POST /message` | Legacy SSE transport |
| `GET /health` | Liveness check, no auth |

Connect Claude Code to it:

```bash
claude mcp add --transport http forge http://127.0.0.1:8080/mcp --header "Authorization: Bearer $FORGE_AUTH_TOKEN"
```

## Tools

| Tool | What it does |
|---|---|
| `workspace_open` | Index a directory (respects `.gitignore`) into a content-hashed snapshot. Reuses the snapshot when nothing changed. |
| `workspace_status` | File/symbol/edge counts and live freshness: `current`, or `stale` with the changed files. |
| `code_search` | Hybrid search (BM25 + symbol match + vector similarity, RRF-ranked) with language, path and test filters. |
| `symbol_find` | Find symbols by exact, prefix or substring name, kind, file and language. |
| `symbol_read` | Full source, signature, doc comment and enclosing scope of a symbol. |
| `symbol_references` | Call, import and inheritance sites of a symbol, with surrounding context. |
| `graph_query` | `callers`, `callees`, `imports`, `importers`, `children`, `implements`, or shortest `path`, up to depth 8. |
| `impact_analyze` | Blast radius of changing a symbol or file: direct and transitive dependents, affected files, impacted tests. |
| `context_build` | Token-budgeted, deduplicated context package for a task, with 1-hop graph expansion. |
| `job_status` / `job_cancel` | Inspect or cancel indexing jobs. |

Every tool returns the same envelope: `snapshot_id`, `freshness`, `results`, `evidence`, `limitations`, `truncated`, `next_cursor`.
Relationships come from Tree-sitter syntax, so they are labelled `heuristic` (or `unresolved` for external symbols), not compiler-verified.

## Configuration

| Flag | Environment variable | Default | Meaning |
|---|---|---|---|
| `--transport` | `FORGE_TRANSPORT` | `stdio` | `stdio` or `http` (`sse` is an alias of `http`) |
| `--host` | `FORGE_HOST` | `127.0.0.1` | HTTP bind address |
| `--port` | `FORGE_PORT` | `8080` | HTTP port |
| `--auth-token` | `FORGE_AUTH_TOKEN` | none | Bearer token for HTTP; empty disables auth |
| `--allow-root` | `FORGE_ALLOW_ROOT` | none | Directory workspaces must live under. Repeat the flag for several roots; the env var takes one path |
| `--db-path` | `FORGE_DB_PATH` | `.forge/forge-index.db` | SQLite index file (relative to the working directory) |
| `--embedding-url` | `FORGE_EMBEDDING_URL` | none | Optional OpenAI/Ollama-compatible `/v1/embeddings` endpoint; a built-in local embedder is used otherwise |
| `--embedding-model` | `FORGE_EMBEDDING_MODEL` | none | Model name for the embedding endpoint |
| | `RUST_LOG` | `warn` | Log level (logs go to stderr only) |

npm wrapper only:

| Variable | Meaning |
|---|---|
| `FORGE_MCP_BIN` | Use this binary instead of downloading one |
| `FORGE_MCP_SKIP_DOWNLOAD` | Never download; fail if no binary is found |
| `FORGE_MCP_CACHE_DIR` | Cache directory for downloaded binaries (default `~/.forge-mcp`) |
| `FORGE_MCP_RELEASE_BASE_URL` | Mirror of the GitHub Releases download URL |
| `FORGE_MCP_RELEASE_TAG` | Release tag to download (default `v<package version>`) |

## Security

- Always pass `--allow-root`. Without it, a client can index any directory the process can read.
- Set `--auth-token` whenever `--host` is not loopback. The token is also accepted as a `?token=` query parameter, which can end up in proxy logs; prefer the `Authorization` header.
- There is no built-in TLS. Put a reverse proxy in front for HTTPS.
- Over HTTP, every response is also broadcast to all `/sse` subscribers. Do not share one server and token between parties who must not see each other's results.
- The index database contains your source text. Protect it like the repository.

## Troubleshooting

- **Download blocked or offline:** install with `cargo install forge-mcp --locked` and set `FORGE_MCP_BIN`, or put `forge-mcp` on `PATH`.
- **`no prebuilt binary for <platform>`:** build from source with Cargo.
- **`Checksum mismatch`:** delete `~/.forge-mcp` and retry; if it repeats, the download was altered in transit.
- **Behind a proxy (Node 24):** set `NODE_USE_ENV_PROXY=1` together with `HTTPS_PROXY`.
- **`outside allowed roots`:** the path passed to `workspace_open` is not under any `--allow-root`.

## Development

```bash
cargo build --release
cargo test -- --test-threads=1
cd npm && npm test
```

Verification reports for every tool and transport are in [docs/verification](docs/verification/README.md).
See [CONTRIBUTING.md](CONTRIBUTING.md) for the release process and [CHANGELOG.md](CHANGELOG.md) for history.

## License

MIT. See [LICENSE](LICENSE).
