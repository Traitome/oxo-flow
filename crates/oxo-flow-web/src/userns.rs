//! Opt-in per-run user + PID namespace isolation for LOCAL web-spawned runs
//! (RB-01 residual / issue #1401).
//!
//! This is the layer above [`crate::sandbox`]: where Landlock confines the
//! filesystem/network an already-started process may touch, this puts the
//! spawned CLI into a fresh **user namespace** (the server uid maps to uid 0
//! inside, so files created on the run workdir keep the server uid outside)
//! and a fresh **PID namespace** (the CLI becomes namespace init, reaps its
//! rules, and a private `/proc` hides host processes; the run cannot see or
//! signal host or sibling-tenant processes). It is a command wrapper —
//! `unshare` from util-linux — not unsafe in-process code. The boundary is
//! one-directional: host-side signals still reach the namespace (an ancestor
//! PID namespace can signal any descendant), which is exactly how cancel
//! works.
//!
//! ## Contract
//!
//! - `OXO_FLOW_WEB_ISOLATION` unset / empty / `off` = disabled (the default;
//!   behavior is byte-identical to prior releases).
//! - `userns` = requested. Availability is PROBED with the exact wrapper
//!   argv before use (as the server uid); a successful probe is cached for
//!   the process lifetime and a failed probe is retried on the next spawn.
//!   A requested-but-unavailable mode FAILS THE SPAWN with a structured
//!   message naming the host prerequisite — it never silently degrades to
//!   an unisolated run.
//! - Any other value (including a non-UTF-8 value) is a configuration error
//!   and also fails the spawn.
//! - Local-only: cluster/remote runs execute on another host and are not
//!   affected (this module is only consulted by the local spawn path).
//!
//! ## Cancellation
//!
//! The executor spawns the whole wrapper in its own process group, so the
//! supervisor's `killpg` reaches `sh`, `unshare`, and the namespace.
//! `--kill-child=SIGTERM` makes the `unshare` parent forward SIGTERM to the
//! namespace init when the parent is terminated, giving a CLI that traps
//! SIGTERM a graceful shutdown window. A PID namespace's init (PID 1)
//! ignores catchable signals it has no handler for, so for a CLI that does
//! not trap SIGTERM the supervisor's existing 5 s SIGKILL escalation is the
//! guaranteed path: it kills init directly, and the kernel tears down every
//! remaining member of the PID namespace. SIGKILL and SIGSTOP (the final
//! kill and the pause/resume signals) are kernel-only and are delivered to
//! namespace init directly.
//!
//! ## Composition
//!
//! RLIMITs are installed in `pre_exec` on the wrapper and inherited by
//! `unshare`, the CLI, and every rule shell — unchanged. `OXO_FLOW_WEB_SANDBOX
//! =landlock` is refused together with `userns` (fail-closed). This was
//! measured, not assumed (tx-ubuntu, kernel with Landlock ABI 4,
//! util-linux 2.39.3): granting read+write on `/proc/self/uid_map`,
//! `/proc/self/gid_map`, and `/proc/self/setgroups` — as individual file
//! rules or as a rule on the whole `/proc/self` directory — does NOT let
//! `unshare` initialize. The open still returns `EACCES`:
//!
//! - with `--fork` (required for the PID namespace) util-linux clones first
//!   and the CHILD opens its own `/proc/self/uid_map`; the child's host pid
//!   is assigned at runtime, so no ruleset built before exec can name that
//!   inode (strace: `clone(...) = <pid>` then that pid's
//!   `openat("/proc/self/uid_map", O_WRONLY) = -1 EACCES`);
//! - even the no-fork self-mapping shape is denied after exec: a Landlock
//!   path rule resolved on the pre-exec process's proc entry does not cover
//!   the post-exec open of the dynamic pid entry.
//!
//! Only a grant on host `/proc` (or a dynamically learned per-pid path,
//! which Landlock cannot express) would cover the write, and that would
//! re-expose every process on the host — the exact disclosure the
//! environment allowlist and `/proc` denial exist to prevent. Composition is
//! therefore refused; operators run one enforcement mode at a time.
//!
//! ## Residuals (documented, not claimed as covered)
//!
//! - The mapping is single-uid: the run is root *inside* its namespace but
//!   the same server uid *outside*. Per-run dedicated uids need subordinate
//!   ID ranges and are out of scope.
//! - `RLIMIT_NPROC` is counted per real uid and is not partitioned by
//!   namespaces; a namespace does not give each tenant its own process
//!   budget.
//! - UDP/ICMP and the network in general are not restricted by namespaces.
//! - Hosts may disable unprivileged user namespaces (Ubuntu 24.04's AppArmor
//!   restriction, Debian's sysctl); the probe reports the remediation and
//!   the spawn fails closed.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Operator knob (issue #1401): selects per-run user + PID namespace
/// isolation for local web-spawned runs. Unset / empty / `off` = disabled;
/// `userns` = requested; anything else is an error that fails the spawn.
pub const ISOLATION_MODE_ENV: &str = "OXO_FLOW_WEB_ISOLATION";

