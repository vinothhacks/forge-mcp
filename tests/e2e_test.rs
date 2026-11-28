use forge_mcp::embeddings::EmbeddingEngine;
use forge_mcp::graph::{graph_query, impact_analyze};
use forge_mcp::http::{build_http_router, run_http_server};
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

/// job_cancel on a terminal job must keep its status and explain why via limitations;
/// unknown job IDs must error without being recorded as cancelled.
#[test]
fn test_job_cancel_terminal_and_unknown_jobs() {
    let ws_dir = create_multilang_workspace();
    let store = ForgeStore::open_in_memory().expect("open in-memory store");
    let embedder = EmbeddingEngine::default();
    let open_env = store
        .open_workspace(
            &WorkspaceOpenRequest {
                path: ws_dir.path().to_string_lossy().into_owned(),
                workspace_id: Some("ws-cancel-terminal".to_string()),
                force_reindex: None,
            },
            &embedder,
            &[],
        )
        .expect("open workspace");
    let job_id = open_env.results[0].job_id.clone();

    let env = store
        .cancel_job(&JobCancelRequest {
            job_id: job_id.clone(),
        })
        .expect("cancel completed job");
    assert_eq!(env.results[0].status, JobStatusState::Completed);
    assert!(
        env.limitations
            .iter()
            .any(|l| l.contains("already completed") && l.contains("nothing to cancel")),
        "expected limitation for completed job, got {:?}",
        env.limitations
    );
    assert!(!store.is_job_cancelled(&job_id));

    let err = store
        .cancel_job(&JobCancelRequest {
            job_id: "job-does-not-exist".to_string(),
        })
        .expect_err("unknown job must error");
    assert!(err.to_string().contains("not found"), "got: {}", err);
    assert!(!store.is_job_cancelled("job-does-not-exist"));

    store
        .insert_job(&JobRecord {
            job_id: "job-running-limit".to_string(),
            workspace_id: "ws-cancel-terminal".to_string(),
            kind: "workspace_index".to_string(),
            status: JobStatusState::Running,
            progress_pct: 10,
            processed_files: 1,
            total_files: 10,
            snapshot_id: None,
            error: None,
            created_at: "2026-10-26T00:00:00Z".to_string(),
            updated_at: "2026-10-26T00:00:00Z".to_string(),
        })
        .expect("insert running job");
    let env = store
        .cancel_job(&JobCancelRequest {
            job_id: "job-running-limit".to_string(),
        })
        .expect("cancel running job");
    assert_eq!(env.results[0].status, JobStatusState::Cancelled);
    assert!(env
        .limitations
        .iter()
        .any(|l| l.contains("Cancellation requested")));
    let env = store
        .cancel_job(&JobCancelRequest {
            job_id: "job-running-limit".to_string(),
        })
        .expect("cancel again");
    assert_eq!(env.results[0].status, JobStatusState::Cancelled);
    assert!(env
        .limitations
        .iter()
        .any(|l| l.contains("already cancelled")));
}

