//! Cluster connection logic — pure, HTTP-free. The probe shells out to
//! the system `ssh` binary with BatchMode (no interactive prompts) and a
//! bounded timeout; every argument is passed as an argv entry, so no
//! shell quoting or injection surface exists.

use std::path::{Path, PathBuf};

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

/// The `user@host` target string for an SSH invocation, shared by both
/// cluster types (the probe holds `ClusterInfo`, remote execution a stored
/// `ClusterRow`).
pub fn ssh_target_string(ssh_user: Option<&str>, ssh_host: &str) -> String {
    match ssh_user {
        Some(user) => format!("{user}@{ssh_host}"),
        None => ssh_host.to_string(),
    }
}

/// The `user@host` target string for an SSH invocation.
pub fn ssh_target(cluster: &ClusterInfo) -> String {
    ssh_target_string(cluster.ssh_user.as_deref(), &cluster.ssh_host)
}

/// Whether a configured `ssh_key` is key *material* rather than a path.
///
/// `ClusterInfo.ssh_key` is documented as "path/material". Material cannot
/// be handed to `ssh -i` directly — OpenSSH treats it as a file name and
/// prints a multi-line warning on stderr.
pub fn is_key_material(configured: &str) -> bool {
    configured.contains("PRIVATE KEY") || configured.contains('\n')
}

/// A resolved `ssh -i` identity.
///
/// Path-form values are used verbatim (`~` is expanded by OpenSSH). Key
/// material is written to a private temp file — `tempfile` creates it with
/// mode 0600 on Unix — and removed when this guard drops, so the secret does
/// not outlive the SSH invocation.
pub struct SshIdentity {
    path: PathBuf,
    _material_file: Option<tempfile::NamedTempFile>,
}

impl SshIdentity {
    /// Resolve a configured `ssh_key` value into a usable `-i` path.
    pub fn resolve(configured: &str) -> std::io::Result<Self> {
        if !is_key_material(configured) {
            return Ok(Self {
                path: PathBuf::from(configured),
                _material_file: None,
            });
        }
        use std::io::Write;
        let mut file = tempfile::Builder::new()
            .prefix("oxo-flow-ssh-key-")
            .tempfile()?;
        file.write_all(configured.as_bytes())?;
        // OpenSSH's private-key parser rejects material whose last line has
        // no trailing newline ("error in libcrypto"). API callers routinely
        // lose it in transit (`$(cat key)` / JSON tooling), so normalize.
        if !configured.ends_with('\n') {
            file.write_all(b"\n")?;
        }
        file.flush()?;
        Ok(Self {
            path: file.path().to_path_buf(),
            _material_file: Some(file),
        })
    }

