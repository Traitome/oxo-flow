#![forbid(unsafe_code)]
//! oxo-flow-web — Standalone web server for the oxo-flow pipeline engine.

use anyhow::Result;
use clap::{CommandFactory, FromArgMatches, Parser, ValueEnum};

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

    // Platform config file (oxo-flow.web.toml / OXO_FLOW_CONFIG) supplies
    // the LOWEST-precedence defaults: CLI flag > env var > config file >
    // built-in default.
    let platform_config = oxo_flow_web::config::load();
    let mut command = Cli::command();
    if let Some(cfg) = &platform_config {
        // clap's default_value wants 'static — pass owned OsStr values
        // (the "string" feature enables From<String>).
        if let Some(mode) = cfg.server.mode.clone() {
            let mode = clap::builder::OsStr::from(mode);
            command = command.mut_arg("mode", |a| a.default_value(mode));
        }
        if let Some(host) = cfg.server.host.clone() {
            let host = clap::builder::OsStr::from(host);
            command = command.mut_arg("host", |a| a.default_value(host));
        }
        if let Some(port) = cfg.server.port {
            let port = clap::builder::OsStr::from(port.to_string());
            command = command.mut_arg("port", |a| a.default_value(port));
        }
        if let Some(base_path) = cfg.server.base_path.clone() {
            let base_path = clap::builder::OsStr::from(base_path);
            command = command.mut_arg("base_path", |a| a.default_value(base_path));
        }
    }
    let cli = Cli::from_arg_matches(&command.get_matches())?;

    // Print license banner on startup
    eprintln!("{}", oxo_flow_web::infra::license::license_banner_text());

    // Determine effective host based on mode — the enforcement itself lives
    // in `oxo_flow_web::effective_bind_host` so `oxo-flow serve` (the web
    // library path) and this binary share one auth boundary.
    let mode_str = cli.mode.to_string();
    let effective_host = oxo_flow_web::effective_bind_host(&mode_str, &cli.host)?;

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

    // Database initialization: PostgreSQL if DATABASE_URL starts with
    // postgres://, otherwise SQLite (default) — shared with `oxo-flow serve`
    // so the two entry points cannot drift (issue #569).
    oxo_flow_web::init_database().await?;

    // Initialize structured logging (three-layer logging per v0.8 spec)
    let log_dir = std::path::PathBuf::from("logs");
    if let Err(e) = oxo_flow_web::domains::observability::logging::init_logging(&log_dir) {
        tracing::warn!("Failed to initialize structured logging: {e}");
    } else {
        tracing::info!("Structured logging initialized at {}", log_dir.display());
    }

    // Initialize AI provider (env → DB → config file tiers) — shared with
    // `oxo-flow serve` via the hoisted helper (issue #569).
    oxo_flow_web::init_ai_provider(platform_config.as_ref()).await;
    // Cluster definitions from the platform config file are imported by
    // BOTH entry points (here and in start_server_with_mode) — idempotent,
    // existing DB rows win, and a no-op when the SQLite pool is absent.
    if let Some(cfg) = &platform_config {
        oxo_flow_web::domains::clusters::handlers::import_from_config(&cfg.clusters).await;
    }

    let addr = oxo_flow_web::resolve_bind_addr(&effective_host, cli.port)?;
    tracing::info!("Starting oxo-flow-web server on {}", addr);

    // Use the domain-driven router from server.rs, merged with frontend
    // Normalize first: the clap default is "/", which must be stored as ""
    // so the SPA <base> injection does not produce href="//" (first write
    // to BASE_PATH wins — a later normalize could not fix it).
    let base_path = oxo_flow_web::server::normalize_base_path(&cli.base_path);
    oxo_flow_web::server::set_base_path(&base_path);
    let app = oxo_flow_web::server::build_router(&mode_str);
    // Mount the whole app under --base-path when set (sub-path deployments,
    // e.g. behind a reverse proxy at /oxoflow). The flag was previously
    // parsed but never applied (issue #79 deployment modes).
    let app = if base_path.is_empty() {
        app
    } else {
        // `nest` registers GET <base_path> itself, but a request with the
        // trailing slash (/oxoflow/) lands on an empty remainder inside the
        // nest — route it explicitly so the mount root serves the SPA.
        let base = &base_path;
        axum::Router::new()
            .route(
                &format!("{base}/"),
                axum::routing::get(oxo_flow_web::server::spa_index),
            )
            .nest(base, app)
    };

    // Background maintenance every serving entry point needs (idempotent):
    // without the daily quota reset, runs_today only ever grows and every
    // POST /api/runs answers 429 until the process restarts.
    oxo_flow_web::start_background_tasks();

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?.port();
    oxo_flow_web::server::set_bound_port(bound);
    tracing::info!("Listening on http://{addr}");
    // The connect-info service feeds the rate limiter's peer-address key;
    // without it the limiter can only fall back to one shared bucket.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(oxo_flow_web::shutdown_signal())
    .await?;

    Ok(())
}