/// Real mid-index cancellation: cancel a running workspace_open from another thread and
/// verify the open fails, the job is recorded as cancelled, and no snapshot is activated.
#[test]
fn test_job_cancel_mid_index_aborts_open() {
    let tmp = tempfile::tempdir().expect("tempdir");
    for i in 0..600 {
        let mut src = String::from("import os\n\n");
        for d in 0..12 {
            src.push_str(&format!(
                "def helper_{i}_{d}(a, b={d}):\n    x = a + b\n    if x > 10:\n        return helper_{i}_{n}(x, b)\n    return os.path.join(str(a), str(b))\n\n",
                n = (d + 1) % 12
            ));
        }
        fs::write(tmp.path().join(format!("mod{}.py", i)), src).expect("write fixture");
    }
    let store = ForgeStore::open_in_memory().expect("open in-memory store");
    let path = tmp.path().to_string_lossy().into_owned();
    let worker_store = store.clone();
    let worker = std::thread::spawn(move || {
        worker_store.open_workspace(
            &WorkspaceOpenRequest {
                path,
                workspace_id: Some("ws-cancel-mid".to_string()),
                force_reindex: None,
            },
            &EmbeddingEngine::default(),
            &[],
        )
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let running_job = loop {
        if let Ok(env) = store.job_status(&JobStatusRequest {
            job_id: None,
            workspace_id: Some("ws-cancel-mid".to_string()),
        }) {
            let job = env.results[0].clone();
            assert_eq!(
                job.status,
                JobStatusState::Running,
                "job finished before it could be cancelled"
            );
            break job;
        }
        assert!(std::time::Instant::now() < deadline, "job never started");
        std::thread::sleep(std::time::Duration::from_millis(1));
    };

    let cancel_env = store
        .cancel_job(&JobCancelRequest {
            job_id: running_job.job_id.clone(),
        })
        .expect("cancel running job");
    assert_eq!(cancel_env.results[0].status, JobStatusState::Cancelled);

    let open_result = worker.join().expect("worker thread");
    let err = open_result.expect_err("cancelled open must fail");
    assert!(err.to_string().contains("cancelled"), "got: {}", err);

    let job = store.get_job(&running_job.job_id).expect("get job");
    assert_eq!(job.status, JobStatusState::Cancelled);
    assert!(job.snapshot_id.is_none());
    assert!(store
        .get_active_snapshot("ws-cancel-mid")
        .expect("active snapshot query")
        .is_none());
}

/// Cancellation that arrives after file parsing (during embedding) must still abort the
/// open, roll back the snapshot, and leave the job cancelled (not overwritten as completed).
#[test]
fn test_job_cancel_after_parse_phase_rolls_back_snapshot() {
    use std::net::TcpListener;
    use std::sync::mpsc;

    let ws_dir = create_multilang_workspace();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind embedding stub");
    let addr = listener.local_addr().expect("stub addr");
    let (first_req_tx, first_req_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let mut first = true;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            if first {
                first = false;
                let _ = first_req_tx.send(());
                let _ = release_rx.recv();
            }
            let _ = stream.write_all(
                b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });

    let store = ForgeStore::open_in_memory().expect("open in-memory store");
    let worker_store = store.clone();
    let path = ws_dir.path().to_string_lossy().into_owned();
    let endpoint = format!("http://{}", addr);
    let worker = std::thread::spawn(move || {
        let embedder = EmbeddingEngine::new(Some(endpoint), Some("stub-model".to_string()));
        worker_store.open_workspace(
            &WorkspaceOpenRequest {
                path,
                workspace_id: Some("ws-cancel-late".to_string()),
                force_reindex: None,
            },
            &embedder,
            &[],
        )
    });

    // The first embedding request proves the indexer has finished the per-file parse loop.
    first_req_rx
        .recv_timeout(std::time::Duration::from_secs(120))
        .expect("indexer reached embedding phase");
    let job = store
        .job_status(&JobStatusRequest {
            job_id: None,
            workspace_id: Some("ws-cancel-late".to_string()),
        })
        .expect("job_status")
        .results[0]
        .clone();
    assert_eq!(job.status, JobStatusState::Running);
    let cancel_env = store
        .cancel_job(&JobCancelRequest {
            job_id: job.job_id.clone(),
        })
        .expect("cancel running job");
    assert_eq!(cancel_env.results[0].status, JobStatusState::Cancelled);
    release_tx.send(()).expect("release stub");

    let err = worker
        .join()
        .expect("worker thread")
        .expect_err("open cancelled after parse phase must fail");
    assert!(err.to_string().contains("cancelled"), "got: {}", err);
    let job = store.get_job(&job.job_id).expect("get job");
    assert_eq!(job.status, JobStatusState::Cancelled);
    assert!(job.snapshot_id.is_none());
    assert!(store
        .get_active_snapshot("ws-cancel-late")
        .expect("active snapshot query")
        .is_none());
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
    reader
        .read_line(&mut line)
        .expect("read initialize response");
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
    reader
        .read_line(&mut blank_line)
        .expect("read CRLF separator");

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

    // 4. Batch [initialize, ping] -> array of 2; supported protocolVersion echoed + instructions
    let batch_req = json!([
        {
            "jsonrpc": "2.0",
            "id": 10,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "rust-e2e", "version": "0.1.0" }
            }
        },
        { "jsonrpc": "2.0", "id": 11, "method": "ping" }
    ]);
    writeln!(stdin, "{}", batch_req).expect("write batch");
    stdin.flush().expect("flush batch");
    line.clear();
    reader.read_line(&mut line).expect("read batch response");
    let batch_resp: Value = serde_json::from_str(&line).expect("parse batch JSON");
    let batch_arr = batch_resp.as_array().expect("batch response is an array");
    assert_eq!(batch_arr.len(), 2);
    assert_eq!(batch_arr[0]["id"], 10);
    assert_eq!(batch_arr[0]["result"]["protocolVersion"], "2025-06-18");
    assert!(batch_arr[0]["result"]["instructions"].is_string());
    assert_eq!(batch_arr[1]["id"], 11);
    assert_eq!(batch_arr[1]["result"], json!({}));

    // 5. Unsupported protocolVersion is negotiated down to the newest supported one
    let old_init = json!({
        "jsonrpc": "2.0",
        "id": 12,
        "method": "initialize",
        "params": { "protocolVersion": "1999-01-01", "capabilities": {} }
    });
    writeln!(stdin, "{}", old_init).expect("write unsupported initialize");
    stdin.flush().expect("flush unsupported initialize");
    line.clear();
    reader
        .read_line(&mut line)
        .expect("read unsupported initialize");
    let old_resp: Value = serde_json::from_str(&line).expect("parse unsupported initialize");
    assert_eq!(old_resp["id"], 12);
    assert_eq!(old_resp["result"]["protocolVersion"], "2025-06-18");

    // 6. Notifications (single and batched, incl. request-type methods) produce no output;
    //    the next line must be the reply to the `id: 0` ping, echoed as 0.
    writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "method": "ping" })).expect("write ping notif");
    writeln!(
        stdin,
        "{}",
        json!([
            { "jsonrpc": "2.0", "method": "notifications/initialized" },
            { "jsonrpc": "2.0", "method": "tools/list" }
        ])
    )
    .expect("write notification batch");
    writeln!(
        stdin,
        "{}",
        json!({ "jsonrpc": "2.0", "id": 0, "method": "ping" })
    )
    .expect("write id 0");
    stdin.flush().expect("flush notifications");
    line.clear();
    reader.read_line(&mut line).expect("read id 0 response");
    let zero_resp: Value = serde_json::from_str(&line).expect("parse id 0 JSON");
    assert_eq!(
        zero_resp["id"], 0,
        "notifications must not be answered: {}",
        line
    );
    assert_eq!(zero_resp["result"], json!({}));

    // 7. Malformed JSON -> -32700 with id null, and the server keeps serving
    writeln!(stdin, "{{\"jsonrpc\":").expect("write malformed JSON");
    stdin.flush().expect("flush malformed JSON");
    line.clear();
    reader.read_line(&mut line).expect("read parse error");
    let parse_err: Value = serde_json::from_str(&line).expect("parse -32700 JSON");
    assert_eq!(parse_err["error"]["code"], -32700);
    assert!(parse_err["id"].is_null());

    // 8. Invalid UTF-8 line -> -32700 instead of terminating the process
    stdin
        .write_all(&[0xff, 0xfe, b'\n'])
        .expect("write invalid UTF-8");
    stdin.flush().expect("flush invalid UTF-8");
    line.clear();
    reader.read_line(&mut line).expect("read UTF-8 parse error");
    let utf8_err: Value = serde_json::from_str(&line).expect("parse UTF-8 error JSON");
    assert_eq!(utf8_err["error"]["code"], -32700);

    // 9. Empty batch and request without method -> -32600 Invalid Request
    writeln!(stdin, "[]").expect("write empty batch");
    writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "id": 13 })).expect("write missing method");
    stdin.flush().expect("flush invalid requests");
    line.clear();
    reader.read_line(&mut line).expect("read empty batch error");
    let empty_err: Value = serde_json::from_str(&line).expect("parse empty batch error");
    assert_eq!(empty_err["error"]["code"], -32600);
    line.clear();
    reader
        .read_line(&mut line)
        .expect("read missing method error");
    let no_method: Value = serde_json::from_str(&line).expect("parse missing method error");
    assert_eq!(no_method["id"], 13);
    assert_eq!(no_method["error"]["code"], -32600);

    // 10. A Content-Length frame cut off by EOF is treated as EOF (clean exit 0)
    write!(stdin, "Content-Length: 100\r\n\r\n{{\"jsonrpc\":\"2.0\"")
        .expect("write truncated frame");
    stdin.flush().expect("flush truncated frame");

    drop(stdin);
    let status = child.wait().expect("wait for child exit");
    assert!(status.success());
}

