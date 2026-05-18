/// Kill a process and all its children by terminating the process group.
/// Only available on Unix systems.
#[cfg(unix)]
pub fn kill_process_tree(pid: u32) -> std::io::Result<()> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::{Pid, getpgid};

    let nix_pid = Pid::from_raw(pid as i32);
    let pgid = getpgid(Some(nix_pid)).map_err(|e| std::io::Error::other(e.to_string()))?;

    // Kill entire process group with SIGKILL
    kill(pgid, Signal::SIGKILL).map_err(|e| std::io::Error::other(e.to_string()))?;

    tracing::debug!(pid = %pid, pgid = %pgid, "killed process group");
    Ok(())
}

/// Stub for non-Unix systems (no process group support).
#[cfg(not(unix))]
pub fn kill_process_tree(_pid: u32) -> std::io::Result<()> {
    // On non-Unix, we rely on the normal timeout behavior
    Ok(())
}
