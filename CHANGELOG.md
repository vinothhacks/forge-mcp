# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.1.0] - 2026-11-16

### Added

- Rust MCP server `forge-mcp` with stdio transport (newline-delimited and `Content-Length` framed JSON-RPC) and self-hosted HTTP transport (`POST /mcp`, `GET /sse` + `POST /message`, `GET /health`) with optional Bearer authentication and workspace root allowlists.
- Eleven MCP tools: `workspace_open`, `workspace_status`, `code_search`, `symbol_find`, `symbol_read`, `symbol_references`, `graph_query`, `impact_analyze`, `context_build`, `job_status`, `job_cancel`.
- Tree-sitter symbol, reference and relationship extraction for Rust, TypeScript/TSX, JavaScript/JSX, Python and Go.
- Hybrid retrieval combining SQLite FTS5 BM25, AST symbol matching and local vector similarity with Reciprocal Rank Fusion; optional OpenAI/Ollama-compatible embedding endpoint.
- Content-hashed snapshots with live freshness detection and token-budgeted context assembly.
- npm wrapper `forge-codebase-mcp` providing the `forge-mcp` CLI launcher, automatic download of release binaries, and a programmatic `ForgeMcpClient`.
- GitHub Actions CI (fmt, clippy, tests on Linux/Windows/macOS) and tag-driven release workflow producing binaries for five targets, crates.io and npm publishes.