#[tokio::test]
async fn test_http_off_loopback_requires_nonblank_token() {
    let store = ForgeStore::open_in_memory().expect("open store");
    let mcp = Arc::new(ForgeMcpServer::new(
        store,
        EmbeddingEngine::default(),
        vec![],
    ));

    for token in [None, Some(String::new()), Some(" \t".to_owned())] {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            run_http_server(mcp.clone(), "192.0.2.1", 0, token),
        )
        .await
        .expect("the guard should reject promptly");
        let error = result.expect_err("off-loopback bind should be refused");
        assert!(
            error.to_string().contains("Set FORGE_AUTH_TOKEN"),
            "{error}"
        );
    }
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

    let secret = uuid::Uuid::new_v4().to_string();
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

    let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});
    let mcp_url = format!("{base_url}/mcp");
    let query_auth = client
        .post(&mcp_url)
        .query(&[("token", &secret)])
        .json(&ping)
        .send()
        .await
        .expect("POST /mcp with query authentication");
    assert_eq!(query_auth.status(), reqwest::StatusCode::UNAUTHORIZED);

    for (origin, expected) in [
        (base_url.as_str(), reqwest::StatusCode::OK),
        ("http://localhost:1", reqwest::StatusCode::FORBIDDEN),
        ("https://example.invalid", reqwest::StatusCode::FORBIDDEN),
        ("null", reqwest::StatusCode::FORBIDDEN),
    ] {
        let response = client
            .post(&mcp_url)
            .header("Origin", origin)
            .header("Authorization", format!("BEARER {secret}"))
            .json(&ping)
            .send()
            .await
            .expect("POST /mcp with Origin");
        assert_eq!(response.status(), expected);
    }

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

#[tokio::test]
async fn test_tokenless_http_requires_loopback_host() {
    let store = ForgeStore::open_in_memory().expect("open store");
    let mcp = Arc::new(ForgeMcpServer::new(
        store,
        EmbeddingEngine::default(),
        vec![],
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let url = format!("http://{}/mcp", listener.local_addr().expect("local addr"));
    let app = build_http_router(mcp, None);
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve HTTP");
    });
    let client = reqwest::Client::new();
    let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});

    let local = client
        .post(&url)
        .json(&ping)
        .send()
        .await
        .expect("POST /mcp");
    assert_eq!(local.status(), reqwest::StatusCode::OK);
    let rejected = client
        .post(&url)
        .header("Host", "example.invalid")
        .json(&ping)
        .send()
        .await
        .expect("POST /mcp with non-loopback Host");
    assert_eq!(rejected.status(), reqwest::StatusCode::FORBIDDEN);
    server_handle.abort();
}

#[test]
fn test_workspace_skips_oversized_files_and_rejects_file_roots() {
    let dir = tempfile::tempdir().expect("create workspace");
    let source_path = dir.path().join("small.rs");
    fs::write(&source_path, "fn hello() {}").expect("write source");
    fs::File::create(dir.path().join("large.rs"))
        .expect("create large source")
        .set_len(2 * 1024 * 1024 + 1)
        .expect("set source size");

    let (_, records) = ForgeStore::compute_workspace_state(dir.path()).expect("scan workspace");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].0, "small.rs");

    let store = ForgeStore::open_in_memory().expect("open store");
    let request = WorkspaceOpenRequest {
        path: source_path.to_string_lossy().into_owned(),
        workspace_id: None,
        force_reindex: None,
    };
    let error = store
        .open_workspace(&request, &EmbeddingEngine::default(), &[])
        .expect_err("file workspace root should fail");
    assert!(error.to_string().contains("not a directory"), "{error}");
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

/// Regression: `.gitignore` must be honored even when the workspace is NOT a git repository.
/// `ignore::WalkBuilder` defaults to `require_git(true)`, which silently disabled gitignore
/// handling for plain directories (no `.git/`), indexing files the user explicitly excluded.
#[test]
fn test_workspace_open_respects_gitignore_outside_git_repo() {
    let ws_dir = create_multilang_workspace();
    let root = ws_dir.path();
    assert!(
        !root.join(".git").exists(),
        "fixture must not be a git repo"
    );

    fs::write(root.join(".gitignore"), "ignored/\n*.generated.ts\n").expect("write .gitignore");
    fs::create_dir_all(root.join("ignored")).expect("create ignored dir");
    fs::write(
        root.join("ignored").join("skip.rs"),
        "pub fn skipped_fn() -> u32 { 1 }\n",
    )
    .expect("write ignored/skip.rs");
    fs::write(
        root.join("src").join("x.generated.ts"),
        "export function generatedFn(): number { return 1; }\n",
    )
    .expect("write src/x.generated.ts");

    // Direct walker check: gitignored paths must not be discovered.
    let discovered: Vec<String> = ForgeStore::collect_workspace_files(root)
        .expect("collect files")
        .into_iter()
        .map(|(rel, _, _)| rel)
        .collect();
    assert!(
        !discovered.iter().any(|p| p == "ignored/skip.rs"),
        "ignored/skip.rs must be excluded by .gitignore, got {:?}",
        discovered
    );
    assert!(
        !discovered.iter().any(|p| p == "src/x.generated.ts"),
        "src/x.generated.ts must be excluded by .gitignore, got {:?}",
        discovered
    );
    assert!(discovered.iter().any(|p| p == "src/auth.rs"));

    // End-to-end: workspace_open must index exactly the 5 fixture files.
    let store = ForgeStore::open_in_memory().expect("open in-memory store");
    let embedder = EmbeddingEngine::default();
    let open_env = store
        .open_workspace(
            &WorkspaceOpenRequest {
                path: root.to_string_lossy().into_owned(),
                workspace_id: Some("ws-gitignore".to_string()),
                force_reindex: Some(false),
            },
            &embedder,
            &[],
        )
        .expect("open_workspace should succeed");
    assert_eq!(open_env.results.len(), 1);
    assert_eq!(
        open_env.results[0].file_count, 5,
        "gitignored files must not be indexed outside a git repo"
    );
}

