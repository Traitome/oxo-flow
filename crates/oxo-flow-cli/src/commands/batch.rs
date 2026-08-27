use anyhow::{Context, Result};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

use crate::commands::{
    collect_batch_items, expand_batch_template, print_banner, run_batch_command, wrap_batch_command,
};

/// Result for a single batch item.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BatchResult {
    pub item: String,
    pub command: String,
    pub exit_code: Option<i32>,
    pub success: bool,
    pub error: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub async fn batch_command(
    template: String,
    items: Vec<String>,
    jobs: usize,
    stop_on_error: bool,
    file: Option<PathBuf>,
    json_output: bool,
    dry_run: bool,
    workdir: Option<PathBuf>,
    environment: Option<String>,
    checksum: bool,
    generate_workflow: bool,
    output: Option<PathBuf>,
) -> Result<()> {
    print_banner();

    // Collect items from command line or file (validates non-empty)
    let items = collect_batch_items(&items, file.as_ref())?;

    // Generate workflow instead of executing
    if generate_workflow {
        return generate_batch_workflow(&template, &items, output.as_ref(), environment.as_ref());
    }

    // Dry-run: print expanded commands
    if dry_run {
        eprintln!(
            "{} {} items with template: {}",
            "Batch (dry-run):".bold().yellow(),
            items.len(),
            template
        );
        for (nr, item) in items.iter().enumerate() {
            let cmd = expand_batch_template(&template, item, nr + 1);
            eprintln!("  [{}] {}", nr + 1, cmd);
        }
        return Ok(());
    }

    let semaphore = Arc::new(Semaphore::new(jobs));
    let results = Arc::new(Mutex::new(Vec::new()));
    let stop_flag = Arc::new(AtomicBool::new(false));

    let progress = ProgressBar::new(items.len() as u64);
    let style = ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({msg})")?
        .progress_chars("#>-");
    progress.set_style(style);

    let mut tasks = Vec::new();

    let env_clone = environment.clone();
    let workdir_clone = workdir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    for (nr, item) in items.iter().enumerate() {
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }

        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            // Safe: the semaphore is never closed; acquire only fails on a
            // closed semaphore, and we hold Arc clones for the whole run.
            .expect("batch semaphore closed unexpectedly");
        let cmd = expand_batch_template(&template, item, nr + 1);

        let item_clone = item.clone();
        let cmd_clone = cmd.clone();
        let results_clone = results.clone();
        let stop_flag_clone = stop_flag.clone();
        let progress_clone = progress.clone();
        let env_clone = env_clone.clone();
        let workdir_clone = workdir_clone.clone();

        progress.set_message(format!("item {}", nr + 1));

        if stop_flag_clone.load(Ordering::Relaxed) {
            progress_clone.inc(1);
            if stop_on_error {
                stop_flag_clone.store(true, Ordering::Relaxed);
            }
            // Release permit and continue
            drop(permit);
            continue;
        }

        // Wrap with environment if specified
        let final_cmd = if let Some(env) = &env_clone {
            wrap_batch_command(&cmd, env)
        } else {
            cmd.clone()
        };

        let task = tokio::spawn(async move {
            // Clone for spawn_blocking (needs 'static lifetime)
            let final_cmd_for_blocking = final_cmd.clone();
            let workdir_for_blocking = workdir_clone.clone();

            // Execute command in blocking context
            let result = tokio::task::spawn_blocking(move || {
                run_batch_command(&final_cmd_for_blocking, &workdir_for_blocking)
            })
            .await;

            // Join failure means the blocking thread died (panic/pressure) —
            // record it as this item's error instead of panicking the task.
            let exit_result = match result {
                Ok(r) => r,
                Err(join_err) => Err(anyhow::anyhow!("batch worker crashed: {join_err}")),
            };

            // Record result
            if let Ok(mut results_lock) = results_clone.lock() {
                match exit_result {
                    Ok(exit_code) => {
                        results_lock.push(BatchResult {
                            item: item_clone.clone(),
                            command: cmd_clone,
                            exit_code: Some(exit_code),
                            success: exit_code == 0,
                            error: None,
                        });
                        if exit_code != 0 && stop_on_error {
                            stop_flag_clone.store(true, Ordering::Relaxed);
                        }
                        if checksum {
                            tracing::info!("checksum verification for {}", item_clone);
                        }
                    }
                    Err(e) => {
                        results_lock.push(BatchResult {
                            item: item_clone.clone(),
                            command: cmd_clone,
                            exit_code: None,
                            success: false,
                            error: Some(e.to_string()),
                        });
                        if stop_on_error {
                            stop_flag_clone.store(true, Ordering::Relaxed);
                        }
                    }
                }
            }
            progress_clone.inc(1);

            // Release permit
            drop(permit);
        });

        tasks.push(task);
    }

    // Wait for all tasks to complete
    for task in tasks {
        if let Err(e) = task.await {
            tracing::error!("task failed: {}", e);
        }
    }

    progress.finish_and_clear();

    // Calculate summary
    let results_lock = results
        .lock()
        .map_err(|_| anyhow::anyhow!("results mutex poisoned"))?;
    let total_results = results_lock.len();
    let failed = results_lock.iter().filter(|r| !r.success).count();
    let done = total_results - failed;

    // Output summary
    if json_output {
        let output = serde_json::json!({
            "tool": "oxo-flow batch",
            "template": template,
            "total": total_results,
            "failed": failed,
            "success": failed == 0,
            "results": results_lock.clone()
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        eprintln!(
            "\n{} {}/{} items succeeded, {} failed",
            "Done:".bold(),
            done,
            total_results,
            failed
        );
        if failed > 0 {
            eprintln!("\n{}", "Failures:".bold().red());
            for r in results_lock.iter().filter(|r| !r.success) {
                eprintln!(
                    "  {} — {}",
                    r.item.bold(),
                    r.error.as_deref().unwrap_or("exit code non-zero")
                );
            }
        }
    }

    if failed > 0 && stop_on_error {
        return Err(anyhow::anyhow!("batch execution failed"));
    }

    Ok(())
}

/// Render `shell = <cmd>` as a TOML table line (with a trailing blank line).
/// Serializing through the toml crate handles backslashes, quotes and
/// newlines correctly, unlike manual escaping which produced invalid TOML for
/// commands containing literal `\n` or trailing `\`.
fn format_shell_line(cmd: &str) -> Result<String> {
    let mut table = toml::map::Map::new();
    table.insert("shell".to_string(), toml::Value::String(cmd.to_string()));
    let rendered = toml::to_string(&toml::Value::Table(table))
        .with_context(|| format!("serialize shell command {cmd:?} as TOML"))?;
    // rendered ends with one newline from the serializer.
    Ok(format!("{rendered}\n"))
}

pub fn generate_batch_workflow(
    template: &str,
    items: &[String],
    output_path: Option<&PathBuf>,
    environment: Option<&String>,
) -> Result<()> {
    let output_path = output_path
        .cloned()
        .unwrap_or_else(|| PathBuf::from("batch.oxoflow"));

    let mut content = String::new();
    content.push_str("[workflow]\n");
    content.push_str("name = \"batch_workflow\"\n");
    content.push_str("version = \"0.1.0\"\n\n");

    for (i, item) in items.iter().enumerate() {
        let cmd = expand_batch_template(template, item, i + 1);
        content.push_str("[[rules]]\n");
        content.push_str(&format!("name = \"batch_item_{}\"\n", i + 1));
        if let Some(env) = environment {
            content.push_str(&format!("environment = \"{}\"\n", env));
        }
        content.push_str(&format_shell_line(&cmd)?);
    }

    std::fs::write(&output_path, content)
        .with_context(|| format!("failed to write {}", output_path.display()))?;

    eprintln!(
        "{} Generated batch workflow: {}",
        "✓".green().bold(),
        output_path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_line_roundtrips_hostile_commands() {
        for cmd in [
            "echo 'single'",
            "awk '{print $1}'",
            "printf \"tab\\there\"",
            "echo back\\slash",
            "trailing\\",
            "multi\nline\ncmd",
            "quote \" in middle",
        ] {
            let line = format_shell_line(cmd).expect("serialization");
            let doc = format!("[r]\n{line}");
            let parsed: toml::Value =
                toml::from_str(&doc).unwrap_or_else(|e| panic!("invalid TOML for {cmd:?}: {e}"));
            assert_eq!(
                parsed["r"]["shell"].as_str().unwrap(),
                cmd,
                "roundtrip mismatch for {cmd:?}"
            );
        }
    }

    #[test]
    fn generated_workflow_parses_with_hostile_template() {
        let out = std::env::temp_dir().join(format!("batch-test-{}.toml", std::process::id()));
        generate_batch_workflow(
            "echo {}",
            &["we\"ird".to_string(), "back\\slash".to_string()],
            Some(&out),
            None,
        )
        .unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        let _parsed: toml::Value = toml::from_str(&text)
            .unwrap_or_else(|e| panic!("generated file must be valid TOML: {e}\n{text}"));
        std::fs::remove_file(&out).ok();
    }
}
