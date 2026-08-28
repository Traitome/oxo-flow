//! AI status, setup, and diagnostic commands.
//!
//! Three subcommands:
//! - `oxo-flow ai`       — quick status + connectivity check
//! - `oxo-flow ai test`  — comprehensive self-test (ping + generate + analyze)
//! - `oxo-flow ai setup` — interactive wizard to configure AI provider

use anyhow::Result;
use colored::Colorize;
use oxo_flow_ai::error::AiError;
use oxo_flow_ai::provider;

/// Carry an already-configured endpoint over a setup run that supplies none.
///
/// The wizard never asks for an endpoint, so a re-run used to persist
/// `api_url = ""` and silently drop a custom one. The saved URL is kept —
/// but only while the provider kind is unchanged, since an endpoint belongs
/// to the provider it was configured for.
fn keep_saved_api_url(
    kind: &str,
    new_url: Option<&str>,
    saved_kind: Option<&str>,
    saved_url: Option<&str>,
) -> Option<String> {
    match new_url {
        Some(url) => Some(url.to_string()),
        None => saved_kind
            .filter(|saved| saved.eq_ignore_ascii_case(kind))
            .and_then(|_| saved_url)
            .filter(|url| !url.is_empty())
            .map(String::from),
    }
}

/// The quick-status exit contract: the connectivity probe is a real network
/// call, and a FAIL must surface as a non-zero exit so scripts can rely on
/// `oxo-flow ai` — the same contract `ai test` follows.
fn connectivity_outcome(probe: &Result<String, AiError>) -> Result<()> {
    match probe {
        Ok(_) => Ok(()),
        Err(e) => anyhow::bail!(
            "Connectivity check failed: {e} — run 'oxo-flow ai test' for the full self-test"
        ),
    }
}

/// Quick AI status + connectivity test.
pub async fn ai_status_command() -> Result<()> {
    println!("{}", "AI Status".bold().green().underline());
    println!();

    let provider = provider::create_provider_from_env();
    let name = provider.name();
    let model = provider.model().unwrap_or_else(|| "default".into());

    // Discovered user-defined skills (read-only listing — discovery never
    // activates; activation requires [ai] skills = [...] in the workflow).
    let project_dir = std::env::current_dir().ok();
    let discovered = oxo_flow_ai::skill::discover_skills(project_dir.as_deref());
    println!();
    println!("{}", "Custom skills:".bold().cyan());
    if discovered.is_empty() {
        println!(
            "  None discovered. Add *.skill.toml files to ~/.oxo-flow/skills/\n  or <project>/.oxo-flow/skills/, then activate them with\n  [ai] skills = [...] in the workflow. See the Custom Skills reference."
        );
    } else {
        for skill in &discovered {
            println!(
                "  {} ({}) — {}{}",
                skill.name.cyan(),
                skill.skill_type.dimmed(),
                skill.description,
                if skill.domains.is_empty() {
                    String::new()
                } else {
                    format!(" [domains: {}]", skill.domains.join(", "))
                }
            );
        }
    }
    println!();

    // Embedded knowledge freshness (knowledge_meta.json): per-source record
    // count, generation date, staleness, and auto/manual origin.
    println!("{}", "Knowledge freshness:".bold().cyan());
    match oxo_flow_ai::knowledge::meta::embedded_meta() {
        Some(meta) => {
            let now = chrono::Utc::now();
            if meta.sources.is_empty() {
                println!("  (no sources recorded in knowledge_meta.json)");
            }
            for src in &meta.sources {
                let row = oxo_flow_ai::knowledge::meta::format_source_row(src, now);
                let staleness = src.is_stale(now);
                println!(
                    "  {}{}",
                    row,
                    if staleness {
                        format!(" {}", "STALE".yellow().bold())
                    } else {
                        String::new()
                    }
                );
            }
            println!(
                "  {}",
                "Auto-updated sources older than 60 days are flagged STALE and block releases."
                    .dimmed()
            );
        }
        None => println!(
            "  {}",
            "(knowledge_meta.json not embedded in this build — update the pipeline first)".dimmed()
        ),
    }
    println!();

    if name == "disabled" {
        println!("  Status: {}", "DISABLED".yellow().bold());
        println!();
        println!("  To configure AI, run:");
        println!("    {}", "oxo-flow ai setup".bold().cyan());
        println!();
        println!("  Or manually set environment variables:");
        println!("    export OXO_FLOW_AI_PROVIDER=<provider>");
        println!("    export <PROVIDER>_API_KEY=sk-...");
        return Ok(());
    }

    println!("  Provider:  {}", name.green());
    println!("  Model:     {}", model);
    if let Some(url) = provider.api_url() {
        println!("  Endpoint:  {}", url.dimmed());
    }

    // Connectivity
    println!();
    print!("  Connectivity ... ");
    let connectivity = provider.chat("You are helpful.", "Say OK").await;
    match &connectivity {
        Ok(_) => println!("{}", "OK".green()),
        Err(e) => println!("{} ({})", "FAIL".red(), e),
    }

    // Sessions
    let sessions_dir = oxo_flow_ai::session::sessions_dir();
    let count = std::fs::read_dir(&sessions_dir)
        .map(|e| e.filter_map(|x| x.ok()).count())
        .unwrap_or(0);
    println!("  Sessions:   {}", count.to_string().cyan());
    println!(
        "  Storage:    {}",
        sessions_dir.display().to_string().dimmed()
    );

    println!(
        "  Run {} for comprehensive self-test.",
        "oxo-flow ai test".bold()
    );

    connectivity_outcome(&connectivity)
}

