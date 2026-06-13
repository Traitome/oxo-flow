#![forbid(unsafe_code)]
//! oxo-flow-web — Standalone web server for the oxo-flow pipeline engine.

use anyhow::Result;
use clap::{Parser, ValueEnum};
use std::net::SocketAddr;

/// Server operation mode.
#[derive(Debug, Clone, ValueEnum)]
enum ServerMode {
    /// Personal workstation mode (127.0.0.1, no auth required).
    Personal,
    /// Team server mode (0.0.0.0, auth required).
    Team,
    /// HPC cluster mode (0.0.0.0, scheduler awareness).
    Hpc,
}

impl std::fmt::Display for ServerMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Personal => write!(f, "personal"),
            Self::Team => write!(f, "team"),
            Self::Hpc => write!(f, "hpc"),
        }
    }
}

/// oxo-flow Web Server — Bioinformatics workflow Command Center.
#[derive(Parser, Debug)]
#[command(
    name = "oxo-flow-web",
    version,
    long_version = oxo_flow_web::infra::license::VERSION_WITH_LICENSE,
    about = "Start the oxo-flow web interface"
)]
struct Cli {
    /// Server operation mode: personal, team, or hpc.
    #[arg(long, default_value = "personal", env = "OXO_FLOW_MODE")]
    mode: ServerMode,

    /// Host address to bind to.
    #[arg(long, default_value = "0.0.0.0", env = "OXO_FLOW_HOST")]
    host: String,

    /// Path to the built frontend dist directory for production serving.
    #[arg(long, default_value = "", env = "OXO_FLOW_FRONTEND_DIR")]
    frontend_dir: String,

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

    // Print license banner on startup
    eprintln!("{}", oxo_flow_web::infra::license::license_banner_text());

    // Determine effective host based on mode
    let mode_str = cli.mode.to_string();
    let effective_host = match cli.mode {
        ServerMode::Personal => {
            // Force localhost for personal mode unless explicitly overridden
            if cli.host == "0.0.0.0" {
                "127.0.0.1".to_string()
            } else {
                cli.host.clone()
            }
        }
        _ => cli.host.clone(),
    };

    tracing::info!(
        "Starting oxo-flow-web in {} mode on {}:{}",
        mode_str,
        effective_host,
        cli.port
    );

    // HPC mode: detect scheduler and show status
    if matches!(cli.mode, ServerMode::Hpc) {
        let hpc_status = oxo_flow_web::hpc::get_hpc_status();
        if hpc_status.available {
            tracing::info!(
                "HPC scheduler detected: {} (version: {})",
                hpc_status.scheduler,
                hpc_status.version.as_deref().unwrap_or("unknown")
            );
        } else {
            tracing::warn!("No HPC scheduler detected. Install SLURM, PBS/Torque, LSF, or SGE.");
        }
    }

    oxo_flow_web::db::init_db("sqlite://oxo-flow.db").await?;
    oxo_flow_web::db::recover_orphaned_runs().await?;

    let addr = SocketAddr::new(effective_host.parse()?, cli.port);
    tracing::info!("Starting oxo-flow-web server on {}", addr);

    // Use the domain-driven router from server.rs, merged with frontend
    let app = oxo_flow_web::server::build_router(&mode_str);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Listening on http://{addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(oxo_flow_web::shutdown_signal())
        .await?;

    Ok(())
}
