//! OS-level per-run execution sandbox for the web spawn path (RB-01 / issue #1129).
//!
//! RB-01a (issue #1129, PR #1241) closed the *environment* dimension of the
//! tenant boundary: the spawned CLI no longer inherits server secrets, and
//! `RLIMIT_DATA`/`RLIMIT_CPU`/`RLIMIT_NPROC` bound CPU, memory, and process
//! count on Linux. What remained open was the **filesystem** and **network**
//! dimension: a tenant-authored rule shell could still read (and write) any
//! host path the server account can reach — another tenant's workspace, the
//! server database, the operator's `$HOME` credentials — and open arbitrary
//! TCP connections.
//!
//! This module closes that gap with **Landlock** (Linux 5.13+, unprivileged
//! LSM, kernel-enforced): the spawn installs a ruleset in a `pre_exec` hook
//! so the `oxo-flow` CLI — and every rule shell it spawns, by inheritance —
//! can only reach an explicit set of paths:
//!
//! - read + execute: system dirs (`/usr`, `/bin`, `/sbin`, `/lib`,
//!   `/lib32`, `/lib64`), the resolved engine binary and its parent
//!   directory, plus operator-declared `OXO_FLOW_SANDBOX_READ_PATHS`
//! - read + write + create: the run workdir, the run's scratch dir
//!   (`OXO_FLOW_TMPDIR`, defaulted to `<workdir>/.oxo-flow/tmp` per run),
//!   the engine dirs the operator configured (`OXO_FLOW_MODULE_CACHE`,
//!   `OXO_FLOW_ENV_STORE`), the engine's env-create lock dir
//!   (`$HOME/.oxo-flow/locks`), plus operator-declared
//!   `OXO_FLOW_SANDBOX_WRITE_PATHS`
//! - everything else — including `/proc`, `/sys`, `/etc`, the server's
//!   `$HOME`, the workspace root, and other tenants' workdirs — is denied
//!   at the kernel level (open-by-path, truncate, and device ioctls)
//! - on kernel 6.7+ (Landlock ABI v4) TCP `bind`/`connect` are denied
//!   outright; on 6.10+/6.12+ (ABI v5/v6) abstract-UNIX-socket and
//!   signal-delivery scoping is additionally handled
//!
//! The mechanism is **opt-in** (`OXO_FLOW_WEB_SANDBOX=landlock`) and
//! **fail-closed**: if the operator requests the sandbox and the kernel or
//! configuration cannot provide it, the spawn refuses and the run is marked
//! failed with a precise reason — it never silently runs unsandboxed. An
//! unparseable knob value likewise fails rather than degrading to `off`, so
//! a typo can never disable the gate (mirroring the `OXO_FLOW_DISK_HOLD_ROUNDS`
//! direction, not the `strict_placeholders` one).
//!
//! # Why Landlock (and what was evaluated)
//!
//! - *Per-run OS identity* (`sudo -u <user>`): the executor carries a
//!   legacy `sudo` payload branch, but every spawn site passes
//!   `auth_type="none"`; a real uid boundary needs operator-side account
//!   provisioning, sudoers rules, and run-dir chowns that an unprivileged
//!   server cannot perform. Documented as a deployment mode, not code.
//! - *Container-per-run* (`docker run`): a PID namespace breaks the
//!   process-group cancel machinery, the exit-code wrapper, and PID
//!   registration, and the daemon is root-equivalent on the host. Documented
//!   as the strict deployment mode (`docker-compose`), not implemented here.
//! - *Landlock*: kernel-enforced, unprivileged, no daemon, composes with
//!   every existing spawn behavior (same process group, same PID
//!   bookkeeping, same wrapper), and restricts the whole spawned tree by
//!   inheritance.
//!
//! # Honest residual (see the web API reference for the full matrix)
//!
//! - Network is denied for **TCP only** (Landlock ABI v4 scope); UDP and
//!   ICMP are not covered by the kernel ABI on 6.x kernels.
//! - Abstract UNIX sockets and cross-process signals are only scoped on
//!   kernels 6.10+/6.12+ (ABI v5/v6).
//! - There is no PID namespace: a sandboxed run still shares the kernel
//!   process table (mitigated, not eliminated: `/proc` reads are denied, so
//!   sibling runs' and the server's own environment are not reachable).
//! - Landlock restricts *path-based* access; already-open file descriptors
//!   inherited across the spawn (the execution-log handle) remain writable.
//! - Self-executable re-resolution via `/proc/self/exe`
//!   (`std::env::current_exe`) fails inside the sandbox; the engine's
//!   repair-verification rerun therefore requires `OXO_FLOW_BIN` (which this
//!   module injects into the child when the web spawn resolved an absolute
//!   binary path).
//!
//! The two-tenant adversarial suite
//! (`crates/oxo-flow-web/tests/rb01_sandbox_adversarial.rs`) drives the REAL
//! spawn path and asserts every claimed dimension, with an unsandboxed
//! control run proving the assertions can fail.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use std::sync::Arc;

use oxo_flow_core::executor::parse_env_passthrough;

/// Operator knob (RB-01 / issue #1129): selects the per-run OS sandbox for
/// web-spawned runs. `off` / unset (default) = today's behavior; `landlock`
/// = kernel-enforced filesystem (and, where the kernel supports it, network)
/// sandbox on the spawned CLI process tree. Any other value is an error and
/// fails the spawn — a typo must never silently disable the gate.
pub const SANDBOX_MODE_ENV: &str = "OXO_FLOW_WEB_SANDBOX";

/// Comma-separated operator read-only trees granted to sandboxed runs
/// (reference data, tool installs). Paths must be absolute; the grant is
/// read + directory-listing + execute — never write.
pub const SANDBOX_READ_PATHS_ENV: &str = "OXO_FLOW_SANDBOX_READ_PATHS";

/// Comma-separated operator read-write trees granted to sandboxed runs on
/// top of the automatic write set (run workdir, scratch, engine dirs).
/// Paths must be absolute. CAUTION: a granted write tree is shared across
/// ALL runs of the server account — grant data/tool trees, never another
/// tenant's workspace.
pub const SANDBOX_WRITE_PATHS_ENV: &str = "OXO_FLOW_SANDBOX_WRITE_PATHS";