#[test]
fn test_symbol_find_rejects_or_aliases_unknown_kind() {
    let ws_dir = create_multilang_workspace();
    let store = ForgeStore::open_in_memory().expect("open in-memory store");
    let embedder = EmbeddingEngine::default();
    store
        .open_workspace(
            &WorkspaceOpenRequest {
                path: ws_dir.path().to_string_lossy().into_owned(),
                workspace_id: Some("ws-kind".to_string()),
                force_reindex: None,
            },
            &embedder,
            &[],
        )
        .expect("open workspace");

    let find = |kind: &str| {
        store.find_symbols(&SymbolFindRequest {
            workspace_id: "ws-kind".to_string(),
            name: String::new(),
            match_mode: Some(MatchMode::Substring),
            kind: Some(kind.to_string()),
            file_path: None,
            language: None,
            limit: Some(500),
        })
    };

    // Unknown kinds must error, never silently fall back to functions.
    for bad in ["bogus", "variable", "Enumeration"] {
        let err = find(bad).expect_err("unknown kind must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("unknown symbol kind"), "msg={msg}");
        assert!(msg.contains(bad), "msg={msg}");
    }

    // Explicit empty kind behaves like no filter.
    let all = find("  ").expect("empty kind is no filter");
    assert_eq!(all.results.len(), 21, "fixture has 21 symbols");

    // `enum` aliases to struct (parser indexes enum_item as Struct); case-insensitive.
    let enums = find("ENUM").expect("enum alias");
    let structs = find("struct").expect("struct kind");
    assert_eq!(structs.results.len(), 2);
    assert!(structs
        .results
        .iter()
        .all(|h| h.symbol.kind == SymbolKind::Struct));
    let names = |env: &ResponseEnvelope<SymbolFindHit>| {
        env.results
            .iter()
            .map(|h| h.symbol.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&enums), names(&structs));

    // Every canonical kind filters to exactly that kind, with fixture counts.
    for (kind, want, count) in [
        ("function", SymbolKind::Function, 6),
        ("method", SymbolKind::Method, 6),
        ("struct", SymbolKind::Struct, 2),
        ("class", SymbolKind::Class, 3),
        ("trait", SymbolKind::Trait, 1),
        ("interface", SymbolKind::Interface, 2),
        ("module", SymbolKind::Module, 1),
    ] {
        let env = find(kind).expect(kind);
        assert_eq!(env.results.len(), count, "kind={kind}");
        assert!(
            env.results.iter().all(|h| h.symbol.kind == want),
            "kind={kind}"
        );
    }
}

/// Regression (F6 symbol_read): an ambiguous `name` must prefer a definition
/// over a one-line trait declaration, honor `file_path` disambiguation,
/// surface the ambiguity in `limitations`, and give clear errors when the
/// lookup key is missing or unknown.
#[test]
fn test_symbol_read_ambiguous_name_disambiguation_and_errors() {
    let ws_dir = create_multilang_workspace();
    // Make `verify` exist in Rust (trait decl + impl method) AND Python.
    let py = ws_dir.path().join("src").join("pipeline.py");
    let mut py_src = fs::read_to_string(&py).expect("read pipeline.py");
    py_src.push_str("\ndef verify(token):\n    return True\n");
    fs::write(&py, py_src).expect("append python verify");

    let store = ForgeStore::open_in_memory().expect("open in-memory store");
    let embedder = EmbeddingEngine::default();
    store
        .open_workspace(
            &WorkspaceOpenRequest {
                path: ws_dir.path().to_string_lossy().into_owned(),
                workspace_id: Some("ws-f6".to_string()),
                force_reindex: None,
            },
            &embedder,
            &[],
        )
        .expect("open workspace");

    let read = |symbol_id: Option<&str>, name: Option<&str>, file_path: Option<&str>| {
        store.read_symbol(&SymbolReadRequest {
            workspace_id: "ws-f6".to_string(),
            symbol_id: symbol_id.map(str::to_string),
            name: name.map(str::to_string),
            file_path: file_path.map(str::to_string),
            context_lines: None,
        })
    };

    // Rust file: impl method (definition, lines 13-15) wins over trait decl (line 3);
    // parent is the impl target struct.
    let rs = read(None, Some("verify"), Some("src/auth.rs")).expect("read rust verify");
    let r = &rs.results[0];
    assert_eq!(r.symbol.file_path, "src/auth.rs");
    assert_eq!(r.symbol.qualified_name, "TokenValidator::verify");
    assert_eq!(
        r.parent_symbol.as_ref().map(|p| p.name.as_str()),
        Some("TokenValidator")
    );
    assert!(r
        .body
        .contains("verify_hmac_token(token, &self.secret_salt)"));
    // Still ambiguous inside the file (trait decl also matches) -> limitation present.
    assert!(
        rs.limitations
            .iter()
            .any(|l| l.contains("Ambiguous symbol name 'verify'")),
        "limitations: {:?}",
        rs.limitations
    );

    // Python file via file_path (backslash form must also work).
    for fp in ["src/pipeline.py", r"src\pipeline.py"] {
        let py = read(None, Some("verify"), Some(fp)).expect("read python verify");
        assert_eq!(py.results[0].symbol.language, "python");
        assert_eq!(py.results[0].symbol.file_path, "src/pipeline.py");
        assert!(
            py.limitations.is_empty(),
            "unique in file: {:?}",
            py.limitations
        );
    }

    // No file_path: 3 candidates; limitation names them all.
    let any = read(None, Some("verify"), None).expect("read ambiguous verify");
    let amb = any
        .limitations
        .iter()
        .find(|l| l.contains("Ambiguous symbol name 'verify'"))
        .expect("ambiguity limitation");
    assert!(amb.contains("3 candidates"), "{amb}");
    assert!(amb.contains("Verifier::verify"), "{amb}");
    assert!(amb.contains("src/pipeline.py"), "{amb}");
    assert!(amb.contains("symbol_id"), "{amb}");
    // Qualified name is an exact disambiguator.
    let q = read(None, Some("TokenValidator::verify"), None).expect("qualified read");
    assert_eq!(q.results[0].symbol.qualified_name, "TokenValidator::verify");
    assert!(q.limitations.is_empty());

    // Error paths.
    let e = read(Some("sym-does-not-exist"), None, None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("not found"), "{e}");
    assert!(e.contains("sym-does-not-exist"), "{e}");
    let e = read(None, None, None).unwrap_err().to_string();
    assert!(e.contains("requires 'symbol_id' or 'name'"), "{e}");
    let e = read(None, Some("no_such_symbol_xyz"), None)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("not found") && e.contains("no_such_symbol_xyz"),
        "{e}"
    );

    // context_lines: 0 == body; huge value clamps to the file without error.
    let c0 = read(None, Some("compute_hmac_digest"), None).expect("ctx default");
    let sid = c0.results[0].symbol.symbol_id.clone();
    let z = store
        .read_symbol(&SymbolReadRequest {
            workspace_id: "ws-f6".to_string(),
            symbol_id: Some(sid.clone()),
            name: None,
            file_path: None,
            context_lines: Some(0),
        })
        .expect("ctx 0");
    assert_eq!(
        z.results[0].enclosing_scope.as_deref(),
        Some(z.results[0].body.as_str())
    );
    let big = store
        .read_symbol(&SymbolReadRequest {
            workspace_id: "ws-f6".to_string(),
            symbol_id: Some(sid),
            name: None,
            file_path: None,
            context_lines: Some(999),
        })
        .expect("ctx 999");
    let whole = fs::read_to_string(ws_dir.path().join("src").join("auth.rs")).unwrap();
    assert_eq!(
        big.results[0].enclosing_scope.as_deref(),
        Some(whole.lines().collect::<Vec<_>>().join("\n").as_str())
    );
}

