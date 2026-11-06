use crate::mcp::ForgeMcpServer;
use anyhow::Result;
use axum::{
    extract::{Query, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

#[derive(Clone)]
pub struct HttpAppState {
    pub mcp: Arc<ForgeMcpServer>,
    pub auth_token: Option<String>,
    pub sse_tx: broadcast::Sender<String>,
}

pub fn build_http_router(mcp: Arc<ForgeMcpServer>, auth_token: Option<String>) -> Router {
    let (sse_tx, _) = broadcast::channel(128);
    let state = HttpAppState {
        mcp,
        auth_token: auth_token.filter(|t| !t.trim().is_empty()),
        sse_tx,
    };

    let protected = Router::new()
        .route("/mcp", post(handle_mcp_post))
        .route("/message", post(handle_mcp_post))
        .route("/sse", get(handle_sse_get))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            bearer_auth_middleware,
        ));

    Router::new()
        .route("/health", get(handle_health))
        .merge(protected)
        .with_state(state)
}

pub async fn run_http_server(
    mcp: Arc<ForgeMcpServer>,
    host: &str,
    port: u16,
    auth_token: Option<String>,
) -> Result<()> {
    let app = build_http_router(mcp, auth_token);
    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Forge HTTP MCP server listening on http://{}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle_health() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "service": "forge-mcp",
            "version": env!("CARGO_PKG_VERSION")
        })),
    )
}

async fn bearer_auth_middleware(
    State(state): State<HttpAppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(ref expected) = state.auth_token {
        let header_token = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer ").or_else(|| s.strip_prefix("bearer ")))
            .map(|s| s.trim());

        let query_token = query.get("token").map(|s| s.as_str());
        let provided = header_token.or(query_token);

        match provided {
            Some(tok) if tok == expected => {}
            _ => {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({
                        "jsonrpc": "2.0",
                        "id": Value::Null,
                        "error": {
                            "code": -32001,
                            "message": "Unauthorized: valid Bearer token required"
                        }
                    })),
                )
                    .into_response();
            }
        }
    }

    next.run(request).await
}

async fn handle_mcp_post(
    State(state): State<HttpAppState>,
    Json(payload): Json<Value>,
) -> Response {
    let mcp = state.mcp.clone();
    let sse_tx = state.sse_tx.clone();

    let resp_opt = tokio::task::spawn_blocking(move || mcp.handle_jsonrpc_value(payload))
        .await
        .unwrap_or(None);

    match resp_opt {
        Some(resp_val) => {
            if let Ok(text) = serde_json::to_string(&resp_val) {
                let _ = sse_tx.send(text);
            }
            (StatusCode::OK, Json(resp_val)).into_response()
        }
        None => (StatusCode::ACCEPTED, Json(json!({}))).into_response(),
    }
}

async fn handle_sse_get(
    State(state): State<HttpAppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = state.sse_tx.subscribe();
    let initial = tokio_stream::once(Ok(Event::default().event("endpoint").data("/message")));
    let updates = BroadcastStream::new(rx).filter_map(|msg| match msg {
        Ok(data) => Some(Ok(Event::default().event("message").data(data))),
        Err(_) => None,
    });
    let stream = initial.chain(updates);
    Sse::new(stream).keep_alive(KeepAlive::default())
}