/// Network posture for sandboxed runs: `deny` (default — TCP bind/connect
/// denied where the kernel supports it, best-effort), `require` (fail the
/// spawn when the kernel cannot deny TCP, for operators who must not run
/// network-capable tenants), or `off` (no network restriction).
pub const SANDBOX_NET_ENV: &str = "OXO_FLOW_WEB_SANDBOX_NET";

/// System trees every sandboxed run may read + execute (system binaries,
/// dynamic libraries, locale data). `/etc`, `/proc`, `/sys`, `/root`, `/var`
/// and everything else stay denied; the dynamic loader tolerates a missing
/// `/etc/ld.so.cache` by falling back to the default lib dirs.
const SYSTEM_READ_DIRS: &[&str] = &["/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64"];

/// A fully resolved sandbox enforcement plan: syscall-ready data only, so
/// the `pre_exec` closure between fork and exec allocates nothing.
#[derive(Clone, Debug)]
pub struct SandboxPlan {
    #[cfg(target_os = "linux")]
    inner: Arc<linux::Enforcement>,
    #[cfg(not(target_os = "linux"))]
    _private: (),
}

impl SandboxPlan {
    /// Install the sandbox in a `pre_exec` hook: between fork and exec the
    /// child drops `no_new_privs`, builds the ruleset, restricts itself, and
    /// only then execs the run wrapper. Every subsequent process in the run
    /// (the CLI, its rule shells, their children) inherits the restriction;
    /// the kernel enforces it regardless of uid. A failure inside the hook
    /// aborts the spawn (fail-closed) rather than running unsandboxed.
    #[cfg_attr(target_os = "linux", allow(unsafe_code))] // pre_exec is the only post-fork hook API
    pub fn attach(&self, cmd: &mut tokio::process::Command) {
        #[cfg(target_os = "linux")]
        {
            let plan = Arc::clone(&self.inner);
            // SAFETY: `pre_exec` is the only mechanism to run a closure
            // between fork and exec; the closure (`linux::enforce`) performs
            // syscalls only and never touches tokio or allocator state.
            unsafe {
                cmd.pre_exec(move || linux::enforce(&plan));
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = cmd;
        }
    }
}

// ── Configuration surface (portable) ──────────────────────────────────────

/// Resolve the sandbox for one web spawn.
///
/// Reads the operator knobs from the server environment, injects per-run
/// defaults into the child environment (`OXO_FLOW_TMPDIR`, and
/// `OXO_FLOW_BIN` when the web spawn resolved an absolute binary — the
/// engine's own re-spawn paths need it once `/proc/self/exe` is denied),
/// then builds the enforcement plan. `Ok(None)` = sandbox off (default);
/// every `Err` is fail-closed and the caller must not spawn.
pub fn configure(
    run_dir: &Path,
    spawn_envs: &mut HashMap<String, String>,
    sudo_mode: bool,
    oxo_bin: &Path,
) -> Result<Option<SandboxPlan>, String> {
    // Fail closed on a knob that is set but not valid UTF-8: `.ok()` would
    // turn it into "unset" and silently disable the requested sandbox.
    let mode = env_utf8(SANDBOX_MODE_ENV)?;
    let net = env_utf8(SANDBOX_NET_ENV)?;
    let read_paths = env_utf8(SANDBOX_READ_PATHS_ENV)?;
    let write_paths = env_utf8(SANDBOX_WRITE_PATHS_ENV)?;
    configure_with(
        mode.as_deref(),
        net.as_deref(),
        read_paths.as_deref(),
        write_paths.as_deref(),
        std::env::var("HOME").ok().as_deref(),
        run_dir,
        spawn_envs,
        sudo_mode,
        oxo_bin,
    )
}

/// Read a sandbox knob as UTF-8, failing closed on a non-UTF-8 value.
fn env_utf8(name: &str) -> Result<Option<String>, String> {
    utf8_value(name, std::env::var_os(name))
}

/// The testable core of [`env_utf8`].
fn utf8_value(name: &str, value: Option<std::ffi::OsString>) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(value) => value
            .into_string()
            .map(Some)
            .map_err(|_| format!("{name} is not valid UTF-8")),
    }
}