/// F7 regression: `include_declarations` must change the output (H3) and
/// `file_path` must restrict hits to that file.
#[test]
fn test_symbol_references_include_declarations() {
    let ws_dir = create_multilang_workspace();
    let store = ForgeStore::open_in_memory().expect("open in-memory store");
    let embedder = EmbeddingEngine::default();
    store
        .open_workspace(
            &WorkspaceOpenRequest {
                path: ws_dir.path().to_string_lossy().into_owned(),
                workspace_id: Some("ws-f7".to_string()),
                force_reindex: None,
            },
            &embedder,
            &[],
        )
        .expect("open workspace");

    let req = |include: Option<bool>, file_path: Option<&str>| SymbolReferencesRequest {
        workspace_id: "ws-f7".to_string(),
        symbol_id: None,
        symbol_name: Some("verify_hmac_token".to_string()),
        name: None,
        symbol: None,
        file_path: file_path.map(|s| s.to_string()),
        include_declarations: include,
        limit: Some(20),
    };

    let without = store
        .symbol_references(&req(Some(false), None))
        .expect("refs false");
    let with = store
        .symbol_references(&req(Some(true), None))
        .expect("refs true");
    let default = store
        .symbol_references(&req(None, None))
        .expect("refs default");

    assert_eq!(
        without.results.len(),
        default.results.len(),
        "default == false"
    );
    assert!(
        without
            .results
            .iter()
            .all(|h| h.reference.reference_kind != "declaration"),
        "include_declarations=false must not emit declaration hits"
    );
    assert_eq!(
        with.results.len(),
        without.results.len() + 1,
        "include_declarations=true adds exactly one declaration hit"
    );
    let decl = &with.results[0].reference;
    assert_eq!(decl.reference_kind, "declaration");
    assert_eq!(decl.file_path, "src/auth.rs");
    assert_eq!(decl.symbol_name, "verify_hmac_token");
    assert!(decl.target_symbol_id.is_some());
    assert!(with.results[0]
        .surrounding_context
        .contains("pub fn verify_hmac_token"));
    assert_eq!(with.evidence.len(), with.results.len());
    assert_eq!(with.evidence[0].producer, "forge-tree-sitter-symbol-reader");

    // file_path restricts hits to that file.
    let only_tests = store
        .symbol_references(&req(Some(true), Some("tests/auth_test.rs")))
        .expect("refs filtered");
    assert!(!only_tests.results.is_empty());
    assert!(only_tests
        .results
        .iter()
        .all(|h| h.reference.file_path == "tests/auth_test.rs"));
    assert!(
        only_tests
            .results
            .iter()
            .all(|h| h.reference.reference_kind != "declaration"),
        "declaration lives in src/auth.rs so must be filtered out"
    );
}

