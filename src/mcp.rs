use crate::embeddings::EmbeddingEngine;
use crate::graph::{graph_query, impact_analyze};
use crate::model::*;
use crate::search::{code_search, context_build};
use crate::store::ForgeStore;
use anyhow::Result;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;

#[derive(Clone)]
pub struct ForgeMcpServer {
    pub store: ForgeStore,
    pub embedder: EmbeddingEngine,
    pub allowed_roots: Vec<PathBuf>,
}

impl ForgeMcpServer {
    pub fn new(
        store: ForgeStore,
        embedder: EmbeddingEngine,
        allowed_roots: Vec<PathBuf>,
    ) -> Self {
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
        if let Some(arr) = req.as_array() {
            let mut responses = Vec::new();
            for item in arr {
                if let Some(r) = self.handle_single_request(item.clone()) {
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
        let id = req.get("id").cloned();
        let method = req
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        let params = req.get("params").cloned().unwrap_or_else(|| json!({}));

        // JSON-RPC notifications have no `id` field (or null in some clients)
        let is_notification = id.is_none();

        match method.as_str() {
            "notifications/initialized" | "initialized" | "notifications/cancelled" => {
                if is_notification {
                    return None;
                }
                return Some(Self::ok_response(id, json!({})));
            }
            "ping" => {
                return Some(Self::ok_response(id, json!({})));
            }
            "initialize" => {
                let client_proto = params
                    .get("protocolVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("2024-11-05");
                let res = json!({
                    "protocolVersion": client_proto,
                    "capabilities": {
                        "tools": {
                            "listChanged": false
                        }
                    },
                    "serverInfo": {
                        "name": "forge-mcp",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                });
                return Some(Self::ok_response(id, res));
            }
            "tools/list" => {
                let res = json!({
                    "tools": Self::tool_definitions()
                });
                return Some(Self::ok_response(id, res));
            }
            "tools/call" => {
                let tool_name = params
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));

                match self.dispatch_tool(tool_name, args) {
                    Ok(envelope_val) => {
                        let serialized =
                            serde_json::to_string(&envelope_val).unwrap_or_else(|_| "{}".to_string());
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
            _ => {
                if is_notification {
                    None
                } else {
                    Some(Self::err_response(
                        id,
                        -32601,
                        &format!("Method not found: {}", method),
                    ))
                }
            }
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
                        "limit": { "type": "integer", "description": "Maximum number of results to return (default 15)" },
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
                        "limit": { "type": "integer", "description": "Maximum results (default 50)" }
                    },
                    "required": ["workspace_id", "name"]
                }
            }),
            json!({
                "name": "symbol_read",
                "description": "Read complete symbol source body, signature, doc comments, and enclosing scope with exact line ranges and content hash.",
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
                        "file_path": { "type": "string", "description": "Optional file path disambiguator" },
                        "include_declarations": { "type": "boolean", "description": "Include declaration site" },
                        "limit": { "type": "integer", "description": "Maximum references to return (default 100)" }
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
                        "symbol_id": { "type": "string", "description": "Starting symbol ID" },
                        "symbol_name": { "type": "string", "description": "Starting symbol name" },
                        "name": { "type": "string", "description": "Alias for symbol_name" },
                        "file_path": { "type": "string", "description": "Optional file path for file-level import queries" },
                        "target_symbol_id": { "type": "string", "description": "Target symbol ID when query_kind is 'path'" },
                        "target_symbol_name": { "type": "string", "description": "Target symbol name when query_kind is 'path'" },
                        "max_depth": { "type": "integer", "description": "Maximum traversal depth (default 2, max 8)" },
                        "limit": { "type": "integer", "description": "Maximum edges to return (default 100)" }
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
                        "file_path": { "type": "string", "description": "File path being modified" },
                        "max_depth": { "type": "integer", "description": "Maximum transitive depth (default 3)" }
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
                        "token_budget": { "type": "integer", "description": "Maximum token budget (default 4000)" },
                        "include_tests": { "type": "boolean", "description": "Whether to include test files (default true)" },
                        "snapshot_policy": { "type": "string", "description": "Snapshot freshness policy (require_current, allow_stale)" },
                        "language": { "type": "string", "description": "Optional language filter" },
                        "path_prefix": { "type": "string", "description": "Optional path prefix filter" }
                    },
                    "required": ["workspace_id", "query"]
                }
            }),
            json!({
                "name": "job_status",
                "description": "Inspect background or completed indexing job status.",
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
            let mut line = String::new();
            let n = reader.read_line(&mut line)?;
            if n == 0 {
                break; // EOF
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if trimmed.to_ascii_lowercase().starts_with("content-length:") {
                let len_str = trimmed["content-length:".len()..].trim();
                let content_len: usize = match len_str.parse() {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                // Consume remaining HTTP-style headers until empty line
                loop {
                    let mut hdr = String::new();
                    let hn = reader.read_line(&mut hdr)?;
                    if hn == 0 || hdr.trim().is_empty() {
                        break;
                    }
                }

                let mut body_buf = vec![0u8; content_len];
                reader.read_exact(&mut body_buf)?;
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
