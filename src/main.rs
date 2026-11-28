use anyhow::Result;
use clap::Parser;
use forge_mcp::embeddings::EmbeddingEngine;
use forge_mcp::http::run_http_server;
use forge_mcp::mcp::ForgeMcpServer;
use forge_mcp::store::ForgeStore;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "forge-mcp",
    version,
    about = "Forge — Self-Hosted Rust Codebase Search and Intelligence MCP Server"
)]
pub struct Cli {
    /// Transport mode: "stdio" or "http"
    #[arg(long, default_value = "stdio", env = "FORGE_TRANSPORT")]
    pub transport: String,

    /// Host interface to bind when running in HTTP mode
    #[arg(long, default_value = "127.0.0.1", env = "FORGE_HOST")]
    pub host: String,

    /// Port to bind when running in HTTP mode
    #[arg(long, default_value_t = 8080, env = "FORGE_PORT")]
    pub port: u16,

    /// Optional Bearer authentication token for HTTP mode
    #[arg(long, env = "FORGE_AUTH_TOKEN", hide_env_values = true)]
    pub auth_token: Option<String>,

    /// Allowed workspace root directory (can be specified multiple times)
    #[arg(long = "allow-root", env = "FORGE_ALLOW_ROOT")]
    pub allow_roots: Vec<PathBuf>,

    /// Path to SQLite index database file
    #[arg(
        long = "db-path",
        visible_alias = "database",
        default_value = ".forge/forge-index.db",
        env = "FORGE_DB_PATH"
    )]
    pub db_path: PathBuf,

    /// Optional OpenAI/Ollama-compatible embeddings endpoint URL (/v1/embeddings)
    #[arg(long, env = "FORGE_EMBEDDING_URL")]
    pub embedding_url: Option<String>,

    /// Optional embedding model identifier
    #[arg(long, env = "FORGE_EMBEDDING_MODEL")]
    pub embedding_model: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Emit all logs to stderr so stdout remains strictly reserved for stdio JSON-RPC frames
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let store = ForgeStore::open(&cli.db_path)?;
    let embedder = EmbeddingEngine::new(cli.embedding_url, cli.embedding_model);
    let server = Arc::new(ForgeMcpServer::new(store, embedder, cli.allow_roots));

    match cli.transport.trim().to_ascii_lowercase().as_str() {
        "stdio" => {
            server.run_stdio()?;
        }
        "http" | "sse" => {
            run_http_server(server, &cli.host, cli.port, cli.auth_token).await?;
        }
        other => {
            anyhow::bail!(
                "Unsupported transport '{}'. Expected 'stdio' or 'http'.",
                other
            );
        }
    }

    Ok(())
}
