//! End-to-end wiring tests for the rule-shell environment allowlist
//! (issues #1108 / #1113 / #1173).
//!
//! The unit tests in `executor::process` pin the policy body; these tests pin
//! the SPAWN SITE — that a local docker rule actually receives the admitted
//! container-daemon variables, and that a bare rule does not. Without this,
//! flipping the spawn-site flag would silently reintroduce #1173 while every
//! policy unit test stayed green.

mod common;
use common::workspace_bin;

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

fn write_docker_workflow(dir: &Path) {
    std::fs::write(
        dir.join("wf.oxoflow"),
        r#"
[workflow]
name = "docker_env"

[[rules]]
name = "probe"
shell = "true"

[rules.environment]
docker = "fake/image:latest"
"#,
    )
    .unwrap();
}

fn write_bare_workflow(dir: &Path) {
    std::fs::write(
        dir.join("wf.oxoflow"),
        r#"
[workflow]
name = "bare_env"

[[rules]]
name = "probe"
shell = "printf '%s' \"$DOCKER_HOST\" > bare_env_observed.txt"
"#,
    )
    .unwrap();
}

/// A fake `docker` that records the daemon variable it was spawned with and
/// succeeds for every setup/verify/wrap invocation, so no real daemon is
/// needed.
fn fake_docker_bin(dir: &Path) -> PathBuf {
    let bin_dir = dir.join("fakebin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let path = bin_dir.join("docker");
    std::fs::write(
        &path,
        "#!/bin/sh\nprintf '%s' \"$DOCKER_HOST\" > docker_env_observed.txt\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
    bin_dir
}

fn run_with_env(dir: &Path, path_prefix: &Path) {
    let base_path = std::env::var("PATH").unwrap_or_default();
    let status = StdCommand::new(workspace_bin("oxo-flow"))
        .args(["run", "wf.oxoflow"])
        .current_dir(dir)
        .env("PATH", format!("{}:{base_path}", path_prefix.display()))
        .env("DOCKER_HOST", "tcp://sentinel.example:2376")
        .env("OXO_FLOW_AI_SESSION_DIR", dir.join("ai_sessions"))
        .status()
        .unwrap();
    assert!(status.success(), "run must succeed");
}

#[test]
fn docker_rule_shell_receives_the_admitted_daemon_variable() {
    let dir = tempfile::tempdir().unwrap();
    write_docker_workflow(dir.path());
    let bin_dir = fake_docker_bin(dir.path());
    run_with_env(dir.path(), &bin_dir);

    let observed =
        std::fs::read_to_string(dir.path().join("docker_env_observed.txt")).unwrap_or_default();
    assert_eq!(
        observed, "tcp://sentinel.example:2376",
        "the docker wrapper must see the admitted DOCKER_HOST (issue #1173)"
    );
}

#[test]
fn bare_rule_shell_does_not_receive_the_daemon_variable() {
    let dir = tempfile::tempdir().unwrap();
    write_bare_workflow(dir.path());
    let bin_dir = dir.path().join("unused_bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    run_with_env(dir.path(), &bin_dir);

    let observed = std::fs::read_to_string(dir.path().join("bare_env_observed.txt")).unwrap();
    assert_eq!(
        observed, "",
        "a bare rule must not observe DOCKER_HOST (issue #1113 allowlist)"
    );
}
