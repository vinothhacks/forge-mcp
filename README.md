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

Use a fresh download directory. Download the archive for your platform and `SHA256SUMS` from
[GitHub Release v0.1.0](https://github.com/vinothhacks/forge-mcp/releases/tag/v0.1.0) and verify the archive
before extracting it. Clean-install first use was verified on Windows x64. Linux x64, Linux arm64,
macOS x64 and macOS arm64 have checksum verification only.

### Windows x64 (PowerShell)

```powershell
$ErrorActionPreference = 'Stop'
$base = 'https://github.com/vinothhacks/forge-mcp/releases/download/v0.1.0'
$asset = 'forge-mcp-v0.1.0-x86_64-pc-windows-msvc.zip'
Invoke-WebRequest -UseBasicParsing "$base/$asset" -OutFile $asset
Invoke-WebRequest -UseBasicParsing "$base/SHA256SUMS" -OutFile SHA256SUMS
$pattern = '^[0-9a-fA-F]{64}  ' + [regex]::Escape($asset) + '$'
$entry = @(Get-Content -LiteralPath SHA256SUMS | Where-Object { $_ -cmatch $pattern })
if ($entry.Count -ne 1) { throw 'Expected one checksum for this archive' }
if ((Get-FileHash -LiteralPath $asset -Algorithm SHA256).Hash -ne $entry[0].Substring(0, 64)) {
    throw 'Checksum mismatch'
}
Expand-Archive -LiteralPath $asset -DestinationPath .
& '.\forge-mcp-v0.1.0-x86_64-pc-windows-msvc\forge-mcp.exe' --version
```

### Linux and macOS (sh)

```sh
(
  set -eu
  base=https://github.com/vinothhacks/forge-mcp/releases/download/v0.1.0
  os=$(uname -s)
  arch=$(uname -m)
  case "$os/$arch" in
    Linux/x86_64) target=x86_64-unknown-linux-gnu ;;
    Linux/aarch64) target=aarch64-unknown-linux-gnu ;;
    Darwin/x86_64) target=x86_64-apple-darwin ;;
    Darwin/arm64) target=aarch64-apple-darwin ;;
    *) echo "No published archive for $os/$arch" >&2; exit 1 ;;
  esac
  asset=forge-mcp-v0.1.0-$target.tar.gz
  curl -fLO "$base/$asset"
  curl -fLO "$base/SHA256SUMS"
  expected=$(awk -v asset="$asset" '
    NF == 2 && $2 == asset && length($1) == 64 && $1 ~ /^[[:xdigit:]]+$/ {
      print tolower($1)
      count++
    }
    END { if (count != 1) exit 1 }
  ' SHA256SUMS) || { echo 'Expected one checksum for this archive' >&2; exit 1; }
  if [ "$os" = Linux ]; then hash_line=$(sha256sum "$asset")
  else hash_line=$(shasum -a 256 "$asset"); fi
  actual=${hash_line%% *}
  [ "$actual" = "$expected" ] || { echo 'Checksum mismatch' >&2; exit 1; }
  tar -xzf "$asset"
  "./${asset%.tar.gz}/forge-mcp" --version
)
```

The `forge-mcp` crate and `forge-codebase-mcp` npm wrapper are unpublished. The npm package named
`forge-mcp` belongs to the unrelated Hermes Forge project. To build from source, check out the `v0.1.0`
tag and run `cargo build --release --locked` from the repository root. The binary is in `target/release/`.

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

### First use

1. Create a disposable directory with a `fixture.py` file containing `def academy_fixture_hello(): return "hello"`.
   Set `--allow-root` to that directory and `--db-path` to a new database path outside it.
2. Connect over stdio. Send `initialize` with `protocolVersion` set to `2024-11-05`, then send
   `notifications/initialized` and call `tools/list`. Expect server name `forge-mcp` and 11 tools.
3. Call `workspace_open` with the absolute fixture `path`. Pass the returned `results[0].workspace_id`
   to `code_search` with `query` set to `academy_fixture_hello`. Expect a result for `fixture.py`.
4. For HTTP, start on `127.0.0.1` with a throwaway `FORGE_AUTH_TOKEN` set in the server environment.
   `POST /mcp` with an `initialize` request returns 401 without a token and 200 with
   `Authorization: Bearer <token>`. The unreleased build also refuses off-loopback binds with a missing
   or blank token. The published v0.1.0 binary requires you to set the token yourself before binding off-loopback.

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
| `--auth-token` | `FORGE_AUTH_TOKEN` | none | Bearer token for HTTP. The unreleased build allows a blank token only on loopback. |
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

- The published v0.1.0 archive predates the bundled SQLite and `url` fixes in [Unreleased](CHANGELOG.md#unreleased).
- Always pass `--allow-root`. Without it, a client can index any directory the process can read.
- Set `FORGE_AUTH_TOKEN` before binding HTTP off-loopback. The unreleased build enforces this requirement.
  Use the `Authorization` header because query parameters can appear in proxy logs.
- There is no built-in TLS. Put a reverse proxy in front for HTTPS.
- Over HTTP, every response is also broadcast to all `/sse` subscribers. Do not share one server and token between parties who must not see each other's results.
- The index database contains your source text. Protect it like the repository.

## Troubleshooting

- **Download blocked or offline:** use an already verified archive or build from a source checkout and set `FORGE_MCP_BIN` to the resulting binary.
- **`no prebuilt binary for <platform>`:** build from source with Cargo.
- **`Checksum mismatch`:** discard the download and fetch the archive and `SHA256SUMS` again from the same release. Stop if the mismatch persists.
- **Behind a proxy (Node 24):** set `NODE_USE_ENV_PROXY=1` together with `HTTPS_PROXY`.
- **`outside allowed roots`:** the path passed to `workspace_open` is not under any `--allow-root`.

## Development

```bash
cargo build --release --locked
cargo test --locked --all-targets -- --test-threads=1
cd npm && FORGE_MCP_SKIP_DOWNLOAD=1 npm test
```

Verification reports for every tool and transport are in [docs/verification](docs/verification/README.md).
See [CONTRIBUTING.md](CONTRIBUTING.md) for the release process and [CHANGELOG.md](CHANGELOG.md) for history.

## License

MIT. See [LICENSE](LICENSE).