/// Regression (F8): graph_query must reject unknown query kinds, advertise every
/// alias the request struct accepts, match importers by whole path segment, and
/// flag hops whose target is outside the snapshot as unresolved.
#[test]
fn test_graph_query_strict_kind_aliases_and_importers() {
    let ws_dir = create_multilang_workspace();
    let store = ForgeStore::open_in_memory().expect("open in-memory store");
    let embedder = EmbeddingEngine::default();
    store
        .open_workspace(
            &WorkspaceOpenRequest {
                path: ws_dir.path().to_string_lossy().into_owned(),
                workspace_id: Some("ws-f8".to_string()),
                force_reindex: None,
            },
            &embedder,
            &[],
        )
        .expect("open workspace");

    let req = |v: Value| -> GraphQueryRequest {
        let mut base = json!({ "workspace_id": "ws-f8" });
        for (k, val) in v.as_object().unwrap() {
            base[k] = val.clone();
        }
        serde_json::from_value(base).expect("deserialize GraphQueryRequest")
    };

    // Unknown query_kind is an error, not a silent `callers` fallback.
    let err = graph_query(
        &store,
        &req(json!({ "query_kind": "foo", "symbol_name": "compute_hmac_digest" })),
    )
    .expect_err("unknown query_kind must fail");
    assert!(err.to_string().contains("unknown query_kind"), "{err}");

    // Every alias accepted by GraphQueryRequest yields the same callers result.
    for args in [
        json!({ "query_kind": "callers", "symbol_name": "compute_hmac_digest", "max_depth": 1 }),
        json!({ "direction": "callers", "symbol_name": "compute_hmac_digest", "max_depth": 1 }),
        json!({ "kind": "callers", "symbol_name": "compute_hmac_digest", "max_depth": 1 }),
        json!({ "relation": "callers", "symbol_name": "compute_hmac_digest", "max_depth": 1 }),
        json!({ "query_kind": "callers", "name": "compute_hmac_digest", "max_depth": 1 }),
        json!({ "query_kind": "callers", "symbol": "compute_hmac_digest", "max_depth": 1 }),
    ] {
        let env = graph_query(&store, &req(args.clone())).expect("alias query");
        let froms: Vec<&str> = env.results[0]
            .hops
            .iter()
            .map(|h| h.relationship.from_name.as_str())
            .collect();
        assert_eq!(froms, vec!["verify_hmac_token"], "args={args}");
    }

    // The advertised schema lists every alias the struct accepts.
    let defs = ForgeMcpServer::tool_definitions();
    let gq = defs
        .iter()
        .find(|t| t["name"] == "graph_query")
        .expect("graph_query tool");
    let props = gq["inputSchema"]["properties"].as_object().unwrap();
    for alias in [
        "query_kind",
        "direction",
        "kind",
        "relation",
        "symbol_name",
        "name",
        "symbol",
    ] {
        assert!(props.contains_key(alias), "schema missing {alias}");
    }

    // Importers of src/auth.rs: tests/auth_test.rs (crate::auth::...) only, not ./auth_client.
    let env = graph_query(
        &store,
        &req(json!({ "query_kind": "importers", "file_path": "src/auth.rs", "max_depth": 1 })),
    )
    .expect("importers");
    let importers: Vec<&str> = env.results[0]
        .hops
        .iter()
        .map(|h| h.relationship.from_file_path.as_str())
        .collect();
    assert_eq!(importers, vec!["tests/auth_test.rs"]);
    assert!(
        !importers.contains(&"src/gateway.ts"),
        "\"./auth_client\" must not be treated as an import of src/auth.rs"
    );

    // Exact stem match still resolves relative specifiers: `./auth_client` -> auth_client.ts.
    let env = graph_query(
        &store,
        &req(
            json!({ "query_kind": "importers", "file_path": "src/auth_client.ts", "max_depth": 1 }),
        ),
    )
    .expect("importers of auth_client");
    let importers: Vec<&str> = env.results[0]
        .hops
        .iter()
        .map(|h| h.relationship.from_file_path.as_str())
        .collect();
    assert_eq!(importers, vec!["src/gateway.ts"]);

    // Imports of src/gateway.ts reach an external module -> limitation mentions unresolved.
    let env = graph_query(
        &store,
        &req(json!({ "query_kind": "imports", "file_path": "src/gateway.ts", "max_depth": 1 })),
    )
    .expect("imports");
    assert!(env.results[0]
        .hops
        .iter()
        .any(|h| h.relationship.to_name == "./auth_client"));
    assert!(
        env.limitations.iter().any(|l| l.contains("unresolved")),
        "{:?}",
        env.limitations
    );
}

#[test]
fn test_context_build_snapshot_policy_values() {
    let ws_dir = create_multilang_workspace();
    let store = ForgeStore::open_in_memory().expect("open in-memory store");
    let embedder = EmbeddingEngine::default();

    store
        .open_workspace(
            &WorkspaceOpenRequest {
                path: ws_dir.path().to_string_lossy().into_owned(),
                workspace_id: Some("ws-policy".to_string()),
                force_reindex: Some(false),
            },
            &embedder,
            &[],
        )
        .expect("open_workspace should succeed");

    let req = |policy: Option<&str>| ContextBuildRequest {
        workspace_id: "ws-policy".to_string(),
        query: "how are bearer tokens sanitized and HMAC digests verified?".to_string(),
        token_budget: Some(4000),
        include_tests: Some(true),
        snapshot_policy: policy.map(|p| p.to_string()),
        language: None,
        path_prefix: None,
    };

    // Fresh snapshot: every valid policy succeeds; an unknown value is rejected even when fresh.
    for policy in [
        None,
        Some("require_current"),
        Some("require_current_strict"),
        Some("allow_stale"),
    ] {
        let env = context_build(&store, &embedder, &req(policy)).unwrap_or_else(|e| {
            panic!("fresh snapshot, policy {:?} should succeed: {}", policy, e)
        });
        assert_eq!(env.freshness, Freshness::Current);
        assert!(!env.limitations.iter().any(|l| l.contains("stale")));
    }
    let bogus_fresh = context_build(&store, &embedder, &req(Some("bogus_policy")))
        .expect_err("unknown snapshot_policy must be rejected");
    assert!(bogus_fresh.to_string().contains("unknown snapshot_policy"));

    // Make the snapshot stale by editing an indexed file on disk.
    let auth_path = ws_dir.path().join("src").join("auth.rs");
    let mut auth_src = fs::read_to_string(&auth_path).expect("read auth.rs");
    auth_src.push_str("\n/// Added after indexing to make the snapshot stale.\npub fn stale_marker() -> u8 { 1 }\n");
    fs::write(&auth_path, auth_src).expect("rewrite auth.rs");

    // require_current (and its strict alias) must fail on a stale snapshot.
    for policy in ["require_current", "require_current_strict"] {
        let err = context_build(&store, &embedder, &req(Some(policy)))
            .expect_err("stale snapshot with require_current must error");
        let msg = err.to_string();
        assert!(
            msg.contains("stale"),
            "error should mention staleness: {}",
            msg
        );
        assert!(
            msg.contains(policy),
            "error should name the policy: {}",
            msg
        );
    }

    // allow_stale and omitted policy proceed and surface a staleness limitation.
    for policy in [Some("allow_stale"), None] {
        let env = context_build(&store, &embedder, &req(policy)).unwrap_or_else(|e| {
            panic!("stale snapshot, policy {:?} should succeed: {}", policy, e)
        });
        assert_eq!(env.freshness, Freshness::Stale);
        assert!(!env.results[0].snippets.is_empty());
        assert!(
            env.limitations
                .iter()
                .any(|l| l.contains("stale") && l.contains("src/auth.rs")),
            "limitations should report staleness: {:?}",
            env.limitations
        );
    }

    // Unknown values are still rejected when stale, before any retrieval happens.
    let bogus_stale = context_build(&store, &embedder, &req(Some("bogus_policy")))
        .expect_err("unknown snapshot_policy must be rejected");
    assert!(bogus_stale.to_string().contains("unknown snapshot_policy"));
}