/// The environment-read-free core of [`configure`] — every knob value is a
/// parameter so tests can drive it without process-global mutation. On
/// success with the sandbox enabled, `spawn_envs` carries the injected
/// per-run defaults the child must see.
#[allow(clippy::too_many_arguments)]
fn configure_with(
    mode_raw: Option<&str>,
    net_raw: Option<&str>,
    read_paths_raw: Option<&str>,
    write_paths_raw: Option<&str>,
    server_home: Option<&str>,
    run_dir: &Path,
    spawn_envs: &mut HashMap<String, String>,
    sudo_mode: bool,
    oxo_bin: &Path,
) -> Result<Option<SandboxPlan>, String> {
    if parse_mode(mode_raw)?.is_none() {
        return Ok(None);
    }

    if sudo_mode {
        // `no_new_privs` blocks setuid gain, so a sudo-prefixed payload would
        // fail opaquely mid-spawn; reject the combination up front instead.
        return Err(
            "OXO_FLOW_WEB_SANDBOX cannot be combined with the legacy sudo spawn payload \
             (auth_type=sudo): no_new_privs forbids the uid transition"
                .to_string(),
        );
    }

    // Per-run scratch + self-binary injection, then the granted path set —
    // portable so the grants and the child env stay consistent; the caller
    // discards both when the platform check below fails.
    let default_scratch = !spawn_envs.contains_key("OXO_FLOW_TMPDIR");
    if default_scratch {
        spawn_envs.insert(
            "OXO_FLOW_TMPDIR".to_string(),
            run_dir
                .join(".oxo-flow")
                .join("tmp")
                .to_string_lossy()
                .into_owned(),
        );
    }
    #[cfg(target_os = "linux")]
    {
        // PATH is inherited from the web server and may begin with private
        // directories (e.g. ~/.cargo/bin) that Landlock deliberately denies.
        // Put the granted system trees first so the engine can resolve its
        // shell and basic tools without broadening the filesystem grant; retain
        // operator PATH entries afterward for explicitly granted tool trees.
        let path = spawn_envs.get("PATH").cloned();
        spawn_envs.insert("PATH".to_string(), sandboxed_path(path.as_deref()));

        let scratch = spawn_envs
            .get("OXO_FLOW_TMPDIR")
            .ok_or_else(|| "sandbox scratch directory was not configured".to_string())?;
        let scratch_path = Path::new(scratch);
        if !scratch_path.is_absolute() {
            return Err(format!(
                "OXO_FLOW_TMPDIR must be absolute when {SANDBOX_MODE_ENV}=landlock; got {scratch_path:?}"
            ));
        }
        // Landlock requires the granted path to exist before the child enters
        // its pre_exec hook. The engine normally creates TMPDIR later, after
        // the CLI has started; do it here in the trusted parent first.
        std::fs::create_dir_all(scratch_path).map_err(|e| {
            format!("failed to create sandbox scratch directory {scratch_path:?}: {e}")
        })?;
        let canonical_scratch = std::fs::canonicalize(scratch_path).map_err(|e| {
            format!("failed to canonicalize sandbox scratch directory {scratch_path:?}: {e}")
        })?;
        if default_scratch {
            let canonical_run = std::fs::canonicalize(run_dir)
                .map_err(|e| format!("failed to canonicalize run directory {run_dir:?}: {e}"))?;
            if !canonical_scratch.starts_with(&canonical_run) {
                return Err(format!(
                    "default sandbox scratch directory {canonical_scratch:?} escaped run directory {canonical_run:?}"
                ));
            }
        }
        spawn_envs.insert(
            "OXO_FLOW_TMPDIR".to_string(),
            canonical_scratch.to_string_lossy().into_owned(),
        );
    }
    if !spawn_envs.contains_key("OXO_FLOW_BIN") && oxo_bin.is_absolute() && oxo_bin.is_file() {
        // The sandbox denies /proc, so `std::env::current_exe()` inside the
        // child fails; the engine's own re-spawn paths (repair verification
        // rerun) fall back to OXO_FLOW_BIN — give it the exact absolute
        // binary the server resolved.
        spawn_envs.insert(
            "OXO_FLOW_BIN".to_string(),
            oxo_bin.to_string_lossy().into_owned(),
        );
    }
    let grants = build_grants(
        run_dir,
        spawn_envs,
        server_home,
        read_paths_raw,
        write_paths_raw,
        oxo_bin,
    )?;

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (net_raw, grants);
        Err(
            "OXO_FLOW_WEB_SANDBOX=landlock requires Linux (Landlock, kernel 5.13+); \
             refusing to spawn unsandboxed (fail-closed)"
                .to_string(),
        )
    }

    #[cfg(target_os = "linux")]
    {
        let grants = prepare_write_grants(grants)?;
        let abi = linux::probe_abi()?;
        let net_mode = linux::parse_net_mode(net_raw)?;
        let (net_supported, scoped_supported) = linux::probe_features(abi);
        if !net_supported {
            match net_mode {
                linux::NetMode::Require => {
                    return Err(format!(
                        "{SANDBOX_NET_ENV}=require but this kernel (Landlock ABI {abi}) \
                         cannot deny TCP bind/connect (needs ABI v4, kernel 6.7+)"
                    ));
                }
                linux::NetMode::Deny => tracing::warn!(
                    abi,
                    "kernel lacks Landlock ABI v4 — sandboxed runs get filesystem \
                     enforcement only; TCP network remains unrestricted"
                ),
                linux::NetMode::Off => {}
            }
        }
        let scoped = if scoped_supported {
            // v5 scopes abstract UNIX sockets; v6 adds signal scoping.
            if abi >= 6 {
                linux::SCOPE_ABSTRACT_UNIX_SOCKET | linux::SCOPE_SIGNAL
            } else {
                linux::SCOPE_ABSTRACT_UNIX_SOCKET
            }
        } else {
            0
        };
        if !scoped_supported {
            tracing::warn!(
                abi,
                "kernel lacks Landlock ABI v5/v6 — abstract UNIX sockets and cross-process \
                 signal delivery are not scoped by the sandbox"
            );
        }

        Ok(Some(SandboxPlan {
            inner: Arc::new(linux::enforcement_from_grants(
                linux::handled_fs_for_abi(abi),
                if net_supported && net_mode != linux::NetMode::Off {
                    linux::NET_BIND_TCP | linux::NET_CONNECT_TCP
                } else {
                    0
                },
                scoped,
                grants,
            )?),
        }))
    }
}

/// Knob parser: `off`/unset/empty = disabled, `landlock` = enabled, anything
/// else is an error so a typo can never silently disable the gate.
fn parse_mode(raw: Option<&str>) -> Result<Option<()>, String> {
    match raw.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(v) if v.eq_ignore_ascii_case("off") => Ok(None),
        Some(v) if v.eq_ignore_ascii_case("landlock") => Ok(Some(())),
        Some(other) => Err(format!(
            "{SANDBOX_MODE_ENV}={other:?} is not a valid sandbox mode; \
             accepted values: off, landlock"
        )),
    }
}

#[derive(Debug, Default)]
struct Grants {
    read_dirs: Vec<PathBuf>,
    read_files: Vec<PathBuf>,
    write_dirs: Vec<PathBuf>,
    /// Character devices granted read+write as individual files.
    dev_files: Vec<PathBuf>,
}

