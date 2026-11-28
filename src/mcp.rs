use crate::embeddings::EmbeddingEngine;
use crate::graph::{graph_query, impact_analyze};
use crate::model::*;
use crate::search::{code_search, context_build};
use crate::store::ForgeStore;
use anyhow::Result;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;

/// MCP protocol revisions this server implements, newest first. `initialize`
/// echoes the client's requested revision when it is listed here and otherwise
/// answers with the newest one, per the MCP version-negotiation rules.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// Usage hint returned in the `initialize` result (`instructions`).
pub const SERVER_INSTRUCTIONS: &str = "Forge indexes local codebases for search and code intelligence. \
Call workspace_open with a directory path first; it returns a workspace_id. Pass that workspace_id to \
code_search, symbol_find, symbol_read, symbol_references, graph_query, impact_analyze and context_build. \
Use workspace_status to check index freshness and job_status/job_cancel for indexing jobs.";

/// Largest Content-Length frame body accepted on stdio; larger frames are drained and rejected.
pub const MAX_STDIO_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// Pick the protocol revision to answer `initialize` with.
pub fn negotiate_protocol_version(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|v| {
            SUPPORTED_PROTOCOL_VERSIONS
                .iter()
                .copied()
                .find(|s| *s == v)
        })
        .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0])
}

#[derive(Clone)]
pub struct ForgeMcpServer {
    pub store: ForgeStore,
    pub embedder: EmbeddingEngine,
    pub allowed_roots: Vec<PathBuf>,
}

impl ForgeMcpServer {
    pub fn new(store: ForgeStore, embedder: EmbeddingEngine, allowed_roots: Vec<PathBuf>) -> Self {
        Self {
            store,
            embedder,
            allowed_roots,
        }
    }

