//! Logic for the 'serve' command.

use crate::commands::print_banner;
use anyhow::Result;
use colored::Colorize;

pub async fn handle_serve(host: String, port: u16, base_path: String) -> Result<()> {
    print_banner();
    eprintln!(
        "{} Starting oxo-flow web server on {}:{}{}",
        "Serve:".bold().cyan(),
        host,
        port,
        if base_path == "/" {
            String::new()
        } else {
            format!(" (base: {base_path})")
        }
    );

    if base_path == "/" {
        oxo_flow_web::start_server(&host, port).await?;
    } else {
        oxo_flow_web::start_server_with_base(&host, port, &base_path).await?;
    }

    Ok(())
}
