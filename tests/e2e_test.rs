use forge_mcp::embeddings::EmbeddingEngine;
use forge_mcp::graph::{graph_query, impact_analyze};
use forge_mcp::http::build_http_router;
use forge_mcp::mcp::ForgeMcpServer;
use forge_mcp::model::*;
use forge_mcp::search::{code_search, context_build};
use forge_mcp::store::ForgeStore;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use tempfile::TempDir;

/// Create a realistic multi-language fixture workspace (Rust, TypeScript, Python, Go + tests).
fn create_multilang_workspace() -> TempDir {
    let tmp = tempfile::tempdir().expect("create temp workspace");
    let root = tmp.path();

    let src_dir = root.join("src");
    let tests_dir = root.join("tests");
    fs::create_dir_all(&src_dir).expect("create src dir");
    fs::create_dir_all(&tests_dir).expect("create tests dir");

    // 1. Rust fixture (src/auth.rs)
    fs::write(
        src_dir.join("auth.rs"),
        r#"/// Trait for token verification backends.
pub trait Verifier {
    fn verify(&self, token: &str) -> bool;
}

/// Validates HMAC-signed bearer tokens.
pub struct TokenValidator {
    pub secret_salt: String,
}

impl Verifier for TokenValidator {
    /// Verify a bearer token using the configured salt.
    pub fn verify(&self, token: &str) -> bool {
        verify_hmac_token(token, &self.secret_salt)
    }
}

/// Computes a deterministic keyed HMAC digest for a secret payload.
pub fn compute_hmac_digest(payload: &str, salt: &str) -> String {
    format!("hmac:{}:{}", salt, payload)
}

/// Verifies whether a raw token produces a non-empty HMAC digest.
pub fn verify_hmac_token(token: &str, salt: &str) -> bool {
    let digest = compute_hmac_digest(token, salt);
    !digest.is_empty()
}
"#,
    )
    .expect("write src/auth.rs");

    // 2. TypeScript fixture (src/gateway.ts)
    fs::write(
        src_dir.join("gateway.ts"),
        r#"import { AuthClient } from "./auth_client";

export interface GatewayHandler {
    handleRequest(header: string): boolean;
}

/**
 * Sanitizes an incoming HTTP Authorization Bearer header.
 */
export function sanitizeBearerHeader(header: string): string {
    return header.replace(/^Bearer\s+/i, "").trim();
}

export class ApiGateway implements GatewayHandler {
    handleRequest(header: string): boolean {
        const token = sanitizeBearerHeader(header);
        return token.length > 0;
    }
}
"#,
    )
    .expect("write src/gateway.ts");

    // 3. Python fixture (src/pipeline.py)
    fs::write(
        src_dir.join("pipeline.py"),
        r#"import hashlib

class BaseWorker:
    pass

class IndexingWorker(BaseWorker):
    """Executes batch indexing jobs over repository snapshots."""
    def run_batch(self, items: list) -> int:
        return normalize_batch_payload(items)

def normalize_batch_payload(items: list) -> int:
    """Normalizes and counts items in an indexing batch."""
    return len(items)
"#,
    )
    .expect("write src/pipeline.py");

    // 4. Go fixture (src/router.go)
    fs::write(
        src_dir.join("router.go"),
        r#"package router

import "fmt"

type RouteDispatcher interface {
    Dispatch(path string) int
}

type HttpRouter struct {
    Prefix string
}

// ValidateRoutePath checks if the request route path is non-empty.
func ValidateRoutePath(path string) bool {
    return len(path) > 0
}

func (r *HttpRouter) Dispatch(path string) int {
    if ValidateRoutePath(path) {
        return 200
    }
    return 404
}
"#,
    )
    .expect("write src/router.go");

    // 5. Rust test fixture (tests/auth_test.rs)
    fs::write(
        tests_dir.join("auth_test.rs"),
        r#"use crate::auth::verify_hmac_token;

#[test]
fn test_verify_hmac_token_valid() {
    let ok = verify_hmac_token("session-token-42", "pepper");
    assert!(ok);
}
"#,
    )
    .expect("write tests/auth_test.rs");

    tmp
}

