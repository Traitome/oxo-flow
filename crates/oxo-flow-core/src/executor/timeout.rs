/// Grace period between SIGTERM and SIGKILL for subtree kills (issue #194
/// A4): long-running tools (aligners, databases) get a chance to flush
/// state and exit cleanly before the engine escalates.
pub const SIGTERM_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

/// Kill a process and all its descendants (deepest first), gracefully:
/// SIGTERM the whole subtree, poll [`SIGTERM_GRACE`] for survivors,
/// re-scan for descendants spawned during the window, then SIGKILL
/// whatever remains.
///
/// Rules deliberately run inside the caller's process group (see
/// `process::execute_rule` — "one run = one process group"), so a timeout
/// cannot kill by process group: the group is shared with the whole run.
/// Instead the parent→child links are snapshotted via sysinfo and every
/// descendant is signaled individually, which also reaches grandchildren
/// that detached from the group. Only available on Unix systems.
#[cfg(unix)]
pub fn kill_process_tree(pid: u32) -> std::io::Result<()> {
    kill_process_tree_with_grace(pid, SIGTERM_GRACE)
}

/// [`kill_process_tree`] with an explicit grace window (tests use a short
/// one; production passes [`SIGTERM_GRACE`]).
#[cfg(unix)]
pub fn kill_process_tree_with_grace(pid: u32, grace: std::time::Duration) -> std::io::Result<()> {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let root = Pid::from_raw(pid as i32);
    let mut targets = collect_descendants(root);

    // Deepest first: children die before their parents so no subtree can be
    // re-parented into init and survive the kill.
    let signal_subtree = |sig: Signal, targets: &[Pid]| -> std::io::Result<()> {
        for p in targets.iter().rev() {
            match kill(*p, sig) {
                Ok(()) => {}
                // Already exited between the snapshot and the signal — fine.
                Err(Errno::ESRCH) => {}
                Err(e) => return Err(std::io::Error::other(e.to_string())),
            }
        }
        Ok(())
    };
    signal_subtree(Signal::SIGTERM, &targets)?;

    // Poll the grace window; a process that honors TERM gets to flush.
    let deadline = std::time::Instant::now() + grace;
    loop {
        let alive = {
            let mut s = sysinfo::System::new();
            s.refresh_processes(sysinfo::ProcessesToUpdate::All, false);
            targets.iter().any(|p| {
                s.process(sysinfo::Pid::from_u32(p.as_raw() as u32))
                    .is_some()
            })
        };
        if !alive {
            tracing::debug!(
                pid = pid,
                targets = targets.len(),
                "process subtree exited on SIGTERM"
            );
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // Re-snapshot before escalating (issue #194 §3.4): a descendant
    // spawned after the first walk — whose spawner survived the TERM sweep
    // and is therefore still reachable through live parent links — missed
    // the TERM signal; it must not miss the KILL sweep too. Descendants
    // whose spawner died and re-parented into init are unreachable by
    // design (the documented limit of parent-chain killing).
    let refreshed = collect_descendants(root);
    for p in refreshed {
        if !targets.contains(&p) {
            targets.push(p);
        }
    }

    tracing::debug!(
        pid = pid,
        targets = targets.len(),
        "process subtree survived SIGTERM grace; escalating to SIGKILL"
    );
    signal_subtree(Signal::SIGKILL, &targets)
}

/// Snapshot parent→child links and walk outward from `root` to collect
/// every descendant pid (including `root` itself).
#[cfg(unix)]
fn collect_descendants(root: nix::unistd::Pid) -> Vec<nix::unistd::Pid> {
    use nix::unistd::Pid;
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let mut targets: Vec<Pid> = vec![root];
    loop {
        let mut found = false;
        for (child_pid, proc) in system.processes() {
            let child = Pid::from_raw(child_pid.as_u32() as i32);
            if targets.contains(&child) {
                continue;
            }
            if let Some(parent) = proc.parent() {
                let parent = Pid::from_raw(parent.as_u32() as i32);
                if targets.contains(&parent) {
                    targets.push(child);
                    found = true;
                }
            }
        }
        if !found {
            break;
        }
    }
    targets
}

/// Stub for non-Unix systems (no process group support).
#[cfg(not(unix))]
pub fn kill_process_tree(_pid: u32) -> std::io::Result<()> {
    // On non-Unix, we rely on the normal timeout behavior
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::Duration;

    use super::*;

    /// Spawn `sh -c 'sleep 30 & echo $! > <pidfile>; wait'` and return the
    /// shell pid and the grandchild pid. Neither process gets its own group,
    /// so the group-killing strategy this replaced would have hit the test
    /// runner's group instead of the subtree.
    fn spawn_sh_with_background_child(pidfile: &std::path::Path) -> std::process::Child {
        let script = format!("sleep 30 & echo $! > {}; wait", pidfile.to_string_lossy());
        Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sh")
    }

    fn process_exists(pid: u32) -> bool {
        use nix::sys::signal::kill;
        use nix::unistd::Pid;
        kill(Pid::from_raw(pid as i32), None).is_ok()
    }

    #[test]
    fn kill_process_tree_kills_descendants_but_not_the_group() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pidfile = dir.path().join("child.pid");

        let mut sh = spawn_sh_with_background_child(&pidfile);
        let sh_pid = sh.id();

        // Wait for the pid file to appear (grandchild started).
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let grandchild_pid = loop {
            if let Ok(content) = std::fs::read_to_string(&pidfile) {
                break content.trim().parse::<u32>().expect("valid pid");
            }
            assert!(
                std::time::Instant::now() < deadline,
                "grandchild pid file never appeared"
            );
            thread::sleep(Duration::from_millis(50));
        };

        kill_process_tree(sh_pid).expect("tree kill");

        // Both the shell and its background child are gone…
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while process_exists(grandchild_pid) && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(
            !process_exists(grandchild_pid),
            "grandchild survived the tree kill"
        );
        let _ = sh.wait();

        // …while the caller (this test binary) is untouched — the old
        // group-kill strategy would have killed us here.
        assert!(process_exists(std::process::id()));
    }

    #[test]
    fn kill_process_tree_delivers_sigterm_before_escalating() {
        // issue #194 A4: a TERM-honoring child must observe SIGTERM (and get
        // its cleanup chance) instead of being SIGKILLed outright.
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("got-term");
        let pidfile = dir.path().join("child.pid");
        let script = format!(
            "trap 'echo term > {marker}; exit 0' TERM; echo $$ > {pidfile}; sleep 30",
            marker = marker.display(),
            pidfile = pidfile.display(),
        );
        let mut sh = Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sh");
        let sh_pid = sh.id();

        // Wait for the shell to record its pid.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !pidfile.exists() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(pidfile.exists(), "child pid file never appeared");

        kill_process_tree(sh_pid).expect("tree kill");
        let _ = sh.wait();

        assert!(
            marker.exists(),
            "the TERM trap never ran — the subtree was killed without grace"
        );
        assert!(!process_exists(sh_pid), "shell survived the tree kill");
    }

    #[test]
    fn kill_process_tree_reaches_descendants_spawned_during_the_grace_window() {
        // issue #194 §3.4: a grandchild spawned AFTER the initial
        // snapshot, whose spawner ignores SIGTERM and is still alive at
        // escalation time, must be caught by the re-scan before SIGKILL.
        // The old single-snapshot walk let it survive.
        let dir = tempfile::tempdir().expect("tempdir");
        let spawner_pidfile = dir.path().join("spawner.pid");
        let grandchild_pidfile = dir.path().join("grandchild.pid");
        let script = format!(
            "trap '' TERM; (trap '' TERM; echo $$ > {spawner}; sleep 0.5; sleep 30 & echo $! > {grandchild}; wait) & wait",
            spawner = spawner_pidfile.display(),
            grandchild = grandchild_pidfile.display(),
        );
        let mut sh = Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sh");
        let sh_pid = sh.id();

        // Wait for the TERM-ignoring spawner subshell to record its pid.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !spawner_pidfile.exists() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(spawner_pidfile.exists(), "spawner pid file never appeared");

        // Grace of 1.2s: the grandchild spawns at +0.5s, inside the window.
        kill_process_tree_with_grace(sh_pid, Duration::from_millis(1200))
            .expect("tree kill with grace");

        // The shell and spawner are gone…
        let _ = sh.wait();
        assert!(!process_exists(sh_pid), "shell survived the tree kill");

        // …and the late grandchild was caught by the re-scan.
        let grandchild_pid = std::fs::read_to_string(&grandchild_pidfile)
            .expect("grandchild pid file")
            .trim()
            .parse::<u32>()
            .expect("valid pid");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while process_exists(grandchild_pid) && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(
            !process_exists(grandchild_pid),
            "grandchild spawned during the grace window survived the kill"
        );
    }

    #[test]
    fn kill_process_tree_tolerates_already_dead_pid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pidfile = dir.path().join("child.pid");
        let mut sh = spawn_sh_with_background_child(&pidfile);
        let sh_pid = sh.id();
        // Kill the shell first; its child may linger briefly as an orphan.
        use nix::sys::signal::{Signal, kill};
        use nix::unistd::Pid;
        kill(Pid::from_raw(sh_pid as i32), Signal::SIGKILL).expect("kill sh");
        let _ = sh.wait();
        // The pid is now dead (or the grandchild re-parented) — the call must
        // not error out (ESRCH is tolerated).
        let _ = kill_process_tree(sh_pid);
        // Hygiene: reap the orphaned grandchild so it does not outlive the test.
        if let Ok(content) = std::fs::read_to_string(&pidfile)
            && let Ok(grandchild) = content.trim().parse::<u32>()
        {
            let _ = kill(Pid::from_raw(grandchild as i32), Signal::SIGKILL);
        }
    }
}