/// Assemble the granted path set. Every path must be absolute — a relative
/// grant would silently resolve against the server's cwd, which is exactly
/// the ambiguity a tenant boundary must not have.
fn build_grants(
    run_dir: &Path,
    spawn_envs: &HashMap<String, String>,
    server_home: Option<&str>,
    read_paths_raw: Option<&str>,
    write_paths_raw: Option<&str>,
    oxo_bin: &Path,
) -> Result<Grants, String> {
    let mut grants = Grants::default();

    // Write set: the run's own workdir + the engine dirs the operator
    // configured (scratch, module cache, env store) + the env-create lock
    // dir + operator-declared write trees.
    grants.write_dirs.push(checked_absolute(
        run_dir.to_string_lossy().as_ref(),
        "run workdir",
    )?);
    for name in [
        "OXO_FLOW_TMPDIR",
        "OXO_FLOW_MODULE_CACHE",
        "OXO_FLOW_ENV_STORE",
    ] {
        if let Some(value) = spawn_envs.get(name) {
            grants.write_dirs.push(PathBuf::from(value));
        }
    }
    if let Some(home) = server_home.filter(|h| !h.is_empty()) {
        // Engine-internal flock dir (oxo_flow_core::executor env-create
        // locks); granting just this subtree keeps concurrent env creation
        // working while the rest of the server's $HOME stays denied.
        grants
            .write_dirs
            .push(Path::new(home).join(".oxo-flow").join("locks"));
    }
    if let Some(raw) = write_paths_raw {
        for path in parse_path_list(raw) {
            grants
                .write_dirs
                .push(checked_absolute(&path, SANDBOX_WRITE_PATHS_ENV)?);
        }
    }

    // Read set: system dirs (those that exist on this host), the resolved
    // engine binary's parent (interpreters and adjacent assets), and
    // operator-declared read trees.
    for dir in SYSTEM_READ_DIRS {
        let path = Path::new(dir);
        if path.exists() {
            grants.read_dirs.push(path.to_path_buf());
        }
    }
    if let Some(parent) = oxo_bin.parent().filter(|p| !p.as_os_str().is_empty()) {
        grants.read_dirs.push(parent.to_path_buf());
    }
    if let Some(raw) = read_paths_raw {
        for path in parse_path_list(raw) {
            grants
                .read_dirs
                .push(checked_absolute(&path, SANDBOX_READ_PATHS_ENV)?);
        }
    }
    // The engine binary itself must be executable by the child.
    if !oxo_bin.is_absolute() {
        return Err(format!(
            "sandbox requires an absolute engine binary path; resolved {oxo_bin:?} — \
             set OXO_FLOW_BIN to an absolute path"
        ));
    }
    grants.read_files.push(oxo_bin.to_path_buf());

    // Ordinary process operation needs the minimal character devices
    // (`stdin = /dev/null` for rule shells, entropy/zero for tools). Grant
    // only these exact device files, never the /dev tree (details in
    // `MINIMAL_DEV_FILES`).
    for dev in MINIMAL_DEV_FILES {
        let path = Path::new(dev);
        if path.exists() {
            grants.dev_files.push(path.to_path_buf());
        }
    }

    Ok(grants)
}

/// Comma-separated path list — same parsing contract as
/// `OXO_FLOW_ENV_PASSTHROUGH` (`parse_env_passthrough`), reused rather than
/// retyped.
fn parse_path_list(raw: &str) -> Vec<String> {
    parse_env_passthrough(Some(raw))
}

fn checked_absolute(path: &str, knob: &str) -> Result<PathBuf, String> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err(format!(
            "{knob} entries must be absolute paths; got {path:?}"
        ));
    }
    Ok(path.to_path_buf())
}

/// Landlock only accepts existing paths when its pre-exec rules are added.
/// Materialize the approved write roots in the trusted parent and pin each
/// grant to its canonical path before handing it to the child.
#[cfg(target_os = "linux")]
fn sandboxed_path(original: Option<&str>) -> String {
    let mut entries = Vec::new();
    for path in [
        "/usr/local/sbin",
        "/usr/local/bin",
        "/usr/sbin",
        "/usr/bin",
        "/sbin",
        "/bin",
    ] {
        if Path::new(path).is_dir() {
            entries.push(path.to_string());
        }
    }
    if let Some(original) = original {
        for path in original.split(':').filter(|path| !path.is_empty()) {
            if !entries.iter().any(|entry| entry == path) {
                entries.push(path.to_string());
            }
        }
    }
    entries.join(":")
}

#[cfg(target_os = "linux")]
fn prepare_write_grants(mut grants: Grants) -> Result<Grants, String> {
    for path in &mut grants.write_dirs {
        if !path.is_absolute() {
            return Err(format!(
                "sandbox write grant must be absolute; got {path:?}"
            ));
        }
        std::fs::create_dir_all(&*path)
            .map_err(|e| format!("failed to create sandbox write grant {path:?}: {e}"))?;
        *path = std::fs::canonicalize(&*path)
            .map_err(|e| format!("failed to canonicalize sandbox write grant {path:?}: {e}"))?;
    }
    Ok(grants)
}

/// Character devices every spawned process needs for ordinary operation
/// (`stdin = /dev/null` on rule shells; entropy/zero reads by tools). Each is
/// granted read+write as a single file — never the whole `/dev` tree, so
/// other devices (ttys, disks, `/dev/mem`) stay denied.
const MINIMAL_DEV_FILES: &[&str] = &["/dev/null", "/dev/zero", "/dev/urandom", "/dev/random"];