#[test]
fn test_multilang_indexing_and_snapshot_freshness_lifecycle() {
    let ws_dir = create_multilang_workspace();
    let store = ForgeStore::open_in_memory().expect("open in-memory store");
    let embedder = EmbeddingEngine::default();

    // 1. Open and index workspace
    let open_env = store
        .open_workspace(
            &WorkspaceOpenRequest {
                path: ws_dir.path().to_string_lossy().into_owned(),
                workspace_id: Some("ws-multilang".to_string()),
                force_reindex: Some(false),
            },
            &embedder,
            &[],
        )
        .expect("open_workspace should succeed");

    assert_eq!(open_env.freshness, Freshness::Current);
    assert_eq!(open_env.results.len(), 1);
    assert!(!open_env.evidence.is_empty());

    let open_res = &open_env.results[0];
    assert_eq!(open_res.workspace_id, "ws-multilang");
    assert_eq!(open_res.file_count, 5);
    assert!(open_res.symbol_count >= 12);
    assert!(open_res.reference_count >= 5);
    assert!(open_res.relationship_count >= 5);
    assert!(open_res.chunk_count >= 10);
    assert!(!open_res.reused_snapshot);
    let snap1 = open_res.snapshot_id.clone();

    // 2. Re-opening without changes reuses snapshot
    let reopen_env = store
        .open_workspace(
            &WorkspaceOpenRequest {
                path: ws_dir.path().to_string_lossy().into_owned(),
                workspace_id: Some("ws-multilang".to_string()),
                force_reindex: Some(false),
            },
            &embedder,
            &[],
        )
        .expect("reopen_workspace should succeed");
    assert!(reopen_env.results[0].reused_snapshot);
    assert_eq!(reopen_env.snapshot_id, snap1);

    // 3. Verify workspace_status is "current"
    let status_cur = store
        .workspace_status(&WorkspaceStatusRequest {
            workspace_id: "ws-multilang".to_string(),
        })
        .expect("workspace_status current");
    assert_eq!(status_cur.freshness, Freshness::Current);
    assert!(status_cur.results[0].changed_files.is_empty());

    // 4. Verify symbols across all 4 languages (Rust, TypeScript, Python, Go)
    for (lang, expected_sym) in [
        ("rust", "verify_hmac_token"),
        ("typescript", "sanitizeBearerHeader"),
        ("python", "normalize_batch_payload"),
        ("go", "ValidateRoutePath"),
    ] {
        let found = store
            .find_symbols(&SymbolFindRequest {
                workspace_id: "ws-multilang".to_string(),
                name: expected_sym.to_string(),
                match_mode: Some(MatchMode::Exact),
                kind: None,
                file_path: None,
                language: Some(lang.to_string()),
                limit: Some(10),
            })
            .expect("find_symbols by language");
        assert!(
            !found.results.is_empty(),
            "Expected symbol '{}' in language '{}'",
            expected_sym,
            lang
        );
        assert_eq!(found.results[0].symbol.name, expected_sym);
        assert_eq!(found.results[0].symbol.language, lang);
    }

    // 5. Modify a file on disk and verify freshness transitions to "stale"
    let auth_path = ws_dir.path().join("src").join("auth.rs");
    let mut current_auth = fs::read_to_string(&auth_path).expect("read auth.rs");
    current_auth.push_str("\npub fn rotate_salt() -> &'static str { \"new-salt\" }\n");
    fs::write(&auth_path, current_auth).expect("write modified auth.rs");

    let status_stale = store
        .workspace_status(&WorkspaceStatusRequest {
            workspace_id: "ws-multilang".to_string(),
        })
        .expect("workspace_status after modification");
    assert_eq!(status_stale.freshness, Freshness::Stale);
    assert!(
        status_stale.results[0]
            .changed_files
            .contains(&"src/auth.rs".to_string()),
        "changed_files should list src/auth.rs"
    );

    // 6. Re-index workspace and verify freshness returns to "current" with new generation
    let reindex_env = store
        .open_workspace(
            &WorkspaceOpenRequest {
                path: ws_dir.path().to_string_lossy().into_owned(),
                workspace_id: Some("ws-multilang".to_string()),
                force_reindex: Some(false),
            },
            &embedder,
            &[],
        )
        .expect("reindex workspace");
    assert_eq!(reindex_env.freshness, Freshness::Current);
    assert!(!reindex_env.results[0].reused_snapshot);
    assert_eq!(reindex_env.results[0].index_generation, 2);
    assert_ne!(reindex_env.snapshot_id, snap1);
}

