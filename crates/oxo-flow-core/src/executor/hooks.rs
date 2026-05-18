use super::security::validate_shell_safety;
use crate::error::Result;
use crate::rule::Rule;
use std::path::Path;
use tokio::process::Command;

/// Execute a rule hook (pre_exec, on_success, on_failure).
pub async fn execute_hook(name: &str, cmd: &str, rule: &Rule, workdir: &Path) -> Result<()> {
    validate_shell_safety(cmd)?;

    tracing::info!(rule = %rule.name, hook = %name, cmd = %cmd, "executing hook");

    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(workdir)
        .envs(&rule.envvars)
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            tracing::warn!(
                rule = %rule.name,
                hook = %name,
                code = %o.status.code().unwrap_or(-1),
                stderr = %stderr.trim(),
                "hook failed"
            );
            // We return Ok(()) here because hooks failing shouldn't necessarily
            // fail the entire rule execution unless it's pre_exec.
            // Caller handles pre_exec specifically.
            Ok(())
        }
        Err(e) => {
            tracing::error!(rule = %rule.name, hook = %name, error = %e, "failed to spawn hook");
            Ok(())
        }
    }
}