/// The util-linux program that installs the namespaces.
const UNSHARE_PROGRAM: &str = "unshare";

/// Namespace flags, in argv order. `--kill-child=SIGTERM` makes the unshare
/// parent forward SIGTERM to the namespace init when the parent is
/// terminated: a CLI that traps SIGTERM can shut down gracefully, and one
/// that does not is covered by the supervisor's SIGKILL escalation (PID 1
/// ignores catchable signals without a handler). `--` ends option parsing
/// before the wrapped command and is part of the probed argv too.
const UNSHARE_FLAGS: &[&str] = &[
    "--user",
    "--map-root-user",
    "--pid",
    "--fork",
    "--mount-proc",
    "--kill-child=SIGTERM",
    "--",
];

/// Bound on the availability probe: `unshare … true` exits immediately when
/// the capability exists, so anything slower is a host problem. Kept short
/// because the probe blocks a spawn-path worker.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Poll interval while waiting for the probe child.
const PROBE_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Parsed isolation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationMode {
    /// Disabled (the default).
    Off,
    /// User + PID namespace wrapper enabled.
    UserNamespace,
}

/// Parse `OXO_FLOW_WEB_ISOLATION`.
///
/// Fail-closed: an unrecognized value is an error, so a typo can never
/// silently disable the isolation the operator asked for.
pub fn parse_isolation_mode(raw: Option<&str>) -> Result<IsolationMode, String> {
    match raw.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(IsolationMode::Off),
        Some(v) if v.eq_ignore_ascii_case("off") => Ok(IsolationMode::Off),
        Some(v) if v.eq_ignore_ascii_case("userns") => Ok(IsolationMode::UserNamespace),
        Some(other) => Err(format!(
            "{ISOLATION_MODE_ENV}={other:?} is not a valid isolation mode; \
             accepted values: off, userns"
        )),
    }
}

/// A resolved namespace-isolation plan: the `unshare` argv prefix to place
/// before the CLI payload. Holding a plan means the mode was requested AND
/// the probe succeeded, so the spawn may proceed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsernsPlan {
    program: PathBuf,
}

impl UsernsPlan {
    /// Wrap a payload (the `sudo` prefix, when applicable, plus the CLI and
    /// its arguments) in the namespace entry command.
    ///
    /// The result replaces the payload inside the existing
    /// `sh -c <exit-code wrapper> sh <exit-file> <payload…>` composition, so
    /// the CLI's exit status is still persisted to `.exit-code`: `unshare`
    /// waits for its child and exits with the child's status.
    pub fn wrap_payload(&self, payload: Vec<OsString>) -> Vec<OsString> {
        let mut wrapped = Vec::with_capacity(1 + UNSHARE_FLAGS.len() + payload.len());
        wrapped.push(self.program.clone().into_os_string());
        wrapped.extend(UNSHARE_FLAGS.iter().map(OsString::from));
        wrapped.extend(payload);
        wrapped
    }

    /// The EXACT probe argv — every wrapper flag including the `--`
    /// separator, with `true` as the payload. The probe must exercise the
    /// same command shape the spawn uses, so a version of `unshare` that
    /// rejects `--kill-child=SIGTERM` fails the probe instead of every run.
    fn probe_argv(&self) -> impl Iterator<Item = &str> {
        std::iter::once(UNSHARE_PROGRAM).chain(UNSHARE_FLAGS.iter().copied())
    }
}