#[test]
fn test_hybrid_search_symbol_intelligence_graph_impact_and_context_budget() {
    let ws_dir = create_multilang_workspace();
    let store = ForgeStore::open_in_memory().expect("open in-memory store");
    let embedder = EmbeddingEngine::default();

    store
        .open_workspace(
            &WorkspaceOpenRequest {
                path: ws_dir.path().to_string_lossy().into_owned(),
                workspace_id: Some("ws-intel".to_string()),
                force_reindex: None,
            },
            &embedder,
            &[],
        )
        .expect("open workspace");

    // 1. Hybrid search (BM25 + AST symbol + vector similarity with RRF)
    let search_env = code_search(
        &store,
        &embedder,
        &CodeSearchRequest {
            workspace_id: "ws-intel".to_string(),
            query: "compute_hmac_digest keyed HMAC digest".to_string(),
            language: Some("rust".to_string()),
            path_prefix: Some("src/".to_string()),
            limit: Some(5),
            include_tests: Some(false),
        },
    )
    .expect("code_search");

    assert_eq!(search_env.freshness, Freshness::Current);
    assert!(!search_env.results.is_empty());
    let top_hit = &search_env.results[0];
    assert_eq!(top_hit.file_path, "src/auth.rs");
    assert!(top_hit.rrf_score > 0.0);
    assert!(!top_hit.match_reasons.is_empty());

    // 2. Symbol find & read with doc comments and enclosing scope
    let find_env = store
        .find_symbols(&SymbolFindRequest {
            workspace_id: "ws-intel".to_string(),
            name: "compute_hmac_digest".to_string(),
            match_mode: Some(MatchMode::Exact),
            kind: Some("function".to_string()),
            file_path: Some("src/auth.rs".to_string()),
            language: Some("rust".to_string()),
            limit: Some(5),
        })
        .expect("symbol_find");
    assert_eq!(find_env.results.len(), 1);
    let digest_sym = &find_env.results[0].symbol;

    let read_env = store
        .read_symbol(&SymbolReadRequest {
            workspace_id: "ws-intel".to_string(),
            symbol_id: Some(digest_sym.symbol_id.clone()),
            name: None,
            file_path: None,
            context_lines: Some(2),
        })
        .expect("symbol_read");
    assert_eq!(read_env.results.len(), 1);
    assert!(read_env.results[0].body.contains("hmac:{}:{}"));
    assert!(read_env.results[0]
        .symbol
        .doc_comment
        .as_deref()
        .unwrap_or("")
        .contains("HMAC digest"));

    // 3. Symbol references across source and test files
    let refs_env = store
        .symbol_references(&SymbolReferencesRequest {
            workspace_id: "ws-intel".to_string(),
            symbol_id: None,
            symbol_name: Some("verify_hmac_token".to_string()),
            name: None,
            symbol: None,
            file_path: None,
            include_declarations: None,
            limit: Some(20),
        })
        .expect("symbol_references");
    assert!(
        refs_env.results.len() >= 2,
        "verify_hmac_token should be referenced in src/auth.rs and tests/auth_test.rs"
    );
    let ref_files: Vec<&str> = refs_env
        .results
        .iter()
        .map(|r| r.reference.file_path.as_str())
        .collect();
    assert!(ref_files.contains(&"src/auth.rs"));
    assert!(ref_files.contains(&"tests/auth_test.rs"));

    // 4. Graph queries: callers, callees, children, implements, imports, importers, path
    let callers_env = graph_query(
        &store,
        &GraphQueryRequest {
            workspace_id: "ws-intel".to_string(),
            query_kind: Some("callers".to_string()),
            direction: None,
            kind: None,
            relation: None,
            symbol_id: None,
            symbol_name: Some("compute_hmac_digest".to_string()),
            name: None,
            symbol: None,
            file_path: None,
            target_symbol_id: None,
            target_symbol_name: None,
            max_depth: Some(2),
            limit: Some(20),
        },
    )
    .expect("graph_query callers");
    assert!(!callers_env.results[0].hops.is_empty());

    let callees_env = graph_query(
        &store,
        &GraphQueryRequest {
            workspace_id: "ws-intel".to_string(),
            query_kind: Some("callees".to_string()),
            direction: None,
            kind: None,
            relation: None,
            symbol_id: None,
            symbol_name: Some("verify_hmac_token".to_string()),
            name: None,
            symbol: None,
            file_path: None,
            target_symbol_id: None,
            target_symbol_name: None,
            max_depth: Some(2),
            limit: Some(20),
        },
    )
    .expect("graph_query callees");
    assert!(callees_env.results[0]
        .hops
        .iter()
        .any(|h| h.relationship.to_name == "compute_hmac_digest"));

    let children_env = graph_query(
        &store,
        &GraphQueryRequest {
            workspace_id: "ws-intel".to_string(),
            query_kind: Some("children".to_string()),
            direction: None,
            kind: None,
            relation: None,
            symbol_id: None,
            symbol_name: Some("TokenValidator".to_string()),
            name: None,
            symbol: None,
            file_path: None,
            target_symbol_id: None,
            target_symbol_name: None,
            max_depth: Some(1),
            limit: Some(20),
        },
    )
    .expect("graph_query children");
    assert!(children_env.results[0]
        .hops
        .iter()
        .any(|h| h.relationship.to_name == "verify"));

    let implements_env = graph_query(
        &store,
        &GraphQueryRequest {
            workspace_id: "ws-intel".to_string(),
            query_kind: Some("implements".to_string()),
            direction: None,
            kind: None,
            relation: None,
            symbol_id: None,
            symbol_name: Some("TokenValidator".to_string()),
            name: None,
            symbol: None,
            file_path: None,
            target_symbol_id: None,
            target_symbol_name: None,
            max_depth: Some(1),
            limit: Some(20),
        },
    )
    .expect("graph_query implements");
    assert!(implements_env.results[0]
        .hops
        .iter()
        .any(|h| h.relationship.to_name == "Verifier"));

    let imports_env = graph_query(
        &store,
        &GraphQueryRequest {
            workspace_id: "ws-intel".to_string(),
            query_kind: Some("imports".to_string()),
            direction: None,
            kind: None,
            relation: None,
            symbol_id: None,
            symbol_name: None,
            name: None,
            symbol: None,
            file_path: Some("src/gateway.ts".to_string()),
            target_symbol_id: None,
            target_symbol_name: None,
            max_depth: Some(1),
            limit: Some(20),
        },
    )
    .expect("graph_query imports");
    assert!(!imports_env.results[0].hops.is_empty());

    let importers_env = graph_query(
        &store,
        &GraphQueryRequest {
            workspace_id: "ws-intel".to_string(),
            query_kind: Some("importers".to_string()),
            direction: None,
            kind: None,
            relation: None,
            symbol_id: None,
            symbol_name: Some("src/auth.rs".to_string()),
            name: None,
            symbol: None,
            file_path: Some("src/auth.rs".to_string()),
            target_symbol_id: None,
            target_symbol_name: None,
            max_depth: Some(1),
            limit: Some(20),
        },
    )
    .expect("graph_query importers");
    assert!(!importers_env.results[0].hops.is_empty());

    let path_env = graph_query(
        &store,
        &GraphQueryRequest {
            workspace_id: "ws-intel".to_string(),
            query_kind: Some("path".to_string()),
            direction: None,
            kind: None,
            relation: None,
            symbol_id: None,
            symbol_name: Some("verify".to_string()),
            name: None,
            symbol: None,
            file_path: Some("src/auth.rs".to_string()),
            target_symbol_id: None,
            target_symbol_name: Some("compute_hmac_digest".to_string()),
            max_depth: Some(4),
            limit: Some(20),
        },
    )
    .expect("graph_query path");
    assert_eq!(path_env.results[0].path_found, Some(true));
    assert_eq!(path_env.results[0].hops.len(), 2);

    // 5. Impact analysis (blast radius including direct, transitive, and test files)
    let impact_env = impact_analyze(
        &store,
        &ImpactAnalyzeRequest {
            workspace_id: "ws-intel".to_string(),
            symbol_id: None,
            symbol_name: Some("compute_hmac_digest".to_string()),
            name: None,
            symbol: None,
            file_path: Some("src/auth.rs".to_string()),
            max_depth: Some(3),
        },
    )
    .expect("impact_analyze");
    let impact_res = &impact_env.results[0];
    assert!(!impact_res.direct_dependents.is_empty());
    assert!(!impact_res.transitive_dependents.is_empty());
    assert!(impact_res
        .affected_files
        .contains(&"src/auth.rs".to_string()));
    assert!(impact_res
        .impacted_test_files
        .contains(&"tests/auth_test.rs".to_string()));

    // 6. Context build with strict token budget enforcement
    let tight_budget = 64usize;
    let ctx_env = context_build(
        &store,
        &embedder,
        &ContextBuildRequest {
            workspace_id: "ws-intel".to_string(),
            query: "how are bearer tokens sanitized and HMAC digests verified?".to_string(),
            token_budget: Some(tight_budget),
            include_tests: Some(true),
            snapshot_policy: Some("require_current".to_string()),
            language: None,
            path_prefix: None,
        },
    )
    .expect("context_build tight budget");
    let ctx_res = &ctx_env.results[0];
    assert!(
        ctx_res.total_tokens_used <= tight_budget,
        "total_tokens_used ({}) must not exceed budget ({})",
        ctx_res.total_tokens_used,
        tight_budget
    );
    assert!(!ctx_res.snippets.is_empty());

    // Generous budget should include 1-hop graph expansion items
    let full_ctx_env = context_build(
        &store,
        &embedder,
        &ContextBuildRequest {
            workspace_id: "ws-intel".to_string(),
            query: "verify_hmac_token compute_hmac_digest".to_string(),
            token_budget: Some(4000),
            include_tests: Some(true),
            snapshot_policy: None,
            language: None,
            path_prefix: None,
        },
    )
    .expect("context_build generous budget");
    assert!(full_ctx_env.results[0].primary_matches >= 1);
    assert!(full_ctx_env.results[0].graph_expanded_items >= 1);

    // 7. Job status & cancellation
    let job_env = store
        .job_status(&JobStatusRequest {
            job_id: None,
            workspace_id: Some("ws-intel".to_string()),
        })
        .expect("job_status");
    let job_id = job_env.results[0].job_id.clone();
    assert_eq!(job_env.results[0].status, JobStatusState::Completed);

    // Insert a synthetic running job and verify job_cancel transitions it to Cancelled
    store
        .insert_job(&JobRecord {
            job_id: "job-running-test".to_string(),
            workspace_id: "ws-intel".to_string(),
            kind: "workspace_index".to_string(),
            status: JobStatusState::Running,
            progress_pct: 40,
            processed_files: 2,
            total_files: 5,
            snapshot_id: None,
            error: None,
            created_at: "2026-10-26T00:00:00Z".to_string(),
            updated_at: "2026-10-26T00:00:00Z".to_string(),
        })
        .expect("insert synthetic job");

    let cancelled_env = store
        .cancel_job(&JobCancelRequest {
            job_id: "job-running-test".to_string(),
        })
        .expect("cancel_job");
    assert_eq!(cancelled_env.results[0].status, JobStatusState::Cancelled);
    assert!(store.is_job_cancelled("job-running-test"));
    let _ = job_id;
}

