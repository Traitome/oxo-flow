//! Registry of running CLI subprocess groups, keyed by run id.
//!
//! The executor spawns each run's `oxo-flow` CLI in its own process group
//! (`process_group(0)`), so signaling the group reaches the CLI and every
//! rule subprocess it spawned — the same group semantics the engine's own
//! timeout enforcement uses. Handlers look the group up here to cancel
//! (SIGTERM → SIGKILL) or pause/resume (SIGSTOP/SIGCONT) a run.

use std::collections::HashMap;
use std::io;
use std::sync::{OnceLock, RwLock};

pub const SIGTERM: nix::sys::signal::Signal = nix::sys::signal::Signal::SIGTERM;
pub const SIGKILL: nix::sys::signal::Signal = nix::sys::signal::Signal::SIGKILL;
pub const SIGSTOP: nix::sys::signal::Signal = nix::sys::signal::Signal::SIGSTOP;
pub const SIGCONT: nix::sys::signal::Signal = nix::sys::signal::Signal::SIGCONT;

/// run id → process group id of the live CLI subprocess.
static REGISTRY: OnceLock<RwLock<HashMap<String, i32>>> = OnceLock::new();

fn registry() -> &'static RwLock<HashMap<String, i32>> {
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Track a live subprocess group under `run_id` (the child's PID doubles as
/// its pgid when spawned with `process_group(0)`).
pub fn register(run_id: &str, pgid: i32) {
    registry()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(run_id.to_string(), pgid);
}

/// Drop tracking after the subprocess has exited.
pub fn unregister(run_id: &str) {
    registry()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(run_id);
}

/// Current process group id for `run_id`, if the subprocess is still tracked.
pub fn pgid(run_id: &str) -> Option<i32> {
    registry()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(run_id)
        .copied()
}

/// Send `sig` to the entire process group identified by `pgid`.
///
/// The crate forbids `unsafe_code`, so this goes through `nix`'s safe
/// `killpg` wrapper instead of calling `libc::kill` directly.
pub fn signal_group(pgid: i32, sig: nix::sys::signal::Signal) -> io::Result<()> {
    use nix::unistd::Pid;

    nix::sys::signal::killpg(Pid::from_raw(pgid), sig).map_err(io::Error::from)
}

/// Probe whether the process group identified by `pgid` still has members
/// (signal 0). A group outlives its leader, so this stays true while
/// orphaned rule subprocesses run on; zombies count until reaped, which is
/// fine for grace windows — the reaper drains them.
pub fn group_alive(pgid: i32) -> bool {
    use nix::unistd::Pid;

    nix::sys::signal::killpg(Pid::from_raw(pgid), None::<nix::sys::signal::Signal>).is_ok()
}

/// Probe whether `pid` still exists (signal 0).
///
/// Zombies answer the probe but are semantically dead, so the state is
/// double-checked via `ps`; a zombie counts as gone. When `ps` itself
/// cannot be consulted, the probe result stands (fail-safe toward keeping
/// a run monitored rather than reaping it early).
pub fn probe_alive(pid: i32) -> bool {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    if kill(Pid::from_raw(pid), None).is_err() {
        return false;
    }
    match std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()
    {
        Ok(out) if out.status.success() => !String::from_utf8_lossy(&out.stdout).contains('Z'),
        _ => true,
    }
}

/// Best-effort guard against pid recycling: after a restart, a reused pid
/// may be alive but belong to an unrelated process. The command line is
/// inspected for the CLI; a mismatch means the recorded pid is stale.
///
/// The match is ANCHORED, not a bare substring: the recorded process must
/// be the oxo-flow CLI itself (its binary path as the FIRST argv element)
/// or the executor's `sh -c` wrapper (an `.exit-code` path directly before
/// the binary token). A process whose command line merely MENTIONS
/// "oxo-flow" — an editor session, `grep -r oxo-flow`, a shell whose cwd
/// contains the name — must NOT pass the probe; the guard's only job is to
/// prove the pid belongs to a run the server itself started.
pub fn looks_like_oxo_flow(pid: i32) -> bool {
    match std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .output()
    {
        Ok(out) => cmdline_is_oxo_flow(&String::from_utf8_lossy(&out.stdout)),
        Err(_) => false,
    }
}

/// Whether one `ps` args= line belongs to the oxo-flow CLI or its wrapper.
///
/// Two accepted shapes, both anchored on the command line's structure:
///   - the CLI itself: the FIRST argv element is the oxo-flow binary (a
///     path whose file name is `oxo-flow`, or the bare name from a PATH
///     lookup — the executor's payload);
///   - the executor's wrapper (see [`crate::executor`]): the line carries
///     an exit-record path (file name `.exit-code`) followed within a few
///     argv elements by the oxo-flow binary token — the exact composition
///     of `spawn_background_run_with_args`, including the `sudo -n -u
///     <user>` prefix in between.
fn cmdline_is_oxo_flow(args: &str) -> bool {
    let tokens: Vec<&str> = args.split_whitespace().collect();
    if tokens.is_empty() {
        return false;
    }
    if is_oxo_flow_token(tokens[0]) {
        return true;
    }
    for (i, token) in tokens.iter().enumerate() {
        if std::path::Path::new(token)
            .file_name()
            .is_some_and(|f| f == ".exit-code")
        {
            let lookahead = &tokens[i + 1..(i + 1 + 5).min(tokens.len())];
            if lookahead.iter().any(|t| is_oxo_flow_token(t)) {
                return true;
            }
        }
    }
    false
}

fn is_oxo_flow_token(token: &str) -> bool {
    std::path::Path::new(token)
        .file_name()
        .is_some_and(|f| f == "oxo-flow")
}

/// Group-level identity guard for the case where the group LEADER has been
/// reaped but the group itself survives — the wrapper `sh` died while the
/// orphaned CLI and rule subprocesses keep running under the same pgid (the
/// window `cancel_run`'s DB-pid fallback must cover, issue #136 tier-2).
/// The leader's command line is gone, so every surviving member is scanned:
/// at least one must look like the CLI or its wrapper.
///
/// `ps -g <pgid>` means something different per platform (user group here,
/// session-or-group on Linux), so all processes are enumerated and filtered
/// on the `pgid` column instead — identical output on both.
pub fn group_looks_like_oxo_flow(pgid: i32) -> bool {
    match std::process::Command::new("ps")
        .args(["-e", "-o", "pid=,pgid=,args="])
        .output()
    {
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).lines().any(|line| {
                // ps pads columns to the widest value, so `pid pgid` can be
                // separated by several spaces — split on whitespace RUNS and
                // rejoin the args column (the rejoin is lossless for the
                // downstream tokenizer, which splits on whitespace anyway).
                let mut fields = line.split_whitespace();
                let member_pgid = fields.nth(1).and_then(|g| g.parse::<i32>().ok());
                member_pgid == Some(pgid) && {
                    let args = fields.collect::<Vec<_>>().join(" ");
                    cmdline_is_oxo_flow(&args)
                }
            })
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::Duration;

    fn spawn_sleep_child() -> std::process::Child {
        // process_group(0) makes the child a group leader; child.id() == pgid.
        Command::new("sleep")
            .arg("5")
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep")
    }

    #[test]
    fn register_and_lookup_roundtrip() {
        let mut child = spawn_sleep_child();
        let pid = child.id() as i32;
        super::register("run-1", pid);
        assert_eq!(super::pgid("run-1"), Some(pid));
        super::unregister("run-1");
        assert_eq!(super::pgid("run-1"), None);
        let _ = super::signal_group(pid, super::SIGKILL);
        let _ = child.wait();
    }

    #[test]
    fn sigstop_freezes_group_and_sigcont_resumes() {
        let mut child = spawn_sleep_child();
        let pgid = child.id() as i32;
        // STOP the group; the child must NOT exit while stopped.
        super::signal_group(pgid, super::SIGSTOP).expect("sigstop");
        thread::sleep(Duration::from_millis(300));
        assert!(
            child.try_wait().expect("try_wait").is_none(),
            "stopped child must not exit"
        );
        // CONT the group; the child exits shortly after (sleep 5 → kill).
        super::signal_group(pgid, super::SIGCONT).expect("sigcont");
        super::signal_group(pgid, super::SIGKILL).expect("sigkill");
        child.wait().expect("wait");
    }

    /// A process whose command line merely MENTIONS "oxo-flow" (an editor
    /// session, `grep -r oxo-flow`, a forged argv[0]) must fail the
    /// identity probe — the guard is anchored, not a bare substring match.
    #[test]
    fn looks_like_oxo_flow_rejects_substring_mentions() {
        let mut child = Command::new("sh")
            .args(["-c", "exec -a 'grep -r oxo-flow .' sleep 5"])
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn forged-argv child");
        let pid = child.id() as i32;
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !super::looks_like_oxo_flow(pid),
            "a substring mention of oxo-flow must not pass the identity probe"
        );
        let _ = super::signal_group(pid, super::SIGKILL);
        let _ = child.wait();
    }

    /// The executor's wrapper composition (`sh -c <script> sh <exitfile>
    /// <binary> run …`) must pass the identity probe even though its first
    /// argv element is `sh` — the binary token follows the exit record.
    #[test]
    fn looks_like_oxo_flow_accepts_executor_wrapper_shape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exit_file = dir.path().join(".exit-code");
        // Stand-in script body that only sleeps — the binary path need not
        // exist; `ps` sees the command line either way.
        let mut child = Command::new("sh")
            .args(["-c", "f=\"$1\"; shift; sleep 5"])
            .arg("sh")
            .arg(&exit_file)
            .arg("/opt/oxo/bin/oxo-flow")
            .arg("run")
            .arg("wf.oxoflow")
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn wrapper-shaped child");
        let pid = child.id() as i32;
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            super::looks_like_oxo_flow(pid),
            "the executor wrapper shape must pass the identity probe"
        );
        let _ = super::signal_group(pid, super::SIGKILL);
        let _ = child.wait();
    }

    /// The identity guard is anchored on the command line's STRUCTURE:
    /// substring presence decides nothing.
    #[test]
    fn cmdline_identity_is_anchored_not_substring() {
        // Mentions without structure must fail.
        assert!(!super::cmdline_is_oxo_flow("grep -r oxo-flow ."));
        assert!(!super::cmdline_is_oxo_flow(
            "vim /tmp/oxo-flow-analysis.txt"
        ));
        assert!(!super::cmdline_is_oxo_flow("sh -c oxo-flow"));
        assert!(!super::cmdline_is_oxo_flow(""));
        // An exit-record path WITHOUT the binary token must fail.
        assert!(!super::cmdline_is_oxo_flow(
            "sh -c 'echo hi' sh /tmp/run/.exit-code echo hi"
        ));
        // A binary whose name merely CONTAINS "oxo-flow" must fail.
        assert!(!super::cmdline_is_oxo_flow(
            "sh -c x sh /tmp/run/.exit-code /usr/bin/oxo-flow-bench run wf"
        ));
        // The CLI itself: binary path or bare name as the FIRST argv element.
        assert!(super::cmdline_is_oxo_flow(
            "/usr/local/bin/oxo-flow run wf.oxoflow --workdir /tmp/run"
        ));
        assert!(super::cmdline_is_oxo_flow("oxo-flow run wf.oxoflow"));
        // The executor wrapper: exit record directly before the binary…
        assert!(super::cmdline_is_oxo_flow(
            "sh -c f=\"$1\"; shift; \"$@\"; rc=$?; printf '%s' \"$rc\" > \"$f\"; exit \"$rc\" sh /tmp/run/.exit-code /usr/local/bin/oxo-flow run wf.oxoflow --workdir /tmp/run"
        ));
        // …including the sudo prefix (`sudo -n -u <user>` between them).
        assert!(super::cmdline_is_oxo_flow(
            "sh -c f=\"$1\"; shift; \"$@\"; rc=$?; printf '%s' \"$rc\" > \"$f\"; exit \"$rc\" sh /tmp/run/.exit-code sudo -n -u bioinfo /usr/local/bin/oxo-flow run wf.oxoflow"
        ));
    }

    /// A group whose only member is a plain `sleep` must fail the group
    /// probe; a group with an oxo-flow-shaped member passes (the shape a
    /// reaped wrapper leader leaves behind: orphaned CLI, no leader). The
    /// CLI-shaped member is its OWN group leader — joining an existing
    /// leader's group from the test races `setpgid` on macOS, and the
    /// reaped-leader scenario is covered end-to-end by the integration test.
    #[test]
    fn group_identity_scans_surviving_members() {
        let mut member = Command::new("sh")
            .args(["-c", "f=\"$1\"; shift; sleep 5"])
            .arg("sh")
            .arg("/tmp/run/.exit-code")
            .arg("/opt/oxo/bin/oxo-flow")
            .arg("run")
            .arg("wf.oxoflow")
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn CLI-shaped group member");
        let pgid = member.id() as i32;
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            super::group_looks_like_oxo_flow(pgid),
            "a group with an oxo-flow-shaped member must pass"
        );
        // A group of unrelated processes must fail the group probe.
        let mut plain = spawn_sleep_child();
        assert!(!super::group_looks_like_oxo_flow(plain.id() as i32));
        let _ = super::signal_group(pgid, super::SIGKILL);
        let _ = super::signal_group(plain.id() as i32, super::SIGKILL);
        let _ = member.wait();
        let _ = plain.wait();
    }

    #[test]
    fn probe_alive_tracks_process_lifecycle() {
        let mut child = spawn_sleep_child();
        let pid = child.id() as i32;
        assert!(super::probe_alive(pid), "live child must probe alive");
        super::signal_group(pid, super::SIGKILL).expect("kill");
        let _ = child.wait();
        // Give the reaper a moment; then the pid must probe dead.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while super::probe_alive(pid) && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(
            !super::probe_alive(pid),
            "killed-and-reaped child must probe dead"
        );
    }
}
