//! AI status and diagnostics command.
//!
//! Provides: provider connectivity test, config inspection, recent session listing.

use anyhow::Result;
use colored::Colorize;
use oxo_flow_ai::provider;

/// Display AI configuration status and test connectivity.
pub async fn ai_status_command() -> Result<()> {
    println!("{}", "AI Status".bold().green().underline());
    println!();

    // Provider config
    let provider = provider::create_provider_from_env();
    let name = provider.name();
    let model = provider.model().unwrap_or_else(|| "default".into());

    if name == "disabled" {
        println!("  Status: {}", "DISABLED".yellow().bold());
        println!();
        println!("  To enable AI, set environment variables:");
        println!("    export OXO_FLOW_AI_PROVIDER=deepseek");
        println!("    export DEEPSEEK_API_KEY=sk-...");
        println!();
        println!("  Or persist config:");
        println!("    ~/.oxo-flow/ai_config.json");
        return Ok(());
    }

    println!("  Provider:  {}", name.green());
    println!("  Model:     {}", model);
    if let Some(url) = provider.api_url() {
        println!("  Endpoint:  {}", url.dimmed());
    }

    // Connectivity test
    println!();
    println!("{}", "Connectivity Test".bold());
    match provider
        .chat("You are a helpful assistant.", "Respond with just: OK")
        .await
    {
        Ok(response) => {
            let preview = if response.len() > 100 {
                format!("{}...", &response[..100])
            } else {
                response
            };
            println!("  {} Response: {}", "✓".green(), preview.dimmed());
        }
        Err(e) => {
            println!("  {} Failed: {}", "✗".red(), e);
        }
    }

    // Session directory
    println!();
    println!("{}", "Session Storage".bold());
    let sessions_dir = oxo_flow_ai::session::sessions_dir();
    println!("  Path: {}", sessions_dir.display().to_string().dimmed());
    match std::fs::read_dir(&sessions_dir) {
        Ok(entries) => {
            let count = entries.filter_map(|e| e.ok()).count();
            println!("  Sessions: {}", count.to_string().cyan());
        }
        Err(_) => {
            println!("  Sessions: none yet");
        }
    }

    Ok(())
}