/// Comprehensive self-test: connectivity + generation + analysis.
pub async fn ai_test_command() -> Result<()> {
    println!("{}", "AI Self-Test".bold().green().underline());
    println!();

    let provider = provider::create_provider_from_env();
    if provider.name() == "disabled" {
        anyhow::bail!("AI is disabled. Run 'oxo-flow ai setup' first.");
    }

    // Test 1: Connectivity
    print!("  [1/3] Connectivity ... ");
    match provider
        .chat("You are helpful.", "Reply with just the word OK")
        .await
    {
        Ok(r) if r.trim() == "OK" => println!("{}", "PASS".green()),
        Ok(r) => println!(
            "{} (unexpected: {})",
            "WARN".yellow(),
            &r[..20.min(r.len())]
        ),
        Err(e) => {
            println!("{} ({})", "FAIL".red(), e);
            anyhow::bail!("Connectivity test failed — check API key and network");
        }
    }

    // Test 2: Template generation
    print!("  [2/3] Generation ... ");
    let test_intent = "a single rule that writes hello world to a file";
    let system = r#"Generate a minimal .oxoflow workflow. Output ONLY the TOML inside ```toml fences.
The workflow should be: [workflow] name="test", [[rules]] name="hello", output=["hello.txt"],
threads=1, memory="1G", shell="echo hello > hello.txt".
No explanation, just the TOML."#;
    let user_msg = format!("Generate: {test_intent}");
    match provider.chat(system, &user_msg).await {
        Ok(response) => {
            if response.contains("[workflow]") && response.contains("[[rules]]") {
                println!("{}", "PASS".green());
            } else {
                println!("{} (no TOML in response)", "WARN".yellow());
            }
        }
        Err(e) => {
            println!("{} ({})", "FAIL".red(), e);
            anyhow::bail!("Generation test failed");
        }
    }

    // Test 3: Analysis (dry-run) on a trivial workflow
    print!("  [3/3] Analysis ... ");
    let test_toml = r#"[workflow]
name = "self-test"
[[rules]]
name = "step1"
output = ["out.txt"]
threads = 1
memory = "1G"
shell = "echo ok > out.txt"
"#;
    let system = "You are a workflow auditor. Reply ONLY with 'PASS' or 'FAIL: <reason>'.";
    let user = format!("Audit this workflow:\n```toml\n{test_toml}\n```");
    match provider.chat(system, &user).await {
        Ok(response) => {
            if response.contains("PASS") {
                println!("{}", "PASS".green());
            } else {
                println!(
                    "{} ({})",
                    "WARN".yellow(),
                    response.lines().next().unwrap_or("")
                );
            }
        }
        Err(e) => {
            println!("{} ({})", "FAIL".red(), e);
        }
    }

    println!();
    println!("{}", "All tests completed.".bold().green());
    println!("  Run 'oxo-flow ai' for quick status anytime.");
    Ok(())
}