#[test]
fn test_stdio_jsonrpc_e2e_subprocess() {
    let ws_dir = create_multilang_workspace();
    let db_path = ws_dir.path().join("stdio-e2e.db");
    let bin_path = env!("CARGO_BIN_EXE_forge-mcp");

    let mut child = Command::new(bin_path)
        .args([
            "--transport",
            "stdio",
            "--db-path",
            &db_path.to_string_lossy(),
            "--allow-root",
            &ws_dir.path().to_string_lossy(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn forge-mcp stdio process");

    let mut stdin = child.stdin.take().expect("take child stdin");
    let stdout = child.stdout.take().expect("take child stdout");
    let mut reader = BufReader::new(stdout);

    // 1. Test line-delimited initialize request
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "rust-e2e", "version": "0.1.0" }
        }
    });
    writeln!(stdin, "{}", init_req).expect("write initialize");
    stdin.flush().expect("flush initialize");

    let mut line = String::new();
    reader.read_line(&mut line).expect("read initialize response");
    let init_resp: Value = serde_json::from_str(&line).expect("parse initialize JSON");
    assert_eq!(init_resp["id"], 1);
    assert_eq!(init_resp["result"]["serverInfo"]["name"], "forge-mcp");

    // 2. Test Content-Length framed tools/list request
    let list_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    })
    .to_string();
    write!(
        stdin,
        "Content-Length: {}\r\n\r\n{}",
        list_req.len(),
        list_req
    )
    .expect("write framed tools/list");
    stdin.flush().expect("flush framed tools/list");

    let mut header_line = String::new();
    reader
        .read_line(&mut header_line)
        .expect("read Content-Length header");
    assert!(
        header_line
            .to_ascii_lowercase()
            .starts_with("content-length:"),
        "Expected Content-Length header, got: {}",
        header_line
    );
    let content_len: usize = header_line["content-length:".len()..]
        .trim()
        .parse()
        .expect("parse Content-Length value");

    let mut blank_line = String::new();
    reader.read_line(&mut blank_line).expect("read CRLF separator");

    let mut body_buf = vec![0u8; content_len];
    reader
        .read_exact(&mut body_buf)
        .expect("read framed JSON-RPC body");
    let list_resp: Value = serde_json::from_slice(&body_buf).expect("parse tools/list JSON");
    assert_eq!(list_resp["id"], 2);
    let tools = list_resp["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert_eq!(tools.len(), 11);

    // 3. Call workspace_open over stdio
    let open_req = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "workspace_open",
            "arguments": {
                "path": ws_dir.path().to_string_lossy(),
                "workspace_id": "stdio-ws"
            }
        }
    });
    writeln!(stdin, "{}", open_req).expect("write workspace_open");
    stdin.flush().expect("flush workspace_open");

    line.clear();
    reader
        .read_line(&mut line)
        .expect("read workspace_open response");
    let open_resp: Value = serde_json::from_str(&line).expect("parse workspace_open JSON");
    assert_eq!(open_resp["id"], 3);
    assert_eq!(open_resp["result"]["isError"], false);
    assert_eq!(
        open_resp["result"]["structuredContent"]["freshness"],
        "current"
    );

    drop(stdin);
    let status = child.wait().expect("wait for child exit");
    assert!(status.success());
}