/// Resolve namespace isolation for one local spawn from the server
/// environment.
///
/// `Ok(None)` = disabled. Every `Err` is fail-closed: the caller must not
/// spawn.
pub fn configure(
    mode_raw: Option<&str>,
    sudo_mode: bool,
    landlock_enabled: bool,
) -> Result<Option<UsernsPlan>, String> {
    configure_with(mode_raw, sudo_mode, landlock_enabled, cached_availability)
}

/// Read [`ISOLATION_MODE_ENV`] and resolve the plan.
///
/// Unlike a `.ok()` read, a value that is not valid UTF-8 is a configuration
/// error (fail-closed): treating it as unset would silently disable the
/// isolation the operator requested.
pub fn configure_env(
    sudo_mode: bool,
    landlock_enabled: bool,
) -> Result<Option<UsernsPlan>, String> {
    let raw = utf8_mode_value(std::env::var_os(ISOLATION_MODE_ENV))?;
    configure(raw.as_deref(), sudo_mode, landlock_enabled)
}

/// Fail-closed UTF-8 conversion for the knob value: a non-UTF-8 value is an
/// error, never "unset".
fn utf8_mode_value(value: Option<OsString>) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(value) => value.into_string().map(Some).map_err(|_| {
            format!("{ISOLATION_MODE_ENV} is not valid UTF-8; accepted values: off, userns")
        }),
    }
}

/// The probe-injectable core of [`configure`], so unit tests can drive an
/// available/unavailable host without touching the real kernel or mutating
/// process-global state.
fn configure_with(
    mode_raw: Option<&str>,
    sudo_mode: bool,
    landlock_enabled: bool,
    probe: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<Option<UsernsPlan>, String> {
    if parse_isolation_mode(mode_raw)? == IsolationMode::Off {
        return Ok(None);
    }
    // A sudo-prefixed payload needs a setuid transition (and one uid per run
    // user); inside a single-uid namespace the only mapped id is root, so
    // sudo cannot work. Reject the combination instead of failing opaquely.
    if sudo_mode {
        return Err(format!(
            "{ISOLATION_MODE_ENV}=userns cannot be combined with the legacy sudo spawn \
             payload (auth_type=sudo): the namespace maps a single uid"
        ));
    }
    if landlock_enabled {
        // Measured on tx-ubuntu (Landlock ABI 4, util-linux 2.39.3): even
        // granting /proc/self/{uid_map,gid_map,setgroups}, the unshare map
        // write returns EACCES — with --fork the write happens in a child
        // whose host pid is assigned at runtime, so no pre-exec static rule
        // can name it; a host-/proc grant would re-expose every process.
        // Refuse rather than weakening either mode.
        return Err(format!(
            "{ISOLATION_MODE_ENV}=userns cannot be combined with \
             OXO_FLOW_WEB_SANDBOX=landlock: unshare writes uid_map/gid_map/setgroups \
             through the dynamic /proc/<pid> of a forked child, which a pre-exec \
             Landlock ruleset cannot grant, and granting host /proc would re-expose \
             every process. Run one mode at a time."
        ));
    }
    let program = PathBuf::from(UNSHARE_PROGRAM);
    probe(&program).map_err(|detail| {
        format!(
            "{ISOLATION_MODE_ENV}=userns requested but unprivileged user namespaces are \
             unavailable on this host: {detail}. Host remediation: on Ubuntu 24.04 grant \
             `userns,` to the server binary in an AppArmor profile (or set \
             kernel.apparmor_restrict_unprivileged_userns=0); ensure \
             kernel.unprivileged_userns_clone=1 and that util-linux `unshare` is installed. \
             Refusing to spawn without the requested isolation (fail-closed)."
        )
    })?;
    Ok(Some(UsernsPlan { program }))
}

/// Process-lifetime cache of a SUCCESSFUL availability probe: the capability
/// cannot disappear within one server process without a restart, and probing
/// per run would add a fork to every spawn. A FAILED probe is deliberately
/// not cached — a transient load failure (fork pressure, a temporarily
/// unavailable helper) must not disable the mode until the server restarts;
/// the next spawn probes again and still fails closed if the host is really
/// incapable.
fn cached_availability(program: &Path) -> Result<(), String> {
    static AVAILABLE: OnceLock<()> = OnceLock::new();
    if AVAILABLE.get().is_some() {
        return Ok(());
    }
    probe_unshare(program)?;
    let _ = AVAILABLE.set(());
    Ok(())
}

/// Run the exact wrapper argv with `true` as the payload, bounded by
/// [`PROBE_TIMEOUT`]. Success requires a zero exit status — anything else
/// (missing binary, EPERM from the kernel/AppArmor, mount-proc failure,
/// timeout) is reported with the child's stderr for the remediation hint.
fn probe_unshare(program: &Path) -> Result<(), String> {
    let plan = UsernsPlan {
        program: program.to_path_buf(),
    };
    let mut cmd = std::process::Command::new(&plan.program);
    cmd.args(plan.probe_argv().skip(1))
        .arg("true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        format!(
            "could not execute {:?}: {e}",
            plan.program.as_os_str().to_string_lossy()
        )
    })?;
    let status = wait_with_timeout(&mut child, PROBE_TIMEOUT)?;
    if status.success() {
        return Ok(());
    }
    let stderr = child
        .stderr
        .take()
        .map(|mut pipe| {
            use std::io::Read;
            let mut buf = String::new();
            let _ = pipe.read_to_string(&mut buf);
            buf
        })
        .unwrap_or_default();
    let detail = stderr.trim();
    Err(if detail.is_empty() {
        format!("unshare probe exited with {status}")
    } else {
        format!("unshare probe exited with {status}: {detail}")
    })
}

