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
        .expect("registry poisoned")
        .insert(run_id.to_string(), pgid);
}

/// Drop tracking after the subprocess has exited.
pub fn unregister(run_id: &str) {
    registry()
        .write()
        .expect("registry poisoned")
        .remove(run_id);
}

/// Current process group id for `run_id`, if the subprocess is still tracked.
pub fn pgid(run_id: &str) -> Option<i32> {
    registry()
        .read()
        .expect("registry poisoned")
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

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::Duration;

    fn spawn_sleep_child() -> std::process::Child {
        // process_group(0) makes the child a group leader; child.id() == pgid.
        use std::os::unix::process::CommandExt;
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
}