#[test]
fn test_context_build_graph_expansion_respects_filters() {
    let ws_dir = create_multilang_workspace();
    let store = ForgeStore::open_in_memory().expect("open in-memory store");
    let embedder = EmbeddingEngine::default();

    store
        .open_workspace(
            &WorkspaceOpenRequest {
                path: ws_dir.path().to_string_lossy().into_owned(),
                workspace_id: Some("ws-filters".to_string()),
                force_reindex: Some(false),
            },
            &embedder,
            &[],
        )
        .expect("open_workspace should succeed");

    let req = |language: Option<&str>, path_prefix: Option<&str>| ContextBuildRequest {
        workspace_id: "ws-filters".to_string(),
        query: "verify_hmac_token".to_string(),
        token_budget: Some(8000),
        include_tests: Some(true),
        snapshot_policy: None,
        language: language.map(|l| l.to_string()),
        path_prefix: path_prefix.map(|p| p.to_string()),
    };

    // path_prefix must also constrain graph-expanded neighbors, not only primary hits.
    // With path_prefix=src/, the tests/ caller of verify_hmac_token is not a primary hit, so before
    // the fix 1-hop expansion re-introduced it. Expansion must still run (src/ neighbors exist).
    let by_prefix =
        context_build(&store, &embedder, &req(None, Some("src/"))).expect("path_prefix");
    assert!(!by_prefix.results[0].snippets.is_empty());
    assert!(
        by_prefix.results[0].graph_expanded_items >= 1,
        "graph expansion should still add src/ neighbors"
    );
    for s in &by_prefix.results[0].snippets {
        assert!(
            s.file_path.starts_with("src/"),
            "path_prefix=src/ leaked {} ({})",
            s.file_path,
            s.inclusion_reason
        );
    }
    assert_eq!(
        by_prefix.evidence.len(),
        by_prefix.results[0].snippets.len()
    );

    // language must also constrain graph-expanded neighbors.
    let by_lang = context_build(&store, &embedder, &req(Some("rust"), None)).expect("language");
    assert!(!by_lang.results[0].snippets.is_empty());
    for s in &by_lang.results[0].snippets {
        assert_eq!(
            s.language, "rust",
            "language=rust leaked {} ({})",
            s.file_path, s.inclusion_reason
        );
    }
}

