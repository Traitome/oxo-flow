//! Logic for the 'serve' command.

use crate::commands::print_banner;
use anyhow::Result;
use colored::Colorize;

pub async fn handle_serve(host: String, port: u16) -> Result<()> {
    print_banner();
    eprintln!(
        "{} Starting oxo-flow web server on {}:{}",
        "Serve:".bold().cyan(),
        host,
        port
    );

    // Decoupling: We only call into oxo_flow_web if it's enabled or just use the public API.
    // For now, let's assume it's available.
    oxo_flow_web::start_server(&host, port).await?;

    Ok(())
}