/// Wait for `child` with a deadline; kill it on expiry and report the
/// timeout. The probe must never wedge the spawn path.
fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(PROBE_POLL_INTERVAL);
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "unshare probe did not finish within {} s",
                    timeout.as_secs()
                ));
            }
            Err(e) => return Err(format!("failed waiting for unshare probe: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_probe(_program: &Path) -> Result<(), String> {
        Ok(())
    }

    fn failing_probe(_program: &Path) -> Result<(), String> {
        Err("simulated EPERM".to_string())
    }

    #[test]
    fn mode_parsing_defaults_to_off() {
        assert_eq!(parse_isolation_mode(None).unwrap(), IsolationMode::Off);
        assert_eq!(parse_isolation_mode(Some("")).unwrap(), IsolationMode::Off);
        assert_eq!(
            parse_isolation_mode(Some("  ")).unwrap(),
            IsolationMode::Off
        );
        assert_eq!(
            parse_isolation_mode(Some("off")).unwrap(),
            IsolationMode::Off
        );
        assert_eq!(
            parse_isolation_mode(Some(" OFF ")).unwrap(),
            IsolationMode::Off
        );
    }

    #[test]
    fn mode_parsing_enables_userns() {
        assert_eq!(
            parse_isolation_mode(Some("userns")).unwrap(),
            IsolationMode::UserNamespace
        );
        assert_eq!(
            parse_isolation_mode(Some("UserNS")).unwrap(),
            IsolationMode::UserNamespace
        );
    }

    #[test]
    fn mode_parsing_fails_closed_on_garbage() {
        for raw in ["landlock", "1", "true", "user-ns", "usernsx"] {
            let err =
                parse_isolation_mode(Some(raw)).expect_err("garbage mode must not be accepted");
            assert!(
                err.contains(ISOLATION_MODE_ENV) && err.contains("off, userns"),
                "error must name the knob and accepted values: {err}"
            );
        }
    }

    #[test]
    fn configure_off_never_probes() {
        let mut probed = false;
        let plan = configure_with(Some("off"), false, false, |_| {
            probed = true;
            Ok(())
        })
        .expect("off is the default");
        assert!(plan.is_none());
        assert!(!probed, "disabled mode must not probe the host");
    }

    #[test]
    fn configure_fails_closed_on_garbage_mode() {
        let err = configure_with(Some("nonsense"), false, false, ok_probe)
            .expect_err("garbage mode must refuse the spawn");
        assert!(err.contains("not a valid isolation mode"), "{err}");
    }

    #[test]
    fn configure_fails_closed_when_probe_fails() {
        let err = configure_with(Some("userns"), false, false, failing_probe)
            .expect_err("unavailable host must refuse the spawn");
        assert!(err.contains("simulated EPERM"), "{err}");
        assert!(
            err.contains("fail-closed") && err.contains("AppArmor"),
            "error must state fail-closed and name the remediation: {err}"
        );
    }

    #[test]
    fn configure_succeeds_when_probe_succeeds() {
        let plan = configure_with(Some("userns"), false, false, ok_probe)
            .expect("probe success")
            .expect("mode requested");
        assert_eq!(plan.program, PathBuf::from(UNSHARE_PROGRAM));
    }

    #[test]
    fn configure_rejects_sudo_payload() {
        let err = configure_with(Some("userns"), true, false, ok_probe)
            .expect_err("sudo + userns must fail closed");
        assert!(err.contains("auth_type=sudo"), "{err}");
    }

    #[test]
    fn configure_rejects_landlock_combination() {
        let err = configure_with(Some("userns"), false, true, ok_probe)
            .expect_err("landlock + userns must fail closed");
        assert!(err.contains("OXO_FLOW_WEB_SANDBOX=landlock"), "{err}");
        assert!(err.contains("one mode at a time"), "{err}");
    }

    #[test]
    fn wrapper_argv_shape_is_exact() {
        let plan = UsernsPlan {
            program: PathBuf::from("unshare"),
        };
        let payload = vec![
            OsString::from("/opt/oxo/bin/oxo-flow"),
            OsString::from("run"),
            OsString::from("wf.oxoflow"),
        ];
        let wrapped = plan.wrap_payload(payload.clone());
        let expected: Vec<OsString> = [
            "unshare",
            "--user",
            "--map-root-user",
            "--pid",
            "--fork",
            "--mount-proc",
            "--kill-child=SIGTERM",
            "--",
        ]
        .into_iter()
        .map(OsString::from)
        .chain(payload)
        .collect();
        assert_eq!(wrapped, expected);
    }

    /// The probe must exercise the EXACT wrapper shape, including the `--`
    /// separator, so an `unshare` that rejects any of the flags fails the
    /// probe instead of every run.
    #[test]
    fn probe_argv_matches_the_wrapper_shape() {
        let plan = UsernsPlan {
            program: PathBuf::from("unshare"),
        };
        let probe: Vec<String> = plan.probe_argv().map(str::to_string).collect();
        let wrapped: Vec<OsString> = plan.wrap_payload(vec![OsString::from("payload")]);
        let wrapper_prefix: Vec<String> = wrapped[..wrapped.len() - 1]
            .iter()
            .map(|v| v.to_string_lossy().into_owned())
            .collect();
        assert_eq!(probe, wrapper_prefix);
        assert_eq!(probe.last().map(String::as_str), Some("--"));
        assert!(probe.iter().any(|f| f == "--kill-child=SIGTERM"));
    }

    /// A transient probe failure must not disable the mode for the process
    /// lifetime: only success is cached, failures are retried.
    #[test]
    fn failed_probes_are_not_cached() {
        let missing = Path::new("/nonexistent/definitely-not-unshare");
        assert!(cached_availability(missing).is_err());
        assert!(
            cached_availability(missing).is_err(),
            "a failed probe must be retried, not cached as a permanent verdict"
        );
    }

    #[test]
    #[cfg(unix)]
    fn non_utf8_knob_value_fails_closed() {
        use std::os::unix::ffi::OsStringExt;
        let raw = OsString::from_vec(vec![0xff, 0xfe]);
        let err = utf8_mode_value(Some(raw)).expect_err("non-UTF-8 must fail closed");
        assert!(err.contains("not valid UTF-8"), "{err}");
        assert!(utf8_mode_value(None).unwrap().is_none());
        assert_eq!(
            utf8_mode_value(Some(OsString::from("userns"))).unwrap(),
            Some("userns".to_string())
        );
    }

    #[test]
    fn probe_with_missing_program_reports_spawn_failure() {
        let err = probe_unshare(Path::new("/nonexistent/definitely-not-unshare"))
            .expect_err("missing program must fail the probe");
        assert!(err.contains("could not execute"), "{err}");
    }

    /// On a Linux host where `unshare` exists, the probe runs the REAL
    /// binary. A standalone `true` stand-in (any program that exits 0 and
    /// ignores arguments) must probe usable; `false` must probe unusable.
    #[test]
    #[cfg(target_os = "linux")]
    fn probe_with_real_program_exit_status_is_authoritative() {
        if which("true") && which("false") {
            assert!(probe_unshare(Path::new("true")).is_ok());
            assert!(probe_unshare(Path::new("false")).is_err());
        }
    }

    #[cfg(target_os = "linux")]
    fn which(name: &str) -> bool {
        std::env::var_os("PATH")
            .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
    }
}
