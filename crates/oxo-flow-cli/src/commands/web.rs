//! Logic for the 'serve' command.

use crate::commands::print_banner;
use anyhow::Result;
use colored::Colorize;

pub async fn handle_serve(
    mode: String,
    host: String,
    port: u16,
    base_path: String,
    open_browser: bool,
) -> Result<()> {
    print_banner();
    let base = if base_path.is_empty() || base_path == "/" {
        String::new()
    } else {
        format!("/{}", base_path.trim_matches('/'))
    };
    eprintln!(
        "{} Starting oxo-flow web server in {} mode on {}:{}{}",
        "Serve:".bold().cyan(),
        mode,
        host,
        port,
        if base.is_empty() {
            String::new()
        } else {
            format!(" (base: {base})")
        }
    );

    // Desktop-app experience: open the interface in the default browser
    // once the server is listening. Fire-and-forget — a missing opener
    // (headless server) must not fail the serve command.
    if open_browser {
        let url = format!("http://{host}:{port}{base}");
        tokio::spawn(async move {
            // Small delay so the listener is up before the browser hits it.
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            if let Err(e) = open_url_in_browser(&url) {
                eprintln!(
                    "{} Could not open a browser automatically: {e} — open {url} manually",
                    "Note:".yellow()
                );
            }
        });
    }

    // Pass the NORMALIZED path — axum's nest() panics on a mount path
    // without a leading slash (e.g. `--base-path oxoflow`), so the raw
    // argument must never reach the router.
    oxo_flow_web::start_server_with_mode(&mode, &host, port, &base).await?;

    Ok(())
}

/// Open a URL in the platform's default browser (best-effort).
fn open_url_in_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map(|_| ())
    }
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", "", url])
            .spawn()
            .map(|_| ())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no browser opener on this platform",
        ))
    }
}
