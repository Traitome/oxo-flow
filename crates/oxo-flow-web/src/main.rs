#![forbid(unsafe_code)]
//! oxo-flow-web — Standalone web server for the oxo-flow pipeline engine.

use anyhow::Result;
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let host = std::env::var("OXO_FLOW_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port: u16 = std::env::var("OXO_FLOW_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);

    let addr = SocketAddr::new(host.parse()?, port);
    tracing::info!("Starting oxo-flow-web server on {}", addr);

    let app = oxo_flow_web::build_router();
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Listening on http://{}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}
