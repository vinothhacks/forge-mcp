use crate::mcp::ForgeMcpServer;
use anyhow::Result;
use axum::{
    extract::{Request, State},
    http::{header, uri::Authority, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
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
        .route_layer(middleware::from_fn_with_state(state.clone(), protect_mcp));

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
    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    if !addr.ip().is_loopback()
        && !auth_token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty())
    {
        anyhow::bail!("Set FORGE_AUTH_TOKEN before binding the HTTP transport beyond loopback");
    }
    let app = build_http_router(mcp, auth_token);
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

fn is_loopback_authority(authority: &Authority) -> bool {
    let host = authority.host();
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

async fn protect_mcp(State(state): State<HttpAppState>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let authority = request.uri().authority().cloned().or_else(|| {
        headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok()?.parse::<Authority>().ok())
    });

    // Restrict browser requests to the local server's origin. Also check Host
    // without authentication so DNS rebinding cannot reach the legacy SSE route.
    let loopback_host = authority.as_ref().is_some_and(is_loopback_authority);
    if state.auth_token.is_none() && !loopback_host {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut origins = headers.get_all(header::ORIGIN).iter();
    if let Some(origin) = origins.next() {
        let same_origin = loopback_host
            && origin.to_str().ok().is_some_and(|value| {
                value
                    .strip_prefix("http://")
                    .or_else(|| value.strip_prefix("https://"))
                    .zip(authority.as_ref())
                    .is_some_and(|(origin, host)| origin.eq_ignore_ascii_case(host.as_str()))
            });
        if !same_origin || origins.next().is_some() {
            return StatusCode::FORBIDDEN.into_response();
        }
    }

    if let Some(ref expected) = state.auth_token {
        let header_token = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|value| value.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
            .map(|(_, token)| token.trim());

        match header_token {
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
    let HttpAppState { mcp, sse_tx, .. } = state;
    let resp_opt = tokio::task::spawn_blocking(move || mcp.handle_jsonrpc_value(payload))
        .await
        .unwrap_or(None);

    match resp_opt {
        Some(resp_val) => {
            if sse_tx.receiver_count() > 0 {
                let _ = sse_tx.send(resp_val.to_string());
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