/// Interactive setup wizard.
pub async fn ai_setup_command() -> Result<()> {
    // Detect non-interactive environments (CI, pipes, cargo run without TTY)
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        println!(
            "{}",
            "AI Setup — Non-Interactive Mode".bold().green().underline()
        );
        println!();
        println!("  The interactive wizard requires a terminal.");
        println!();
        println!("  To configure AI non-interactively, set environment variables:");
        println!("    export OXO_FLOW_AI_PROVIDER=<deepseek|openai|claude|ollama>");
        println!(
            "    export DEEPSEEK_API_KEY=sk-...     # or OPENAI_API_KEY, ANTHROPIC_AUTH_TOKEN"
        );
        println!();
        println!("  Or edit the config file directly:");
        println!("    {}", "~/.oxo-flow/ai_config.json".dimmed());
        println!();
        println!("  See docs: https://traitome.github.io/oxo-flow/latest/reference/ai-cli/");
        return Ok(());
    }

    println!("{}", "AI Setup Wizard".bold().green().underline());
    println!();
    println!("  This will configure AI for oxo-flow and save to:");
    println!("    {}", "~/.oxo-flow/ai_config.json".dimmed());
    println!();

    // Step 1: Choose provider
    println!("{}", "Step 1: Choose Provider".bold());
    println!("  [1] DeepSeek (OpenAI-compatible, recommended)");
    println!("  [2] OpenAI / OpenAI-compatible (Groq, Together, Azure, etc.)");
    println!("  [3] Anthropic Claude / Anthropic-compatible");
    println!("  [4] Ollama (local)");
    print!("\n  Select [1-4]: ");

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let choice = input.trim();

    let (provider_name, key_env, key_prompt) = match choice {
        "1" => ("deepseek", "DEEPSEEK_API_KEY", "Enter DeepSeek API key"),
        "2" => ("openai", "OPENAI_API_KEY", "Enter OpenAI API key"),
        "3" => ("claude", "ANTHROPIC_AUTH_TOKEN", "Enter Anthropic API key"),
        "4" => ("ollama", "", ""),
        _ => anyhow::bail!("Invalid choice: {choice}"),
    };

    // Step 2: API key (skip for Ollama)
    let api_key = if choice == "4" {
        None
    } else {
        println!();
        println!("{}", "Step 2: API Key".bold());
        println!("  {}:", key_prompt);
        print!("  ");
        let mut key = String::new();
        std::io::stdin().read_line(&mut key)?;
        let key = key.trim().to_string();
        if key.is_empty() {
            anyhow::bail!("API key is required");
        }
        Some(key)
    };

    // Step 3: Model (optional)
    println!();
    println!(
        "{}",
        "Step 3: Model (optional, press Enter for default)".bold()
    );
    print!("  Model name: ");
    let mut model = String::new();
    std::io::stdin().read_line(&mut model)?;
    let model = model.trim().to_string();
    let model = if model.is_empty() { None } else { Some(model) };

    // Save config. The wizard never asks for an endpoint, so keep the one
    // already persisted (for this provider) instead of wiping it.
    let key_ref: Option<&str> = api_key.as_deref();
    let model_ref: Option<&str> = model.as_deref();
    let saved = provider::load_ai_config();
    let api_url = keep_saved_api_url(
        provider_name,
        None,
        saved.as_ref().map(|(k, ..)| k.as_str()),
        saved.as_ref().map(|(_, _, url, _)| url.as_str()),
    );
    oxo_flow_ai::provider::save_ai_config(provider_name, key_ref, api_url.as_deref(), model_ref);

    println!();
    println!(
        "{} Configuration saved to ~/.oxo-flow/ai_config.json",
        "✓".green()
    );
    if api_key.is_some() {
        println!();
        println!("  {} To use in current terminal, run:", "💡".bold());
        println!("    export OXO_FLOW_AI_PROVIDER={provider_name}");
        println!("    export {key_env}=\"<your-key>\"");
    }

    // Test connectivity (provider picks up from saved config)
    print!("  Testing connectivity ... ");
    let provider = provider::create_provider_from_env();
    match provider.chat("You are helpful.", "Say OK").await {
        Ok(_) => println!("{}", "Connected!".green()),
        Err(e) => println!("{}: {}", "Failed".red(), e),
    }

    println!();
    println!(
        "  Run {} for comprehensive self-test.",
        "oxo-flow ai test".bold()
    );
    println!("  Run {} anytime to check status.", "oxo-flow ai".bold());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_without_url_keeps_saved_endpoint_for_same_provider() {
        assert_eq!(
            keep_saved_api_url(
                "deepseek",
                None,
                Some("deepseek"),
                Some("https://api.example.com/v1")
            ),
            Some("https://api.example.com/v1".to_string())
        );
    }

    #[test]
    fn setup_switching_provider_does_not_inherit_endpoint() {
        assert_eq!(
            keep_saved_api_url(
                "ollama",
                None,
                Some("deepseek"),
                Some("https://api.example.com/v1")
            ),
            None
        );
        // Case differences in the provider spelling are the same provider.
        assert_eq!(
            keep_saved_api_url(
                "DeepSeek",
                None,
                Some("deepseek"),
                Some("https://api.example.com/v1")
            )
            .as_deref(),
            Some("https://api.example.com/v1")
        );
    }

    #[test]
    fn setup_explicit_url_wins_and_empty_saved_url_is_ignored() {
        assert_eq!(
            keep_saved_api_url(
                "deepseek",
                Some("https://new.example.com/v1"),
                Some("deepseek"),
                Some("https://old.example.com/v1")
            ),
            Some("https://new.example.com/v1".to_string())
        );
        assert_eq!(
            keep_saved_api_url("deepseek", None, Some("deepseek"), Some("")),
            None
        );
        assert_eq!(keep_saved_api_url("deepseek", None, None, None), None);
    }

    #[test]
    fn connectivity_outcome_propagates_probe_failure() {
        assert!(connectivity_outcome(&Ok("OK".to_string())).is_ok());

        let err = AiError::Auth {
            provider: "openai".into(),
            message: "HTTP 401: bad key".into(),
        };
        let outcome = connectivity_outcome(&Err(err)).unwrap_err().to_string();
        assert!(
            outcome.contains("Connectivity check failed"),
            "unexpected message: {outcome}"
        );
    }
}
