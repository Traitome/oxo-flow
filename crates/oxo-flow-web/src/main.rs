#![forbid(unsafe_code)]
//! oxo-flow-web — Standalone web server for the oxo-flow pipeline engine.

use anyhow::Result;
use clap::Parser;
use std::net::SocketAddr;

/// oxo-flow Web Server — Bioinformatics workflow Command Center.
#[derive(Parser, Debug)]
#[command(
    name = "oxo-flow-web",
    version,
    about = "Start the oxo-flow web interface"
)]
struct Cli {
    /// Host address to bind to.
    #[arg(long, default_value = "0.0.0.0", env = "OXO_FLOW_HOST")]
    host: String,

    /// Port to listen on.
    #[arg(short = 'p', long, default_value = "3000", env = "OXO_FLOW_PORT")]
    port: u16,

    /// Base path for mounting under a sub-path.
    #[arg(long, default_value = "/")]
    base_path: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    oxo_flow_web::db::init_db("sqlite://oxo-flow.db").await?;
    oxo_flow_web::db::recover_orphaned_runs().await?;

    let addr = SocketAddr::new(cli.host.parse()?, cli.port);
    tracing::info!("Starting oxo-flow-web server on {}", addr);

    if cli.base_path == "/" || cli.base_path.is_empty() {
        let app = oxo_flow_web::build_router();
        let listener = tokio::net::TcpListener::bind(addr).await?;
        tracing::info!("Listening on http://{addr}");
        axum::serve(listener, app)
            .with_graceful_shutdown(oxo_flow_web::shutdown_signal())
            .await?;
    } else {
        let app = oxo_flow_web::build_router_with_base(&cli.base_path);
        let listener = tokio::net::TcpListener::bind(addr).await?;
        tracing::info!("Listening on http://{addr}{}", cli.base_path);
        axum::serve(listener, app)
            .with_graceful_shutdown(oxo_flow_web::shutdown_signal())
            .await?;
    }

    Ok(())
}
