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

    // Credential visibility (issue #79 P1-06): sign-in and user management
    // depend on env-var credentials — warn loudly when none are configured
    // instead of letting the first sign-in hit an unexplained 401 wall.
    if std::env::var("OXO_FLOW_ADMIN_PASSWORD").is_err()
        && std::env::var("OXO_FLOW_USER_PASSWORD").is_err()
        && std::env::var("OXO_FLOW_VIEWER_PASSWORD").is_err()
    {
        tracing::warn!(
            "No sign-in credentials configured (OXO_FLOW_ADMIN_PASSWORD / \
             OXO_FLOW_USER_PASSWORD / OXO_FLOW_VIEWER_PASSWORD). Every login \
             will be rejected until one is set; personal mode does not \
             require sign-in for daily use."
        );
    }

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

    // Database initialization: PostgreSQL if DATABASE_URL starts with postgres://,
    // otherwise SQLite (default).
    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://oxo-flow.db".to_string());

    let is_postgres =
        database_url.starts_with("postgres://") || database_url.starts_with("postgresql://");

    if is_postgres {
        #[cfg(feature = "postgres")]
        {
            tracing::info!("Initializing PostgreSQL backend");
            oxo_flow_web::infra::db::postgres::init_pool(&database_url).await;
        }
        #[cfg(not(feature = "postgres"))]
        {
            tracing::error!(
                "DATABASE_URL is a PostgreSQL URL but the 'postgres' feature is not enabled. \
                 Rebuild with: cargo build --features postgres"
            );
            anyhow::bail!("PostgreSQL support not compiled in — rebuild with --features postgres");
        }
    } else {
        oxo_flow_web::db::init_db(&database_url).await?;
        oxo_flow_web::db::recover_orphaned_runs().await?;
        // Also initialize the new v0.8 domain-driven DB pool for domain handlers
        oxo_flow_web::infra::db::sqlite::init_pool(&database_url).await;
    }

    // Initialize structured logging (three-layer logging per v0.8 spec)
    let log_dir = std::path::PathBuf::from("logs");
    if let Err(e) = oxo_flow_web::domains::observability::logging::init_logging(&log_dir) {
        tracing::warn!("Failed to initialize structured logging: {e}");
    } else {
        tracing::info!("Structured logging initialized at {}", log_dir.display());
    }

    // Initialize AI provider from environment variables
    oxo_flow_web::ai_provider::AiProviderRegistry::global().init_from_env();
    // Restore the DB-persisted tier (settings UI) when env did not configure
    // a provider — otherwise a saved key would be lost on restart.
    oxo_flow_web::domains::ai::handlers::restore_ai_config_from_db().await;
    tracing::info!(
        "AI provider: {}",
        oxo_flow_web::ai_provider::AiProviderRegistry::global()
            .get_config()
            .provider
    );

    let addr = SocketAddr::new(effective_host.parse()?, cli.port);
    tracing::info!("Starting oxo-flow-web server on {}", addr);

    // Use the domain-driven router from server.rs, merged with frontend
    oxo_flow_web::server::set_base_path(&cli.base_path);
    let app = oxo_flow_web::server::build_router(&mode_str);
    // Mount the whole app under --base-path when set (sub-path deployments,
    // e.g. behind a reverse proxy at /oxoflow). The flag was previously
    // parsed but never applied (issue #79 deployment modes).
    let app = if cli.base_path.is_empty() || cli.base_path == "/" {
        app
    } else {
        // `nest` registers GET <base_path> itself, but a request with the
        // trailing slash (/oxoflow/) lands on an empty remainder inside the
        // nest — route it explicitly so the mount root serves the SPA.
        let base = &cli.base_path;
        axum::Router::new()
            .route(
                &format!("{base}/"),
                axum::routing::get(oxo_flow_web::server::spa_index),
            )
            .nest(base, app)
    };

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Listening on http://{addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(oxo_flow_web::shutdown_signal())
        .await?;

    Ok(())
}