    /// The path to pass to `ssh -i`.
    pub fn path(&self) -> &Path {
        &self.path
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
    for (field, value) in [
        ("ssh_host", cluster.ssh_host.as_str()),
        ("ssh_user", cluster.ssh_user.as_deref().unwrap_or("")),
        ("remote_dir", cluster.remote_dir.as_deref().unwrap_or("")),
    ] {
        if value.is_empty() {
            continue;
        }
        // These values are interpolated into remote shell commands (escaped
        // at the call sites) and the ssh target argument — reject anything
        // that could break out of either context.
        if value.chars().any(|c| {
            c.is_control()
                || matches!(
                    c,
                    '\'' | '"' | '`' | '$' | ';' | '&' | '|' | '<' | '>' | '\n'
                )
        }) {
            return Err(format!(
                "{field} contains characters that are not allowed in shell contexts"
            ));
        }
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

/// Sanitize a probe error so the UI never leaks hostnames, IP addresses, or
/// local filesystem paths. The raw error is logged server-side for admins.
fn sanitize_probe_error(raw: &str) -> String {
    let has_hostname = raw
        .split_whitespace()
        .any(|w| w.trim_end_matches([':', ',', '.']).contains('.'));
    let has_ip = raw
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .any(|s| s.matches('.').count() == 3 && s.parse::<std::net::IpAddr>().is_ok());
    let has_path = raw.starts_with('/') || raw.contains('\\') || raw.contains('~');

    if has_ip || has_hostname || has_path {
        "Could not reach cluster scheduler".into()
    } else {
        raw.into()
    }
}

/// Probe a cluster endpoint over SSH: connectivity, remote hostname, and
/// scheduler detection. Best-effort — a failed probe is a structured
/// result, never a panic.
pub async fn probe(cluster: &ClusterInfo) -> ClusterProbeResult {
    let started = std::time::Instant::now();

    // tokio::process keeps the SSH round-trip off the blocking worker pool.
    let mut cmd = tokio::process::Command::new("ssh");
    for arg in ssh_base_args(cluster.ssh_port) {
        cmd.arg(arg);
    }
    // The stored value may be sealed (OXO_FLOW_MASTER_KEY set, #517) —
    // open it just before use. Material is written to a private temp file;
    // the guard keeps it alive until the probe completes.
    let identity = match cluster.ssh_key.as_deref().map(crate::infra::crypto::open) {
        Some(key) => match SshIdentity::resolve(&key) {
            Ok(identity) => Some(identity),
            Err(e) => {
                tracing::warn!("cluster probe failed: cannot prepare ssh identity: {e}");
                return ClusterProbeResult {
                    ok: false,
                    hostname: None,
                    scheduler: None,
                    version: None,
                    error: Some("Could not prepare the SSH key".into()),
                    duration_ms: started.elapsed().as_millis() as u64,
                };
            }
        },
        None => None,
    };
    if let Some(identity) = &identity {
        cmd.arg("-i").arg(identity.path());
    }
    cmd.arg(ssh_target(cluster));

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
            let raw = stderr
                .lines()
                .next()
                .unwrap_or("ssh probe failed")
                .to_string();
            tracing::warn!("cluster probe failed: {raw}");
            ClusterProbeResult {
                ok: false,
                hostname: None,
                scheduler: None,
                version: None,
                error: Some(sanitize_probe_error(&raw)),
                duration_ms,
            }
        }
        Err(e) => {
            let duration_ms = started.elapsed().as_millis() as u64;
            let raw = format!("failed to run ssh: {e}");
            tracing::warn!("cluster probe failed: {raw}");
            ClusterProbeResult {
                ok: false,
                hostname: None,
                scheduler: None,
                version: None,
                error: Some(sanitize_probe_error(&raw)),
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
    use super::super::types::ClusterUpsertRequest;
    use super::*;

    #[test]
    fn validates_good_definitions() {
        let req = ClusterUpsertRequest {
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
    fn rejects_shell_metacharacters_in_ssh_fields() {
        let base = ClusterUpsertRequest {
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
        let bad_host = super::super::types::ClusterUpsertRequest {
            ssh_host: "h'; rm -rf ~".into(),
            ..base.clone()
        };
        assert!(validate(&bad_host).is_err());
        let bad_user = super::super::types::ClusterUpsertRequest {
            ssh_user: Some("u`id`".into()),
            ..base.clone()
        };
        assert!(validate(&bad_user).is_err());
        let bad_dir = super::super::types::ClusterUpsertRequest {
            remote_dir: Some("~/'`$HOME".into()),
            ..base.clone()
        };
        assert!(validate(&bad_dir).is_err());
        // A plain directory with a space stays legal — the call sites quote it.
        let spaced_dir = super::super::types::ClusterUpsertRequest {
            remote_dir: Some("/data/my jobs".into()),
            ..base
        };
        assert!(validate(&spaced_dir).is_ok());
    }

    #[test]
    fn rejects_bad_ids_and_schedulers() {
        let base = ClusterUpsertRequest {
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

    #[test]
    fn distinguishes_key_material_from_path() {
        // Arrange
        let path = "~/.ssh/id_ed25519";
        let material = "-----BEGIN OPENSSH PRIVATE KEY-----\nnot-a-real-key-fixture\n-----END OPENSSH PRIVATE KEY-----\n";
        let multiline_without_marker = "line one\nline two\n";

        // Act / Assert
        assert!(!is_key_material(path));
        assert!(is_key_material(material));
        assert!(is_key_material(multiline_without_marker));
    }

    #[test]
    fn path_identity_is_used_verbatim() {
        // Arrange / Act
        let identity = SshIdentity::resolve("~/.ssh/id_ed25519").expect("path resolves");

        // Assert — no temp file, the configured path is passed through so
        // OpenSSH keeps expanding `~`.
        assert_eq!(identity.path().to_str(), Some("~/.ssh/id_ed25519"));
    }

    #[test]
    fn material_identity_lands_in_private_temp_file() {
        // Arrange
        let material = "-----BEGIN OPENSSH PRIVATE KEY-----\nsecret-material\n-----END OPENSSH PRIVATE KEY-----\n";

        // Act
        let identity = SshIdentity::resolve(material).expect("material resolves");
        let written = std::fs::read_to_string(identity.path()).expect("temp key readable");

        // Assert — content round-trips and, on Unix, the file is 0600.
        assert_eq!(written, material);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(identity.path())
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "key material must be owner-only");
        }

        // The guard removes the file on drop.
        let path = identity.path().to_path_buf();
        drop(identity);
        assert!(!path.exists(), "temp key file must be removed on drop");
    }

    #[test]
    fn material_without_trailing_newline_is_normalized() {
        // Arrange — what `$(cat key)` / JSON tooling sends: no final newline.
        let stripped = "-----BEGIN OPENSSH PRIVATE KEY-----\nsecret-material\n-----END OPENSSH PRIVATE KEY-----";

        // Act
        let identity = SshIdentity::resolve(stripped).expect("material resolves");
        let written = std::fs::read_to_string(identity.path()).expect("temp key readable");

        // Assert — OpenSSH's parser fails on a missing final newline
        // ("error in libcrypto"), so the temp copy gets one.
        assert_eq!(written, format!("{stripped}\n"));
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
            ssh_key_set: false,
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
