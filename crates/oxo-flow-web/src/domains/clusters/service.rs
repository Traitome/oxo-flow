//! Cluster connection logic — pure, HTTP-free. The probe shells out to
//! the system `ssh` binary with BatchMode (no interactive prompts) and a
//! bounded timeout; every argument is passed as an argv entry, so no
//! shell quoting or injection surface exists.

use super::types::{ClusterInfo, ClusterProbeResult};

/// SSH option prefix shared by the probe and the remote-execution path:
/// BatchMode (no interactive prompts), bounded connect timeout, argv-only
/// arguments (no shell quoting surface).
pub fn ssh_base_args(port: u16) -> Vec<String> {
    vec![
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ConnectTimeout=8".into(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-p".into(),
        port.to_string(),
    ]
}

/// The `user@host` target string for an SSH invocation.
pub fn ssh_target(cluster: &ClusterInfo) -> String {
    match cluster.ssh_user.as_deref() {
        Some(user) => format!("{user}@{}", cluster.ssh_host),
        None => cluster.ssh_host.clone(),
    }
}

/// Validate a cluster definition before it touches storage or SSH.
pub fn validate(cluster: &super::types::ClusterUpsertRequest) -> Result<(), String> {
    if cluster.id.trim().is_empty() {
        return Err("id must not be empty".into());
    }
    if !cluster
        .id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("id may only contain letters, digits, '-' and '_'".into());
    }
    if cluster.name.trim().is_empty() {
        return Err("name must not be empty".into());
    }
    if cluster.ssh_host.trim().is_empty() {
        return Err("ssh_host must not be empty".into());
    }
    if let Some(scheduler) = cluster.scheduler.as_deref()
        && !matches!(scheduler, "auto" | "slurm" | "pbs" | "lsf" | "sge")
    {
        return Err(format!(
            "unknown scheduler '{scheduler}' — use auto, slurm, pbs, lsf, or sge"
        ));
    }
    Ok(())
}

/// Probe a cluster endpoint over SSH: connectivity, remote hostname, and
/// scheduler detection. Best-effort — a failed probe is a structured
/// result, never a panic.
pub async fn probe(cluster: &ClusterInfo) -> ClusterProbeResult {
    let started = std::time::Instant::now();

    // tokio::process keeps the SSH round-trip off the blocking worker pool.
    let mut cmd = tokio::process::Command::new("ssh");
    cmd.arg("-o").arg("BatchMode=yes");
    cmd.arg("-o").arg("ConnectTimeout=8");
    cmd.arg("-o").arg("StrictHostKeyChecking=accept-new");
    cmd.arg("-p").arg(cluster.ssh_port.to_string());
    if let Some(key) = cluster.ssh_key.as_deref() {
        cmd.arg("-i").arg(key);
    }
    let target = match cluster.ssh_user.as_deref() {
        Some(user) => format!("{user}@{}", cluster.ssh_host),
        None => cluster.ssh_host.clone(),
    };
    cmd.arg(target);

    // One remote round-trip gathers hostname + scheduler signature.
    // `; exit 0` matters: the presence checks are the last commands, and a
    // missing scheduler would otherwise flip the whole probe to "failed"
    // even though connectivity was fine.
    cmd.arg(
        "hostname; command -v sinfo >/dev/null 2>&1 && sinfo --version 2>/dev/null; \
         command -v qstat >/dev/null 2>&1 && echo pbs; \
         command -v bjobs >/dev/null 2>&1 && echo lsf; \
         command -v qsub >/dev/null 2>&1 && echo sge; exit 0",
    );

    match cmd.output().await {
        Ok(output) if output.status.success() => {
            let duration_ms = started.elapsed().as_millis() as u64;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let lines: Vec<&str> = stdout.lines().map(str::trim).collect();
            ClusterProbeResult {
                ok: true,
                hostname: lines.first().map(|s| s.to_string()),
                scheduler: Some(detect_scheduler(&stdout).to_string()),
                version: scheduler_version(&stdout),
                error: None,
                duration_ms,
            }
        }
        Ok(output) => {
            let duration_ms = started.elapsed().as_millis() as u64;
            let stderr = String::from_utf8_lossy(&output.stderr);
            ClusterProbeResult {
                ok: false,
                hostname: None,
                scheduler: None,
                version: None,
                error: Some(
                    stderr
                        .lines()
                        .next()
                        .unwrap_or("ssh probe failed")
                        .to_string(),
                ),
                duration_ms,
            }
        }
        Err(e) => {
            let duration_ms = started.elapsed().as_millis() as u64;
            ClusterProbeResult {
                ok: false,
                hostname: None,
                scheduler: None,
                version: None,
                error: Some(format!("failed to run ssh: {e}")),
                duration_ms,
            }
        }
    }
}

/// Detect the scheduler from the probe transcript (same vocabulary as the
/// local hpc detection).
fn detect_scheduler(stdout: &str) -> &'static str {
    if stdout.contains("slurm") {
        "slurm"
    } else if stdout.contains("pbs") {
        "pbs"
    } else if stdout.contains("lsf") {
        "lsf"
    } else if stdout.contains("sge") {
        "sge"
    } else {
        "none"
    }
}

fn scheduler_version(stdout: &str) -> Option<String> {
    // sinfo --version prints the slurm version line (e.g. "slurm 23.11.7").
    stdout
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("slurm "))
        .map(|l| l.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_good_definitions() {
        let req = super::super::types::ClusterUpsertRequest {
            id: "lab-slurm".into(),
            name: "Lab".into(),
            ssh_host: "login.lab.edu".into(),
            ssh_port: 22,
            ssh_user: Some("bioinf".into()),
            ssh_key: Some("~/.ssh/id_ed25519".into()),
            scheduler: Some("slurm".into()),
            remote_dir: None,
            enabled: true,
        };
        assert!(validate(&req).is_ok());
    }

    #[test]
    fn rejects_bad_ids_and_schedulers() {
        let base = super::super::types::ClusterUpsertRequest {
            id: "ok".into(),
            name: "x".into(),
            ssh_host: "h".into(),
            ssh_port: 22,
            ssh_user: None,
            ssh_key: None,
            scheduler: None,
            remote_dir: None,
            enabled: true,
        };
        let mut bad_id = base.clone();
        bad_id.id = "has space".into();
        assert!(validate(&bad_id).is_err());

        let mut bad_sched = base.clone();
        bad_sched.scheduler = Some("condor".into());
        assert!(validate(&bad_sched).is_err());
    }

    #[test]
    fn scheduler_detection_from_transcript() {
        assert_eq!(detect_scheduler("n1\nslurm 23.11.7\n"), "slurm");
        assert_eq!(detect_scheduler("n1\npbs\n"), "pbs");
        assert_eq!(detect_scheduler("n1\n"), "none");
        assert_eq!(
            scheduler_version("n1\nslurm 23.11.7\n"),
            Some("slurm 23.11.7".into())
        );
    }

    /// The probe against a nonexistent host must fail gracefully and fast.
    #[tokio::test]
    async fn probe_unreachable_host_returns_structured_failure() {
        let cluster = ClusterInfo {
            id: "gone".into(),
            name: "gone".into(),
            ssh_host: "no-such-host.invalid".into(),
            ssh_port: 22,
            ssh_user: Some("nobody".into()),
            ssh_key: None,
            scheduler: None,
            remote_dir: None,
            enabled: true,
            created_at: String::new(),
        };
        let result = probe(&cluster).await;
        assert!(!result.ok);
        assert!(result.error.is_some());
    }
}
