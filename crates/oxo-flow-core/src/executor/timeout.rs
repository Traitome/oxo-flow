/// Kill a process and all its descendants (deepest first).
///
/// Rules deliberately run inside the caller's process group (see
/// `process::execute_rule` — "one run = one process group"), so a timeout
/// cannot kill by process group: the group is shared with the whole run.
/// Instead the parent→child links are snapshotted via sysinfo and every
/// descendant is signaled individually, which also reaches grandchildren
/// that detached from the group. Only available on Unix systems.
#[cfg(unix)]
pub fn kill_process_tree(pid: u32) -> std::io::Result<()> {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let root = Pid::from_raw(pid as i32);

    // Snapshot parent→child links, then walk outward from the root pid.
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

    // Deepest first: children die before their parents so no subtree can be
    // re-parented into init and survive the kill.
    for p in targets.iter().rev() {
        match kill(*p, Signal::SIGKILL) {
            Ok(()) => {}
            // Already exited between the snapshot and the signal — fine.
            Err(Errno::ESRCH) => {}
            Err(e) => return Err(std::io::Error::other(e.to_string())),
        }
    }

    tracing::debug!(pid = pid, killed = targets.len(), "killed process subtree");
    Ok(())
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