    pub fn handle_jsonrpc_str(&self, raw: &str) -> Result<Option<String>> {
        let val: Value = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(e) => {
                let err_resp = json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": {
                        "code": -32700,
                        "message": format!("Parse error: {}", e)
                    }
                });
                return Ok(Some(serde_json::to_string(&err_resp)?));
            }
        };

        if let Some(resp_val) = self.handle_jsonrpc_value(val) {
            Ok(Some(serde_json::to_string(&resp_val)?))
        } else {
            Ok(None)
        }
    }

    pub fn handle_jsonrpc_value(&self, req: Value) -> Option<Value> {
        if let Value::Array(arr) = req {
            // JSON-RPC 2.0 section6: an empty batch is answered with a single Invalid Request error
            if arr.is_empty() {
                return Some(Self::err_response(
                    None,
                    -32600,
                    "Invalid Request: empty batch",
                ));
            }
            let mut responses = Vec::new();
            for item in arr {
                if let Some(r) = self.handle_single_request(item) {
                    responses.push(r);
                }
            }
            if responses.is_empty() {
                None
            } else {
                Some(Value::Array(responses))
            }
        } else {
            self.handle_single_request(req)
        }
    }

    fn handle_single_request(&self, req: Value) -> Option<Value> {
        if !req.is_object() {
            return Some(Self::err_response(
                None,
                -32600,
                "Invalid Request: expected a JSON-RPC request object",
            ));
        }
        let id = req.get("id").cloned();
        // JSON-RPC notifications have no `id` member; `"id": null` is answered like a request
        let is_notification = id.is_none();

        let method = match req.get("method").and_then(|m| m.as_str()) {
            Some(m) => m.to_string(),
            None => {
                return Some(Self::err_response(
                    id,
                    -32600,
                    "Invalid Request: missing or non-string 'method'",
                ));
            }
        };
        let params = req.get("params").cloned().unwrap_or_else(|| json!({}));

        let response = match method.as_str() {
            "notifications/initialized" | "initialized" | "notifications/cancelled" => {
                Some(Self::ok_response(id, json!({})))
            }
            "ping" => Some(Self::ok_response(id, json!({}))),
            "initialize" => {
                let requested = params.get("protocolVersion").and_then(|v| v.as_str());
                let res = json!({
                    "protocolVersion": negotiate_protocol_version(requested),
                    "capabilities": {
                        "tools": {
                            "listChanged": false
                        }
                    },
                    "serverInfo": {
                        "name": "forge-mcp",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "instructions": SERVER_INSTRUCTIONS
                });
                Some(Self::ok_response(id, res))
            }
            "tools/list" => {
                let res = json!({
                    "tools": Self::tool_definitions()
                });
                Some(Self::ok_response(id, res))
            }
            "tools/call" => {
                let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));

                match self.dispatch_tool(tool_name, args) {
                    Ok(envelope_val) => {
                        let serialized = serde_json::to_string(&envelope_val)
                            .unwrap_or_else(|_| "{}".to_string());
                        let res = json!({
                            "content": [
                                {
                                    "type": "text",
                                    "text": serialized
                                }
                            ],
                            "structuredContent": envelope_val,
                            "isError": false
                        });
                        Some(Self::ok_response(id, res))
                    }
                    Err(err) => Some(Self::err_response(
                        id,
                        -32000,
                        &format!("Tool '{}' failed: {}", tool_name, err),
                    )),
                }
            }
            _ => Some(Self::err_response(
                id,
                -32601,
                &format!("Method not found: {}", method),
            )),
        };

        // JSON-RPC 2.0 section4.1: the server MUST NOT reply to a notification
        if is_notification {
            None
        } else {
            response
        }
    }

    fn dispatch_tool(&self, name: &str, args: Value) -> Result<Value> {
        match name {
            "workspace_open" => {
                let req: WorkspaceOpenRequest = serde_json::from_value(args)?;
                let env = self
                    .store
                    .open_workspace(&req, &self.embedder, &self.allowed_roots)?;
                Ok(serde_json::to_value(env)?)
            }
            "workspace_status" => {
                let req: WorkspaceStatusRequest = serde_json::from_value(args)?;
                let env = self.store.workspace_status(&req)?;
                Ok(serde_json::to_value(env)?)
            }
            "code_search" => {
                let req: CodeSearchRequest = serde_json::from_value(args)?;
                let env = code_search(&self.store, &self.embedder, &req)?;
                Ok(serde_json::to_value(env)?)
            }
            "symbol_find" => {
                let req: SymbolFindRequest = serde_json::from_value(args)?;
                let env = self.store.find_symbols(&req)?;
                Ok(serde_json::to_value(env)?)
            }
            "symbol_read" => {
                let req: SymbolReadRequest = serde_json::from_value(args)?;
                let env = self.store.read_symbol(&req)?;
                Ok(serde_json::to_value(env)?)
            }
            "symbol_references" => {
                let req: SymbolReferencesRequest = serde_json::from_value(args)?;
                let env = self.store.symbol_references(&req)?;
                Ok(serde_json::to_value(env)?)
            }
            "graph_query" => {
                let req: GraphQueryRequest = serde_json::from_value(args)?;
                let env = graph_query(&self.store, &req)?;
                Ok(serde_json::to_value(env)?)
            }
            "impact_analyze" => {
                let req: ImpactAnalyzeRequest = serde_json::from_value(args)?;
                let env = impact_analyze(&self.store, &req)?;
                Ok(serde_json::to_value(env)?)
            }
            "context_build" => {
                let req: ContextBuildRequest = serde_json::from_value(args)?;
                let env = context_build(&self.store, &self.embedder, &req)?;
                Ok(serde_json::to_value(env)?)
            }
            "job_status" => {
                let req: JobStatusRequest = serde_json::from_value(args)?;
                let env = self.store.job_status(&req)?;
                Ok(serde_json::to_value(env)?)
            }
            "job_cancel" => {
                let req: JobCancelRequest = serde_json::from_value(args)?;
                let env = self.store.cancel_job(&req)?;
                Ok(serde_json::to_value(env)?)
            }
            other => Err(anyhow::anyhow!("Unknown tool: {}", other)),
        }
    }

    fn ok_response(id: Option<Value>, result: Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(Value::Null),
            "result": result
        })
    }

    fn err_response(id: Option<Value>, code: i32, message: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(Value::Null),
            "error": {
                "code": code,
                "message": message
            }
        })
    }

    pub fn tool_definitions() -> Vec<Value> {
        vec![
            json!({
                "name": "workspace_open",
                "description": "Register and index a workspace directory (respecting .gitignore and path allowlists), creating a content-hashed Snapshot.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Filesystem path to the workspace root directory" },
                        "workspace_id": { "type": "string", "description": "Optional explicit workspace identifier" },
                        "force_reindex": { "type": "boolean", "description": "Force full re-index even if snapshot hash is unchanged" }
                    },
                    "required": ["path"]
                }
            }),
            json!({
                "name": "workspace_status",
                "description": "Inspect active workspace metadata, indexed file/symbol/edge counts, index generation, and live filesystem freshness status.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workspace_id": { "type": "string", "description": "Workspace identifier" }
                    },
                    "required": ["workspace_id"]
                }
            }),
            json!({
                "name": "code_search",
                "description": "Hybrid codebase search (BM25 lexical + AST symbol match + vector similarity with Reciprocal Rank Fusion) across chunks and symbols.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workspace_id": { "type": "string", "description": "Workspace identifier" },
                        "query": { "type": "string", "description": "Search query (identifiers, keywords, or natural language)" },
                        "language": { "type": "string", "description": "Optional language filter (rust, typescript, javascript, python, go)" },
                        "path_prefix": { "type": "string", "description": "Optional relative file path prefix filter" },
                        "limit": { "type": "integer", "description": "Maximum number of results to return (default 15, clamped to 1..100)" },
                        "include_tests": { "type": "boolean", "description": "Whether to include test files (default true)" }
                    },
                    "required": ["workspace_id", "query"]
                }
            }),
            json!({
                "name": "symbol_find",
                "description": "Locate symbols by exact, prefix, or substring name, kind (function, class, struct, interface, method, trait, module), and file path.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workspace_id": { "type": "string", "description": "Workspace identifier" },
                        "name": { "type": "string", "description": "Symbol name or substring to match" },
                        "match_mode": { "type": "string", "enum": ["exact", "prefix", "substring"], "description": "Matching mode (default substring)" },
                        "kind": { "type": "string", "description": "Optional symbol kind filter (function, struct, class, interface, method, trait, module)" },
                        "file_path": { "type": "string", "description": "Optional file path filter" },
                        "language": { "type": "string", "description": "Optional language filter" },
                        "limit": { "type": "integer", "description": "Maximum results (default 50, clamped to 1..500)" }
                    },
                    "required": ["workspace_id", "name"]
                }
            }),
            json!({
                "name": "symbol_read",
                "description": "Read complete symbol source body, signature, doc comments, and surrounding lines with exact line ranges and content hash. Provide symbol_id, or name (optionally with file_path to disambiguate).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workspace_id": { "type": "string", "description": "Workspace identifier" },
                        "symbol_id": { "type": "string", "description": "Symbol ID from symbol_find" },
                        "name": { "type": "string", "description": "Symbol name if symbol_id is not provided" },
                        "file_path": { "type": "string", "description": "Optional file path disambiguator" },
                        "context_lines": { "type": "integer", "description": "Surrounding context lines for enclosing_scope (default 3)" }
                    },
                    "required": ["workspace_id"]
                }
            }),
            json!({
                "name": "symbol_references",
                "description": "Find all recorded reference and call sites for a symbol across the workspace with surrounding context lines.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workspace_id": { "type": "string", "description": "Workspace identifier" },
                        "symbol_id": { "type": "string", "description": "Target symbol ID" },
                        "symbol_name": { "type": "string", "description": "Target symbol name" },
                        "name": { "type": "string", "description": "Alias for symbol_name" },
                        "symbol": { "type": "string", "description": "Alias for symbol_name" },
                        "file_path": { "type": "string", "description": "Optional file path: restricts hits to this file and disambiguates the target symbol" },
                        "include_declarations": { "type": "boolean", "description": "Prepend the symbol's declaration site as a hit with reference_kind \"declaration\" (default false)" },
                        "limit": { "type": "integer", "description": "Maximum references to return (default 100, clamped to 1..500)" }
                    },
                    "required": ["workspace_id"]
                }
            }),
            json!({
                "name": "graph_query",
                "description": "Query bounded relationships (callers, callees, imports, importers, children, implements, path) up to a configurable max_depth.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workspace_id": { "type": "string", "description": "Workspace identifier" },
                        "query_kind": { "type": "string", "enum": ["callers", "callees", "imports", "importers", "children", "implements", "path"], "description": "Relationship query kind" },
                        "direction": { "type": "string", "enum": ["callers", "callees", "imports", "importers", "children", "implements", "path"], "description": "Alias for query_kind" },
                        "kind": { "type": "string", "enum": ["callers", "callees", "imports", "importers", "children", "implements", "path"], "description": "Alias for query_kind" },
                        "relation": { "type": "string", "enum": ["callers", "callees", "imports", "importers", "children", "implements", "path"], "description": "Alias for query_kind" },
                        "symbol_id": { "type": "string", "description": "Starting symbol ID" },
                        "symbol_name": { "type": "string", "description": "Starting symbol name" },
                        "name": { "type": "string", "description": "Alias for symbol_name" },
                        "symbol": { "type": "string", "description": "Alias for symbol_name" },
                        "file_path": { "type": "string", "description": "Optional file path for file-level import queries" },
                        "target_symbol_id": { "type": "string", "description": "Target symbol ID when query_kind is 'path'" },
                        "target_symbol_name": { "type": "string", "description": "Target symbol name when query_kind is 'path'" },
                        "max_depth": { "type": "integer", "description": "Maximum traversal depth (default 2, clamped to 1..8)" },
                        "limit": { "type": "integer", "description": "Maximum edges to return (default 100, clamped to 1..500)" }
                    },
                    "required": ["workspace_id"]
                }
            }),
            json!({
                "name": "impact_analyze",
                "description": "Compute blast radius of modifying a symbol or file (direct and transitive dependents, affected files, and impacted test files).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workspace_id": { "type": "string", "description": "Workspace identifier" },
                        "symbol_id": { "type": "string", "description": "Symbol ID being modified" },
                        "symbol_name": { "type": "string", "description": "Symbol name being modified" },
                        "name": { "type": "string", "description": "Alias for symbol_name" },
                        "symbol": { "type": "string", "description": "Alias for symbol_name" },
                        "file_path": { "type": "string", "description": "File path being modified" },
                        "max_depth": { "type": "integer", "description": "Maximum transitive depth (default 3, clamped to 1..8)" }
                    },
                    "required": ["workspace_id"]
                }
            }),
            json!({
                "name": "context_build",
                "description": "Assemble a token-budgeted, deduplicated, source-backed context package for a natural-language query using hybrid search + 1-hop graph expansion.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workspace_id": { "type": "string", "description": "Workspace identifier" },
                        "query": { "type": "string", "description": "Natural-language or code task query" },
                        "token_budget": { "type": "integer", "description": "Maximum token budget (default 4000; clamped to 64..=128000)" },
                        "include_tests": { "type": "boolean", "description": "Whether to include test files (default true)" },
                        "snapshot_policy": {
                            "type": "string",
                            "enum": ["require_current", "allow_stale", "require_current_strict"],
                            "description": "Snapshot freshness policy. require_current: fail if the active snapshot is stale (require_current_strict is an alias). allow_stale (default): proceed and report staleness in limitations. Any other value is rejected."
                        },
                        "language": { "type": "string", "description": "Optional language filter" },
                        "path_prefix": { "type": "string", "description": "Optional path prefix filter" }
                    },
                    "required": ["workspace_id", "query"]
                }
            }),
            json!({
                "name": "job_status",
                "description": "Inspect background or completed indexing job status. Provide job_id, or workspace_id to get that workspace's latest job.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "job_id": { "type": "string", "description": "Job ID returned by workspace_open" },
                        "workspace_id": { "type": "string", "description": "Optional workspace ID to look up latest job" }
                    }
                }
            }),
            json!({
                "name": "job_cancel",
                "description": "Cancel an in-progress indexing job.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "job_id": { "type": "string", "description": "Job ID to cancel" }
                    },
                    "required": ["job_id"]
                }
            }),
        ]
    }

    /// Run the MCP stdio server loop supporting both newline-delimited JSON-RPC
    /// and `Content-Length: <N>\r\n\r\n` framed JSON-RPC messages.
    pub fn run_stdio(&self) -> Result<()> {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        let mut reader = BufReader::new(stdin.lock());
        let mut writer = stdout.lock();

        loop {
            let mut raw_line = Vec::new();
            let n = reader.read_until(b'\n', &mut raw_line)?;
            if n == 0 {
                break; // EOF
            }

            // Invalid UTF-8 must not stop the server: answer with a parse error and keep serving
            let line = match String::from_utf8(raw_line) {
                Ok(s) => s,
                Err(_) => {
                    let err =
                        Self::err_response(None, -32700, "Parse error: message is not valid UTF-8");
                    writeln!(writer, "{}", serde_json::to_string(&err)?)?;
                    writer.flush()?;
                    continue;
                }
            };

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if trimmed.to_ascii_lowercase().starts_with("content-length:") {
                let len_str = trimmed["content-length:".len()..].trim();
                let content_len: usize = match len_str.parse() {
                    Ok(v) => v,
                    Err(_) => {
                        tracing::warn!("ignoring invalid Content-Length header: {:?}", len_str);
                        continue;
                    }
                };

                // Consume remaining HTTP-style headers until empty line
                loop {
                    let mut hdr = Vec::new();
                    let hn = reader.read_until(b'\n', &mut hdr)?;
                    if hn == 0 || hdr.iter().all(|b| b.is_ascii_whitespace()) {
                        break;
                    }
                }

                if content_len > MAX_STDIO_FRAME_BYTES {
                    // Drain the oversized body without buffering it, then reject the frame
                    let drained = std::io::copy(
                        &mut Read::take(&mut reader, content_len as u64),
                        &mut std::io::sink(),
                    )?;
                    let err = serde_json::to_string(&Self::err_response(
                        None,
                        -32600,
                        &format!(
                            "Invalid Request: Content-Length {} exceeds limit of {} bytes",
                            content_len, MAX_STDIO_FRAME_BYTES
                        ),
                    ))?;
                    write!(writer, "Content-Length: {}\r\n\r\n{}", err.len(), err)?;
                    writer.flush()?;
                    if (drained as usize) < content_len {
                        break; // EOF inside the oversized body
                    }
                    continue;
                }

                let mut body_buf = vec![0u8; content_len];
                match reader.read_exact(&mut body_buf) {
                    Ok(()) => {}
                    // Client closed stdin mid-frame: treat as EOF, not as a server failure
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                        tracing::warn!(
                            "stdin closed inside a {}-byte Content-Length frame",
                            content_len
                        );
                        break;
                    }
                    Err(e) => return Err(e.into()),
                }
                let body_str = String::from_utf8_lossy(&body_buf);

                if let Some(resp_json) = self.handle_jsonrpc_str(&body_str)? {
                    write!(
                        writer,
                        "Content-Length: {}\r\n\r\n{}",
                        resp_json.len(),
                        resp_json
                    )?;
                    writer.flush()?;
                }
            } else {
                // Line-delimited JSON-RPC
                if let Some(resp_json) = self.handle_jsonrpc_str(trimmed)? {
                    writeln!(writer, "{}", resp_json)?;
                    writer.flush()?;
                }
            }
        }

        Ok(())
    }
}