#[tokio::test]
async fn test_http_server_auth_allowlist_and_full_tool_invocation() {
    let ws_dir = create_multilang_workspace();
    let disallowed_dir = tempfile::tempdir().expect("create disallowed dir");

    let store = ForgeStore::open_in_memory().expect("open store");
    let embedder = EmbeddingEngine::default();
    let mcp = Arc::new(ForgeMcpServer::new(
        store,
        embedder,
        vec![PathBuf::from(ws_dir.path())],
    ));

    let secret = "forge-http-secret-token-2026".to_string();
    let app = build_http_router(mcp, Some(secret.clone()));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    let base_url = format!("http://{}", addr);

    let server_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = reqwest::Client::new();

    // 1. GET /health is publicly accessible without Bearer token
    let health_resp = client
        .get(format!("{}/health", base_url))
        .send()
        .await
        .expect("GET /health");
    assert_eq!(health_resp.status(), reqwest::StatusCode::OK);
    let health_json: Value = health_resp.json().await.expect("parse health JSON");
    assert_eq!(health_json["status"], "ok");
    assert_eq!(health_json["service"], "forge-mcp");

    // 2. POST /mcp without Authorization header returns 401 Unauthorized
    let unauth_resp = client
        .post(format!("{}/mcp", base_url))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .await
        .expect("POST /mcp unauthenticated");
    assert_eq!(unauth_resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    // 3. POST /mcp with wrong Bearer token returns 401 Unauthorized
    let bad_token_resp = client
        .post(format!("{}/mcp", base_url))
        .bearer_auth("wrong-token")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .await
        .expect("POST /mcp wrong token");
    assert_eq!(bad_token_resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    // 4. POST /mcp with valid Bearer token succeeds for workspace_open on allowed root
    let open_resp = client
        .post(format!("{}/mcp", base_url))
        .bearer_auth(&secret)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "workspace_open",
                "arguments": {
                    "path": ws_dir.path().to_string_lossy(),
                    "workspace_id": "http-ws"
                }
            }
        }))
        .send()
        .await
        .expect("POST /mcp workspace_open");
    assert_eq!(open_resp.status(), reqwest::StatusCode::OK);
    let open_body: Value = open_resp.json().await.expect("parse workspace_open body");
    assert_eq!(
        open_body["result"]["structuredContent"]["freshness"],
        "current"
    );

    // 5. POST /mcp with path outside allowlist fails with structured JSON-RPC error
    let forbidden_resp = client
        .post(format!("{}/mcp", base_url))
        .bearer_auth(&secret)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "workspace_open",
                "arguments": {
                    "path": disallowed_dir.path().to_string_lossy(),
                    "workspace_id": "forbidden-ws"
                }
            }
        }))
        .send()
        .await
        .expect("POST /mcp disallowed workspace");
    assert_eq!(forbidden_resp.status(), reqwest::StatusCode::OK);
    let forbidden_body: Value = forbidden_resp.json().await.expect("parse forbidden body");
    assert!(
        forbidden_body.get("error").is_some(),
        "Expected JSON-RPC error when opening path outside allowed roots"
    );
    assert!(forbidden_body["error"]["message"]
        .as_str()
        .unwrap_or("")
        .contains("outside allowed roots"));

    // 6. Invoke code_search over HTTP
    let search_resp = client
        .post(format!("{}/mcp", base_url))
        .bearer_auth(&secret)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "code_search",
                "arguments": {
                    "workspace_id": "http-ws",
                    "query": "ValidateRoutePath Dispatch"
                }
            }
        }))
        .send()
        .await
        .expect("POST /mcp code_search");
    assert_eq!(search_resp.status(), reqwest::StatusCode::OK);
    let search_body: Value = search_resp.json().await.expect("parse code_search body");
    let results = search_body["result"]["structuredContent"]["results"]
        .as_array()
        .expect("results array");
    assert!(!results.is_empty());

    server_handle.abort();
}

#[test]
fn test_generate_persistent_verification_sqlite_db() {
    // Populate a persistent SQLite index database at `.forge/verify-index.db` for REA MCP inspection
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let db_path = manifest_dir.join(".forge").join("verify-index.db");

    let store = ForgeStore::open(&db_path).expect("open .forge/verify-index.db");
    let embedder = EmbeddingEngine::default();
    let env = store
        .open_workspace(
            &WorkspaceOpenRequest {
                path: manifest_dir.to_string_lossy().into_owned(),
                workspace_id: Some("forge-self-index".to_string()),
                force_reindex: Some(true),
            },
            &embedder,
            &[],
        )
        .expect("index forge repository into .forge/verify-index.db");

    assert_eq!(env.freshness, Freshness::Current);
    assert!(env.results[0].file_count >= 10);
    assert!(env.results[0].symbol_count >= 40);

    // Checkpoint WAL into the main database file so external tools see all rows cleanly
    store
        .with_conn(|conn| {
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            Ok(())
        })
        .expect("checkpoint WAL");

    assert!(Path::new(&db_path).exists());
}