// ── Kernel interface (Linux only) ─────────────────────────────────────────
//
// Landlock ABI constants (include/uapi/linux/landlock.h, stable). libc 0.2
// ships the syscall numbers (`SYS_landlock_*`) but not the uapi structs, so
// they are defined here verbatim. Enforcement data is stack-only and passed
// to raw syscalls inside `pre_exec` — no allocation between fork and exec
// (the closure must stay async-signal-safe).

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::CString;
    use std::io;

    use super::Grants;

    /// NUL-check + CString conversion for a granted path (the ruleset
    /// interface speaks C strings; a path with an interior NUL can never be
    /// granted faithfully).
    fn cstr(path: &std::path::Path) -> Result<CString, String> {
        CString::new(path.to_string_lossy().into_owned())
            .map_err(|_| format!("sandbox path {path:?} contains an interior NUL byte"))
    }

    /// Convert the portable grant set into syscall-ready enforcement data.
    pub(super) fn enforcement_from_grants(
        handled_fs: u64,
        handled_net: u64,
        handled_scoped: u64,
        grants: Grants,
    ) -> Result<Enforcement, String> {
        Ok(Enforcement {
            handled_fs,
            handled_net,
            handled_scoped,
            read_dirs: grants
                .read_dirs
                .iter()
                .map(|p| cstr(p))
                .collect::<Result<_, _>>()?,
            read_files: grants
                .read_files
                .iter()
                .map(|p| cstr(p))
                .collect::<Result<_, _>>()?,
            write_dirs: grants
                .write_dirs
                .iter()
                .map(|p| cstr(p))
                .collect::<Result<_, _>>()?,
            dev_files: grants
                .dev_files
                .iter()
                .map(|p| cstr(p))
                .collect::<Result<_, _>>()?,
        })
    }

    pub const CREATE_RULESET_VERSION: std::ffi::c_uint = 1 << 0;
    pub const RULE_PATH_BENEATH: std::ffi::c_int = 1;

    // FS access bits, ABI v1 (kernel 5.13).
    pub const FS_EXECUTE: u64 = 1 << 0;
    pub const FS_WRITE_FILE: u64 = 1 << 1;
    pub const FS_READ_FILE: u64 = 1 << 2;
    pub const FS_READ_DIR: u64 = 1 << 3;
    pub const FS_REMOVE_DIR: u64 = 1 << 4;
    pub const FS_REMOVE_FILE: u64 = 1 << 5;
    pub const FS_MAKE_CHAR: u64 = 1 << 6;
    pub const FS_MAKE_DIR: u64 = 1 << 7;
    pub const FS_MAKE_REG: u64 = 1 << 8;
    pub const FS_MAKE_SOCK: u64 = 1 << 9;
    pub const FS_MAKE_FIFO: u64 = 1 << 10;
    pub const FS_MAKE_BLOCK: u64 = 1 << 11;
    pub const FS_MAKE_SYM: u64 = 1 << 12;
    /// ABI v2 (kernel 6.2): `truncate(2)` needs no file descriptor — without
    /// handling this bit a tenant could truncate any host file.
    pub const FS_TRUNCATE: u64 = 1 << 13;
    /// ABI v3 (kernel 6.3): `ioctl` on character/block device files.
    pub const FS_IOCTL_DEV: u64 = 1 << 14;

    /// ABI v4 (kernel 6.7): network bits live in their own handled field;
    /// zero granted rules deny all TCP bind/connect.
    pub const NET_BIND_TCP: u64 = 1 << 0;
    pub const NET_CONNECT_TCP: u64 = 1 << 1;

    /// ABI v5 (kernel 6.10): abstract UNIX socket scoping.
    pub const SCOPE_ABSTRACT_UNIX_SOCKET: u64 = 1 << 0;
    /// ABI v6 (kernel 6.12): signal scoping (a sandboxed process can no
    /// longer signal processes outside its own domain — e.g. the server or
    /// sibling runs).
    pub const SCOPE_SIGNAL: u64 = 1 << 1;

    pub const FS_BITS_V1: u64 = FS_EXECUTE
        | FS_WRITE_FILE
        | FS_READ_FILE
        | FS_READ_DIR
        | FS_REMOVE_DIR
        | FS_REMOVE_FILE
        | FS_MAKE_CHAR
        | FS_MAKE_DIR
        | FS_MAKE_REG
        | FS_MAKE_SOCK
        | FS_MAKE_FIFO
        | FS_MAKE_BLOCK
        | FS_MAKE_SYM;

    /// Access granted to a read-only tree: read files, list directories,
    /// execute (so operator tool trees and system bins work). Write-family
    /// bits are deliberately absent.
    pub const GRANT_READ: u64 = FS_READ_FILE | FS_READ_DIR | FS_EXECUTE;

    /// Read grant for a single file (no directory bits — the kernel rejects
    /// directory-only access on a non-directory parent rule).
    pub const GRANT_READ_FILE: u64 = FS_READ_FILE | FS_EXECUTE;

    /// Network posture knob: `off`, `deny` (default), `require`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum NetMode {
        Off,
        Deny,
        Require,
    }

    /// `#[repr(C)]` mirror of `struct landlock_ruleset_attr` — the kernel
    /// reads exactly `handled_access_fs` (+ `handled_access_net` on ABI v4+,
    /// `handled_scoped` on ABI v5+); trailing fields are omitted by passing
    /// the matching size, which is the documented forward-compat mechanism.
    #[repr(C)]
    pub struct RulesetAttr {
        pub handled_access_fs: u64,
        pub handled_access_net: u64,
        pub handled_scoped: u64,
    }

    /// `#[repr(C)]` mirror of `struct landlock_path_beneath_attr`.
    #[repr(C)]
    pub struct PathBeneathAttr {
        pub allowed_access: u64,
        pub parent_fd: std::ffi::c_int,
    }

    /// The syscall-ready enforcement data moved into the `pre_exec` closure.
    #[derive(Debug)]
    pub struct Enforcement {
        pub handled_fs: u64,
        pub handled_net: u64,
        pub handled_scoped: u64,
        pub read_dirs: Vec<CString>,
        pub read_files: Vec<CString>,
        pub write_dirs: Vec<CString>,
        pub dev_files: Vec<CString>,
    }

    pub fn parse_net_mode(raw: Option<&str>) -> Result<NetMode, String> {
        match raw.map(str::trim).filter(|v| !v.is_empty()) {
            None => Ok(NetMode::Deny),
            Some(v) if v.eq_ignore_ascii_case("off") => Ok(NetMode::Off),
            Some(v) if v.eq_ignore_ascii_case("deny") => Ok(NetMode::Deny),
            Some(v) if v.eq_ignore_ascii_case("require") => Ok(NetMode::Require),
            Some(other) => Err(format!(
                "{}={other:?} is not a valid network mode; accepted values: off, deny, require",
                super::SANDBOX_NET_ENV
            )),
        }
    }

    pub fn handled_fs_for_abi(abi: u32) -> u64 {
        let mut bits = FS_BITS_V1;
        if abi >= 2 {
            bits |= FS_TRUNCATE;
        }
        if abi >= 3 {
            bits |= FS_IOCTL_DEV;
        }
        bits
    }

    /// Query the kernel's Landlock ABI version. `Err` = no usable Landlock
    /// (kernel < 5.13, `CONFIG_SECURITY_LANDLOCK` unset, or disabled at
    /// boot).
    #[allow(unsafe_code)] // raw landlock syscalls; no stable libc binding exists
    pub fn probe_abi() -> Result<u32, String> {
        // SAFETY: `landlock_create_ruleset(NULL, 0, VERSION)` is a pure
        // query — no pointers are dereferenced and no fd is created.
        let version = unsafe {
            nix::libc::syscall(
                nix::libc::SYS_landlock_create_ruleset,
                std::ptr::null::<nix::libc::c_void>(),
                0usize,
                CREATE_RULESET_VERSION,
            )
        };
        if version < 0 {
            return Err(format!(
                "Landlock is unavailable on this kernel ({}) — Linux 5.13+ with \
                 CONFIG_SECURITY_LANDLOCK is required; refusing to spawn unsandboxed \
                 (fail-closed)",
                io::Error::last_os_error()
            ));
        }
        Ok(version as u32)
    }

    /// Validate the handled-feature combination against this kernel by
    /// creating a throwaway ruleset. Guards against any ABI-mapping mistake:
    /// a feature the kernel does not know would make ruleset creation fail
    /// with `EINVAL` inside `pre_exec`, so it is probed here in the parent
    /// (where it can be warned about and degraded) instead.
    #[allow(unsafe_code)]
    pub fn probe_features(abi: u32) -> (bool, bool) {
        let handled_fs = handled_fs_for_abi(abi);
        let net = if abi >= 4 {
            probe_and_close(handled_fs, NET_BIND_TCP | NET_CONNECT_TCP, 0)
        } else {
            false
        };
        let net_bits = if net {
            NET_BIND_TCP | NET_CONNECT_TCP
        } else {
            0
        };
        let scoped = if abi >= 6 {
            probe_and_close(
                handled_fs,
                net_bits,
                SCOPE_ABSTRACT_UNIX_SOCKET | SCOPE_SIGNAL,
            )
        } else if abi >= 5 {
            probe_and_close(handled_fs, net_bits, SCOPE_ABSTRACT_UNIX_SOCKET)
        } else {
            false
        };
        (net, scoped)
    }

    /// Probe a handled combination and close the throwaway ruleset fd
    /// immediately — `probe_features` only needs the verdict.
    #[allow(unsafe_code)]
    fn probe_and_close(handled_fs: u64, handled_net: u64, handled_scoped: u64) -> bool {
        match probe_ruleset(handled_fs, handled_net, handled_scoped) {
            Ok(fd) => {
                // SAFETY: closing the fd probe_ruleset just created.
                unsafe { nix::libc::close(fd) };
                true
            }
            Err(_) => false,
        }
    }

    /// The ruleset-attr byte size a kernel with the given handled sets
    /// accepts. Pre-6.7 kernels (ABI v1–v3) reject a size larger than the
    /// fields they know — the flexible "trailing bytes must be zero"
    /// contract only landed with ABI v4 — so the size must always match the
    /// handled fields exactly.
    fn attr_size_for(handled_net: u64, handled_scoped: u64) -> usize {
        if handled_scoped != 0 {
            std::mem::size_of::<RulesetAttr>() // fs + net + scoped (24)
        } else if handled_net != 0 {
            std::mem::size_of::<RulesetAttr>() - std::mem::size_of::<u64>() // fs + net (16)
        } else {
            std::mem::size_of::<u64>() // fs only (8)
        }
    }

    /// Create a ruleset with the given handled sets, returning the fd (the
    /// caller closes it) or the kernel error.
    #[allow(unsafe_code)]
    pub fn probe_ruleset(
        handled_fs: u64,
        handled_net: u64,
        handled_scoped: u64,
    ) -> io::Result<i32> {
        let attr = RulesetAttr {
            handled_access_fs: handled_fs,
            handled_access_net: handled_net,
            handled_scoped,
        };
        // SAFETY: `attr` is a fully initialized stack struct passed by
        // pointer; the size covers exactly the fields this kernel ABI
        // version understands (features are probed before use, see
        // `probe_features`).
        let fd = unsafe {
            nix::libc::syscall(
                nix::libc::SYS_landlock_create_ruleset,
                &attr as *const RulesetAttr as *const nix::libc::c_void,
                attr_size_for(handled_net, handled_scoped),
                0u32,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(fd as i32)
    }

    /// The `pre_exec` body: install the Landlock restriction on the forked
    /// child. Stack-only data, syscall-only work — safe between fork and
    /// exec.
    #[allow(unsafe_code)] // post-fork pre-exec syscalls; no safe API exists
    pub fn enforce(plan: &Enforcement) -> io::Result<()> {
        // SAFETY: prctl with PR_SET_NO_NEW_PRIVS is a plain syscall;
        // required before an unprivileged `landlock_restrict_self`, and
        // inherited by exec.
        if unsafe { nix::libc::prctl(nix::libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the attr struct is fully initialized on the stack; the
        // handled combination was probed in the parent (probe_features) so
        // the kernel accepts this exact size/field set — and the size is
        // trimmed to the handled fields for pre-6.7 kernels that require an
        // exact match.
        let fd = unsafe {
            nix::libc::syscall(
                nix::libc::SYS_landlock_create_ruleset,
                &RulesetAttr {
                    handled_access_fs: plan.handled_fs,
                    handled_access_net: plan.handled_net,
                    handled_scoped: plan.handled_scoped,
                } as *const RulesetAttr as *const nix::libc::c_void,
                attr_size_for(plan.handled_net, plan.handled_scoped),
                0u32,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = fd as i32;

        let add_rule = |parent_fd: i32, allowed: u64| -> io::Result<()> {
            let attr = PathBeneathAttr {
                allowed_access: allowed,
                parent_fd,
            };
            // SAFETY: `attr` is an initialized stack struct; `parent_fd` is
            // an O_PATH descriptor opened just above.
            let rc = unsafe {
                nix::libc::syscall(
                    nix::libc::SYS_landlock_add_rule,
                    fd,
                    RULE_PATH_BENEATH,
                    &attr as *const PathBeneathAttr as *const nix::libc::c_void,
                    0u32,
                )
            };
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        };
        let open_path = |path: &CString| -> io::Result<i32> {
            // SAFETY: `path` is a NUL-terminated CString; O_PATH opens a
            // descriptor for the path itself without reading contents, and
            // O_CLOEXEC keeps it out of the final exec.
            let fd =
                unsafe { nix::libc::open(path.as_ptr(), nix::libc::O_PATH | nix::libc::O_CLOEXEC) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(fd)
        };

        let restrict = |fd: i32| -> io::Result<()> {
            // SAFETY: plain syscall on the ruleset fd; requires the
            // no_new_privs bit set above.
            let rc = unsafe { nix::libc::syscall(nix::libc::SYS_landlock_restrict_self, fd, 0u32) };
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        };

        // Grant every tree, then restrict. Any failure aborts the spawn
        // (fail-closed): a partially-applied ruleset never reaches exec.
        let mut path_fds = Vec::new();
        let result = (|| -> io::Result<()> {
            for path in &plan.write_dirs {
                let pfd = open_path(path)?;
                path_fds.push(pfd);
                add_rule(pfd, plan.handled_fs)?;
            }
            for path in &plan.read_dirs {
                let pfd = open_path(path)?;
                path_fds.push(pfd);
                add_rule(pfd, GRANT_READ)?;
            }
            for path in &plan.read_files {
                let pfd = open_path(path)?;
                path_fds.push(pfd);
                add_rule(pfd, GRANT_READ_FILE)?;
            }
            for path in &plan.dev_files {
                let pfd = open_path(path)?;
                path_fds.push(pfd);
                add_rule(pfd, FS_READ_FILE | FS_WRITE_FILE)?;
            }
            restrict(fd)
        })();
        // SAFETY: closing the fds this function opened; valid regardless of
        // the grant loop's outcome.
        for pfd in path_fds {
            unsafe { nix::libc::close(pfd) };
        }
        // SAFETY: closing the ruleset fd opened above.
        unsafe { nix::libc::close(fd) };
        result
    }

    #[cfg(test)]
    mod tests {
        use super::super::parse_path_list;
        use super::*;

        #[test]
        fn net_mode_knob_defaults_and_garbage() {
            assert_eq!(parse_net_mode(None), Ok(NetMode::Deny));
            assert_eq!(parse_net_mode(Some("")), Ok(NetMode::Deny));
            assert_eq!(parse_net_mode(Some(" ")), Ok(NetMode::Deny));
            assert_eq!(parse_net_mode(Some("off")), Ok(NetMode::Off));
            assert_eq!(parse_net_mode(Some("deny")), Ok(NetMode::Deny));
            assert_eq!(parse_net_mode(Some("require")), Ok(NetMode::Require));
            assert!(parse_net_mode(Some("blocked")).is_err());
        }

        /// A knob that is set to non-UTF-8 bytes must NOT read as "unset":
        /// that would silently disable the requested sandbox.
        #[test]
        #[cfg(unix)]
        fn non_utf8_knob_value_fails_closed() {
            use std::os::unix::ffi::OsStringExt;
            let raw = std::ffi::OsString::from_vec(vec![0xff, 0xfe]);
            let err =
                utf8_value(SANDBOX_MODE_ENV, Some(raw)).expect_err("non-UTF-8 must fail closed");
            assert!(
                err.contains(SANDBOX_MODE_ENV) && err.contains("not valid UTF-8"),
                "{err}"
            );
            assert!(utf8_value(SANDBOX_MODE_ENV, None).unwrap().is_none());
        }

        #[test]
        fn handled_fs_bits_grow_with_abi() {
            assert_eq!(handled_fs_for_abi(1), FS_BITS_V1);
            assert_eq!(handled_fs_for_abi(2), FS_BITS_V1 | FS_TRUNCATE);
            assert_eq!(
                handled_fs_for_abi(3),
                FS_BITS_V1 | FS_TRUNCATE | FS_IOCTL_DEV
            );
            assert_eq!(handled_fs_for_abi(9), handled_fs_for_abi(3));
        }

        /// The probe runs against the REAL kernel in CI — assert the
        /// advertised ABI features actually produce a usable ruleset, so a
        /// future uapi mismatch fails here instead of inside a spawn.
        #[test]
        fn feature_probes_build_valid_rulesets_on_this_kernel() {
            let abi = probe_abi().expect("CI Linux kernels ship Landlock");
            assert!(abi >= 1, "unexpected ABI version {abi}");
            let (net, scoped) = probe_features(abi);
            if abi >= 4 {
                assert!(net, "ABI {abi} kernel must accept handled TCP bits");
            } else {
                assert!(!net);
            }
            if abi >= 5 {
                assert!(scoped, "ABI {abi} kernel must accept handled scoped bits");
            } else {
                assert!(!scoped);
            }
        }

        #[test]
        fn path_list_parsing_reuses_the_passthrough_contract() {
            assert_eq!(parse_path_list("/a , /b ,, /c "), vec!["/a", "/b", "/c"]);
        }

        #[test]
        fn cstr_round_trips_paths_and_rejects_nul() {
            assert!(cstr(std::path::Path::new("/usr")).is_ok());
            let err = cstr(std::path::Path::new("/bad\0path"))
                .expect_err("interior NUL must be rejected");
            assert!(err.contains("NUL"), "{err}");
        }
    }
}

// ── Tests (portable — kernel paths are exercised against the real spawn
//    path by `tests/rb01_sandbox_adversarial.rs` on Linux) ─────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn env_map(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn mode_knob_off_unset_and_garbage() {
        assert_eq!(parse_mode(None), Ok(None));
        assert_eq!(parse_mode(Some("")), Ok(None));
        assert_eq!(parse_mode(Some("  ")), Ok(None));
        assert_eq!(parse_mode(Some("off")), Ok(None));
        assert_eq!(parse_mode(Some("OFF")), Ok(None));
        assert_eq!(parse_mode(Some("landlock")), Ok(Some(())));
        assert_eq!(parse_mode(Some(" LANDLOCK ")), Ok(Some(())));
        // A typo must FAIL, never silently disable the gate.
        assert!(parse_mode(Some("landlockd")).is_err());
        assert!(parse_mode(Some("1")).is_err());
    }

    #[test]
    fn grants_assemble_run_and_engine_paths() {
        let run_dir = std::env::temp_dir().join("rb01-grants-run");
        let bin = std::env::temp_dir()
            .join("rb01-grants-bin")
            .join("oxo-flow");
        let envs = env_map(&[
            ("OXO_FLOW_TMPDIR", "/opt/oxo-scratch"),
            ("OXO_FLOW_MODULE_CACHE", "/var/cache/oxo/modules"),
        ]);
        let grants = build_grants(
            &run_dir,
            &envs,
            Some("/home/oxo-server"),
            Some("/data/refs,/data/tools"),
            None,
            &bin,
        )
        .expect("grants build");
        assert!(grants.write_dirs.contains(&run_dir));
        assert!(
            grants
                .write_dirs
                .contains(&PathBuf::from("/opt/oxo-scratch"))
        );
        assert!(
            grants
                .write_dirs
                .contains(&PathBuf::from("/var/cache/oxo/modules"))
        );
        assert!(
            grants
                .write_dirs
                .contains(&PathBuf::from("/home/oxo-server/.oxo-flow/locks"))
        );
        assert!(grants.read_dirs.contains(&PathBuf::from("/data/refs")));
        assert!(grants.read_dirs.contains(&PathBuf::from("/data/tools")));
        assert!(grants.read_dirs.contains(&PathBuf::from("/usr")));
        assert_eq!(grants.read_files, vec![bin]);
        // Neither /etc nor /proc is ever granted.
        assert!(!grants.read_dirs.iter().any(|p| p.starts_with("/etc")));
        assert!(!grants.read_dirs.iter().any(|p| p.starts_with("/proc")));
    }

    #[test]
    fn grants_reject_relative_paths_and_bare_binary() {
        let bin = std::env::temp_dir().join("oxo-flow");
        let envs = HashMap::new();
        let err = build_grants(Path::new("relative/run"), &envs, None, None, None, &bin)
            .expect_err("relative workdir must be rejected");
        assert!(err.contains("absolute"), "{err}");

        let run = std::env::temp_dir().join("rb01-grants-run2");
        let err = build_grants(
            &run,
            &envs,
            None,
            Some("/data/refs,relative/tree"),
            None,
            &bin,
        )
        .expect_err("relative read path must be rejected");
        assert!(err.contains(SANDBOX_READ_PATHS_ENV), "{err}");

        let err = build_grants(&run, &envs, None, None, Some("scratch"), &bin)
            .expect_err("relative write path must be rejected");
        assert!(err.contains(SANDBOX_WRITE_PATHS_ENV), "{err}");

        // A bare binary name (PATH fallback) cannot be granted precisely.
        let err = build_grants(&run, &envs, None, None, None, Path::new("oxo-flow"))
            .expect_err("bare binary name must be rejected");
        assert!(err.contains("OXO_FLOW_BIN"), "{err}");
    }

    #[test]
    fn path_list_parsing_reuses_passthrough_contract() {
        assert_eq!(parse_path_list("/a , /b ,, /c "), vec!["/a", "/b", "/c"]);
        assert!(parse_path_list("").is_empty());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn requested_sandbox_fails_closed_off_linux() {
        let mut envs = HashMap::new();
        let run = std::env::temp_dir().join("rb01-offlinux");
        let err = configure_with(
            Some("landlock"),
            None,
            None,
            None,
            None,
            &run,
            &mut envs,
            false,
            &run.join("oxo-flow"),
        )
        .expect_err("a requested sandbox must never silently run unsandboxed");
        assert!(err.contains("Linux"), "{err}");
        // Default stays off.
        assert!(
            configure_with(
                None,
                None,
                None,
                None,
                None,
                &run,
                &mut envs,
                false,
                &run.join("oxo-flow")
            )
            .expect("default off")
            .is_none()
        );
    }

    #[test]
    fn sandbox_conflicts_with_sudo_payload() {
        // `no_new_privs` would make the sudo transition fail mid-spawn; the
        // combination is rejected up front on every platform.
        let mut envs = HashMap::new();
        let run = std::env::temp_dir().join("rb01-sudo");
        let err = configure_with(
            Some("landlock"),
            None,
            None,
            None,
            None,
            &run,
            &mut envs,
            true,
            &run.join("oxo-flow"),
        )
        .expect_err("sudo + sandbox must be rejected");
        assert!(err.contains("sudo"), "{err}");
    }

    #[test]
    fn tmpdir_and_bin_injection_only_when_absent() {
        let mut envs = HashMap::new();
        let run = std::env::temp_dir().join("rb01-inject");
        let bin = std::env::temp_dir().join("rb01-inject-bin");
        std::fs::create_dir_all(&run).ok();
        std::fs::write(&bin, b"stub").ok();
        // Sandbox off → no injection at all (behavior unchanged).
        configure_with(None, None, None, None, None, &run, &mut envs, false, &bin).expect("off");
        assert!(
            !envs.contains_key("OXO_FLOW_TMPDIR"),
            "sandbox-off must not mutate the child env"
        );
        // Sandbox on (off-Linux this still injects before failing closed —
        // the caller discards the env map on Err, so the behavior is only
        // observable on Linux where the plan is actually built).
        #[cfg(target_os = "linux")]
        {
            let mut envs = HashMap::new();
            envs.insert(
                "PATH".to_string(),
                "/private/tools:/usr/bin:/private/bin".to_string(),
            );
            let plan = configure_with(
                Some("landlock"),
                None,
                None,
                None,
                None,
                &run,
                &mut envs,
                false,
                &bin,
            )
            .expect("sandbox plan on a Landlock-capable kernel");
            assert!(plan.is_some());
            assert_eq!(
                envs.get("OXO_FLOW_TMPDIR"),
                Some(
                    &std::fs::canonicalize(run.join(".oxo-flow").join("tmp"))
                        .expect("canonical scratch")
                        .to_string_lossy()
                        .into_owned()
                )
            );
            assert_eq!(
                envs.get("OXO_FLOW_BIN"),
                Some(&bin.to_string_lossy().into_owned())
            );
            let path = envs.get("PATH").expect("sandbox PATH injection");
            assert!(
                path.starts_with("/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"),
                "private PATH entries must follow the granted system trees: {path}"
            );
            assert!(
                path.ends_with("/private/tools:/private/bin"),
                "operator PATH entries remain available behind the system prefix: {path}"
            );
        }
    }
}