/// F15 regression: Tree-sitter coverage for every supported extension, TSX/JSX
/// reported under their family language filters, and Go receiver parents.
#[test]
fn test_language_coverage_matrix() {
    use std::collections::BTreeSet;

    let tmp = tempfile::tempdir().expect("create temp workspace");
    let root = tmp.path();
    let files: &[(&str, &str)] = &[
        (
            "rust.rs",
            "use std::collections::HashMap;\n\nfn private_fn() {}\n\n/// doc\npub fn public_fn() {}\n\npub struct S;\n\npub enum Color { Red }\n\npub trait T { fn t(&self); }\n\nmod inner { pub fn nested() {} }\n\nimpl T for S { fn t(&self) { public_fn(); } }\n\nimpl S { pub fn assoc() {} }\n",
        ),
        (
            "ts.ts",
            "import { x } from \"./x\";\n\n/** doc */\nexport function f() {}\n\nconst g = () => {};\n\nexport const h = async () => {};\n\ninterface J {}\n\ninterface I extends J {}\n\nclass B {}\n\nexport class C extends B implements I { m() { f(); } }\n\nnamespace N { export function n() {} }\n\nnew C();\n",
        ),
        (
            "tsx.tsx",
            "function f() {}\n\nexport function App() { return <div onClick={() => f()} />; }\n",
        ),
        ("js.js", "function a() {}\nclass K { m() {} }\nconst arr = () => {};\n"),
        (
            "js.mjs",
            "import fs from \"fs\";\nexport function ma() {}\nclass MK { mm() {} }\nconst marr = () => {};\n",
        ),
        (
            "js.cjs",
            "const r = require(\"fs\");\nfunction ca() {}\nclass CK { cm() {} }\nconst carr = () => {};\nmodule.exports = { ca };\n",
        ),
        (
            "jsx.jsx",
            "function Comp() { return <div className=\"x\" />; }\nclass JK { render() { return <span />; } }\nconst jarr = () => <p />;\n",
        ),
        (
            "py.py",
            "import os\nfrom a import b\n\ndef top():\n    pass\n\ndef _private():\n    pass\n\nclass Base:\n    pass\n\nclass Child(Base):\n    \"\"\"Child doc.\"\"\"\n    def method(self):\n        \"\"\"method doc\"\"\"\n        top()\n",
        ),
        ("pyi.pyi", "def stub(x: int) -> int: ...\nclass StubClass: ...\n"),
        (
            "go.go",
            "package pkg\n\nimport (\n\t\"fmt\"\n\t\"os\"\n)\n\ntype Iface interface {\n\tM()\n}\n\ntype St struct{}\n\n// F does X\nfunc F() {}\n\nfunc (s *St) M() { F() }\n\ntype Box[T any] struct{ v T }\n\nfunc (b *Box[T]) Get() T { return b.v }\n",
        ),
        ("broken.rs", "fn ok() {}  fn broken( {\n"),
        ("notes.txt", "plain text, not code\n"),
    ];
    for (name, body) in files {
        fs::write(root.join(name), body).expect("write fixture file");
    }

    let store = ForgeStore::open_in_memory().expect("open in-memory store");
    let embedder = EmbeddingEngine::default();
    let open_env = store
        .open_workspace(
            &WorkspaceOpenRequest {
                path: root.to_string_lossy().into_owned(),
                workspace_id: Some("ws-lang-matrix".to_string()),
                force_reindex: Some(true),
            },
            &embedder,
            &[],
        )
        .expect("open_workspace should index every grammar, including broken.rs");
    // 11 source files; notes.txt is skipped (unknown extension).
    assert_eq!(open_env.results[0].file_count, 11);

    let find = |lang: &str| -> Vec<Symbol> {
        store
            .find_symbols(&SymbolFindRequest {
                workspace_id: "ws-lang-matrix".to_string(),
                name: String::new(),
                match_mode: Some(MatchMode::Substring),
                kind: None,
                file_path: None,
                language: Some(lang.to_string()),
                limit: Some(500),
            })
            .expect("find_symbols by language")
            .results
            .into_iter()
            .map(|h| h.symbol)
            .collect()
    };
    let by_file = |syms: &[Symbol], file: &str| -> BTreeSet<(String, String)> {
        syms.iter()
            .filter(|s| s.file_path == file)
            .map(|s| (s.name.clone(), s.kind.as_str().to_string()))
            .collect()
    };
    let set = |pairs: &[(&str, &str)]| -> BTreeSet<(String, String)> {
        pairs
            .iter()
            .map(|(n, k)| (n.to_string(), k.to_string()))
            .collect()
    };

    let rust = find("rust");
    assert_eq!(
        by_file(&rust, "rust.rs"),
        set(&[
            ("private_fn", "function"),
            ("public_fn", "function"),
            ("S", "struct"),
            ("Color", "struct"),
            ("T", "trait"),
            ("t", "method"),
            ("inner", "module"),
            ("nested", "method"),
            ("assoc", "method"),
        ])
    );
    assert!(by_file(&rust, "broken.rs").contains(&("ok".to_string(), "function".to_string())));
    let public_fn = rust
        .iter()
        .find(|s| s.name == "public_fn")
        .expect("public_fn");
    assert_eq!(public_fn.doc_comment.as_deref(), Some("doc"));
    assert_eq!(public_fn.visibility.as_deref(), Some("pub"));

    // H6: the "typescript" filter must include .tsx symbols (TSX grammar still used).
    let ts = find("typescript");
    assert_eq!(
        by_file(&ts, "ts.ts"),
        set(&[
            ("f", "function"),
            ("g", "function"),
            ("h", "function"),
            ("J", "interface"),
            ("I", "interface"),
            ("B", "class"),
            ("C", "class"),
            ("m", "method"),
            ("N", "module"),
            ("n", "function"),
        ])
    );
    assert_eq!(
        by_file(&ts, "tsx.tsx"),
        set(&[("f", "function"), ("App", "function")])
    );
    assert!(ts.iter().all(|s| s.language == "typescript"));
    assert!(
        find("tsx").is_empty(),
        "tsx is no longer a separate language value"
    );

    // H6: the "javascript" filter must include .js/.mjs/.cjs/.jsx symbols.
    let js = find("javascript");
    assert_eq!(
        by_file(&js, "js.js"),
        set(&[
            ("a", "function"),
            ("K", "class"),
            ("m", "method"),
            ("arr", "function")
        ])
    );
    assert_eq!(
        by_file(&js, "js.mjs"),
        set(&[
            ("ma", "function"),
            ("MK", "class"),
            ("mm", "method"),
            ("marr", "function")
        ])
    );
    assert_eq!(
        by_file(&js, "js.cjs"),
        set(&[
            ("ca", "function"),
            ("CK", "class"),
            ("cm", "method"),
            ("carr", "function")
        ])
    );
    assert_eq!(
        by_file(&js, "jsx.jsx"),
        set(&[
            ("Comp", "function"),
            ("JK", "class"),
            ("render", "method"),
            ("jarr", "function")
        ])
    );
    assert!(
        find("jsx").is_empty(),
        "jsx is no longer a separate language value"
    );

    let py = find("python");
    assert_eq!(
        by_file(&py, "py.py"),
        set(&[
            ("top", "function"),
            ("_private", "function"),
            ("Base", "class"),
            ("Child", "class"),
            ("method", "method"),
        ])
    );
    assert_eq!(
        by_file(&py, "pyi.pyi"),
        set(&[("stub", "function"), ("StubClass", "class")])
    );
    let private = py.iter().find(|s| s.name == "_private").expect("_private");
    assert_eq!(private.visibility.as_deref(), Some("private"));
    let child = py.iter().find(|s| s.name == "Child").expect("Child");
    assert_eq!(child.doc_comment.as_deref(), Some("Child doc."));

    // Go: method receiver `(s *St)` resolves to parent St, not the parameter name `s`.
    let go = find("go");
    assert_eq!(
        by_file(&go, "go.go"),
        set(&[
            ("pkg", "module"),
            ("Iface", "interface"),
            ("St", "struct"),
            ("F", "function"),
            ("M", "method"),
            ("Box", "struct"),
            ("Get", "method"),
        ])
    );
    let st = go.iter().find(|s| s.name == "St").expect("St");
    let m = go.iter().find(|s| s.name == "M").expect("M");
    assert_eq!(m.qualified_name, "St.M");
    assert_eq!(m.parent_symbol_id.as_deref(), Some(st.symbol_id.as_str()));
    // Generic pointer receiver `(b *Box[T])` resolves to Box (same bug class as
    // `(r *HttpRouter)` being indexed as `r.Dispatch`).
    let boxed = go.iter().find(|s| s.name == "Box").expect("Box");
    let get = go.iter().find(|s| s.name == "Get").expect("Get");
    assert_eq!(get.qualified_name, "Box.Get");
    assert_eq!(
        get.parent_symbol_id.as_deref(),
        Some(boxed.symbol_id.as_str())
    );
    let f = go.iter().find(|s| s.name == "F").expect("F");
    assert_eq!(f.doc_comment.as_deref(), Some("F does X"));
}
