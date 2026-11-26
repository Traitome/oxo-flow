use chrono::Utc;
use regex::Regex;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::OnceLock;
use tokio::process::Command;
use tracing::{error, info, warn};

use crate::broadcast_event_for;
use crate::domains::execution::checkpoint_status;
use crate::workspace::get_run_directory;

/// Locate the `oxo-flow` CLI binary.
///
/// Search order:
///   1. `OXO_FLOW_BIN` environment variable (explicit override)
///   2. `CARGO_BIN_EXE_oxo-flow` (set by cargo test when oxo-flow-cli is a dependency)
///   3. Next to the current executable (same target dir)
///   4. One level above the current executable (cargo test places test binaries in `deps/`)
///   5. Fall back to `"oxo-flow"` (PATH lookup)
pub(crate) fn find_oxo_flow_binary() -> PathBuf {
    if let Ok(path) = std::env::var("OXO_FLOW_BIN") {
        return PathBuf::from(path);
    }
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_oxo-flow")
        && std::path::Path::new(&path).exists()
    {
        return PathBuf::from(path);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        let sibling = exe_dir.join("oxo-flow");
        if sibling.exists() {
            return sibling;
        }
        // When running under `cargo test`, the test binary lives in
        // `target/debug/deps/`, so the actual binary is one level up.
        if let Some(parent) = exe_dir.parent() {
            let in_parent = parent.join("oxo-flow");
            if in_parent.exists() {
                return in_parent;
            }
        }
    }
    PathBuf::from("oxo-flow")
}

/// How the engine binary was located — carried alongside the probed version
/// so operators can tell a deliberate `OXO_FLOW_BIN` pin from a stale PATH
/// fallback without reading the startup log again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinarySource {
    /// `OXO_FLOW_BIN` — explicit operator override.
    EnvOverride,
    /// Any of the target-dir locations (CARGO_BIN_EXE, sibling, parent).
    TargetDir,
    /// Bare `"oxo-flow"` resolved through PATH — the silently-stale case.
    PathLookup,
}

/// One-shot result of the startup `--version` probe (issue #579).
#[derive(Debug, Clone)]
pub(crate) struct EngineProbe {
    /// Version string the engine itself reported (e.g. `"0.20.1"`).
    pub version: String,
    /// Absolute (or PATH-bare) path of the binary that was probed.
    pub binary: String,
    pub source: BinarySource,
}

impl EngineProbe {
    /// Whether the engine's major.minor differs from the server's — the
    /// mismatch class the issue flags as silent semantic drift.
    pub fn major_minor_mismatch(&self, server_version: &str) -> bool {
        let major_minor = |v: &str| -> (u64, u64) {
            let mut parts = v.split('.');
            (
                parts.next().and_then(|p| p.parse().ok()).unwrap_or(0),
                parts.next().and_then(|p| p.parse().ok()).unwrap_or(0),
            )
        };
        major_minor(&self.version) != major_minor(server_version)
    }
}

static ENGINE_PROBE: OnceLock<Option<EngineProbe>> = OnceLock::new();

/// Run `<binary> --version` once and cache the result (issue #579).
///
/// Returns `None` when the binary cannot be spawned or prints nothing
/// parseable — the probe must never block serving; the run log header is
/// the remaining version record in that case.
pub(crate) fn probe_engine_binary(binary: &std::path::Path) -> Option<EngineProbe> {
    let source = if std::env::var_os("OXO_FLOW_BIN").is_some() {
        BinarySource::EnvOverride
    } else if binary.file_name().is_some_and(|n| n == "oxo-flow")
        && binary.parent().is_some_and(|p| !p.as_os_str().is_empty())
    {
        // A path with a real parent directory is one of the target-dir
        // candidates; the bare PATH fallback is the file name alone.
        BinarySource::TargetDir
    } else {
        BinarySource::PathLookup
    };

    let output = std::process::Command::new(binary)
        .arg("--version")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    // The CLI prints `oxo-flow 0.20.1`; take the last whitespace token of
    // the first line so a renamed binary still parses.
    let version: String = text.lines().next()?.split_whitespace().next_back()?.into();
    let looks_like_version = version
        .split('.')
        .take(2)
        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    if !looks_like_version || version == "version" {
        return None;
    }
    Some(EngineProbe {
        version,
        binary: binary.to_string_lossy().into_owned(),
        source,
    })
}

/// Probe once at startup and log the (server, engine) pair.
///
/// Called from `start_background_tasks`, so both serving entry points get
/// it and tests that never call that stay probe-free. Warnings per issue
/// #579: any parse failure, and a PATH-fallback binary older than the
/// server (the resolved-v0.18.0-under-v0.20.1 case) even when the
/// major.minor happens to match.
pub(crate) fn init_engine_version_probe() {
    let server_version = env!("CARGO_PKG_VERSION");
    let binary = find_oxo_flow_binary();
    let probe = ENGINE_PROBE.get_or_init(|| {
        let probed = probe_engine_binary(&binary);
        match &probed {
            Some(p) => {
                info!(
                    server = server_version,
                    engine = %p.version,
                    binary = %p.binary,
                    "engine CLI resolved for run execution"
                );
                let stale_path_fallback = p.source == BinarySource::PathLookup
                    && version_less_than(&p.version, server_version);
                if p.major_minor_mismatch(server_version) || stale_path_fallback {
                    warn!(
                        server = server_version,
                        engine = %p.version,
                        binary = %p.binary,
                        "engine/server version drift: pipelines will run under the engine's semantics — set OXO_FLOW_BIN to pin the expected binary"
                    );
                }
            }
            None => warn!(
                binary = %binary.display(),
                "could not determine engine CLI version (`--version` failed); /api/health will not report engine_version"
            ),
        }
        probed
    });
    // Tests call this with OXO_FLOW_BIN pointing at an echo stub; the probe
    // result was already logged inside get_or_init, nothing more to do.
    let _ = probe;
}

/// Whether version `a` is strictly older than `b` (plain release triples,
/// non-numeric tails ignored) — mirrors the min_version gate semantics.
fn version_less_than(a: &str, b: &str) -> bool {
    fn triple(v: &str) -> [u64; 3] {
        let mut out = [0u64; 3];
        for (i, part) in v.split('.').take(3).enumerate() {
            let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
            out[i] = digits.parse().unwrap_or(0);
        }
        out
    }
    triple(a) < triple(b)
}

/// The cached startup probe, for `/api/health` and `/api/system`.
///
/// `None` before `init_engine_version_probe` ran or when the probe failed.
pub(crate) fn engine_version_probe() -> Option<&'static EngineProbe> {
    ENGINE_PROBE.get().and_then(|probed| probed.as_ref())
}

/// Extract the invalidation summary lines the CLI writes into execution.log
/// (issue #69): the config-change block, input-manifest invalidation lines,
/// and baseline adoption notes. Returns `None` when the log shows no
/// invalidation activity.
///
/// Substring matching is ANSI-escape tolerant: the colored prefixes wrap
/// whole words, so the literal text stays contiguous.
fn extract_invalidation_summary(log: &str) -> Option<String> {
    let mut summary: Vec<&str> = Vec::new();
    let mut in_config_block = false;
    for line in log.lines() {
        if line.contains("Config change:") {
            in_config_block = true;
            summary.push(line);
        } else if in_config_block {
            if line.contains("re-running") {
                // Final line of the block ("→ invalidated N …, re-running …").
                summary.push(line);
                in_config_block = false;
            } else if line.trim().is_empty() {
                // The block ended without a summary line.
                in_config_block = false;
            } else {
                // Key-level lines ("  key: old → new", "(new key)",
                // "rule definition changed: …", "→ invalidated N …").
                summary.push(line);
            }
        } else if line.contains("input changes invalidated")
            || line.contains("recorded baseline input manifests")
        {
            summary.push(line);
        }
    }
    if summary.is_empty() {
        None
    } else {
        Some(summary.join("\n"))
    }
}

/// Extract the CLI dry-run's machine-readable preview from the mixed
/// stdout/stderr log. The banner and human sections interleave with the
/// JSON — scan from the first occurrence of `checkpoint_preview`, back up
/// to the enclosing `{`, and grow the slice until the document parses.
pub fn extract_dry_run_preview(log: &str) -> Option<serde_json::Value> {
    let start = log.find("\"checkpoint_preview\"")?;
    // Back up to the enclosing '{' of the JSON document.
    let object_start = log[..start].rfind('{')?;
    let tail = &log[object_start..];
    // Grow the slice until serde accepts it (handles nested braces).
    let mut end = 1;
    while end <= tail.len() {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&tail[..end])
            && value.get("checkpoint_preview").is_some()
        {
            return Some(value);
        }
        end += 1;
    }
    None
}

/// Map a subprocess exit status to the run status vocabulary.
///
/// The single definition of "did this exit code succeed" — shared by the
/// local finalizer and the remote pull decision, so a cluster run's
/// terminal status can never drift from a local one's (issue #1112).
pub(crate) fn final_status_from_exit(success: bool) -> &'static str {
    if success { "completed" } else { "failed" }
}

/// Wrapper script the CLI runs inside: `f="$1"; shift; "$@"; rc=$?;
/// printf '%s' "$rc" > "$f"; exit "$rc"`.
///
/// `$0` is a dummy, `$1` the exit-record path, and the remaining positional
/// parameters form the real command line. The record lets startup recovery
/// attribute a run honestly after a web-server crash — the same pattern
/// Nextflow uses (`.exitcode` files). Positional forwarding avoids any
/// shell-quoting of user-controlled arguments.
///
/// `pub` so the process-control integration tests can compose the exact
/// same invocation (and the identity guard can recognize it).
pub const EXIT_CODE_WRAPPER_SCRIPT: &str =
    r#"f="$1"; shift; "$@"; rc=$?; printf '%s' "$rc" > "$f"; exit "$rc""#;

/// Lowest data-segment ceiling the executor will install for a web-run
/// workflow (RB-09 / issue #1133).
///
/// The declared memory is a budget for the workflow's *rules*, but the same
/// rlimit is inherited by the `oxo-flow` CLI process itself (thread stacks,
/// DAG/checkpoint state). A declaration below this floor is therefore
/// enforced at the floor: still a hard, observable ceiling, just one wide
/// enough for the engine to run. Declarations at or above it are enforced
/// exactly as declared.
pub const MIN_ENFORCED_MEMORY_MB: u64 = 512;

/// Operator knob (RB-01a / issue #1129): wall-of-CPU ceiling in seconds for
/// a web-spawned run's process tree, installed as `RLIMIT_CPU`.
///
/// Unset (or `0`) installs nothing — today's behavior. A positive value
/// sends `SIGXCPU` (then `SIGKILL` if that is ignored) once the tree has
/// burned that many CPU-seconds, so a runaway rule cannot pin a core of the
/// server forever. This is an operator-wide ceiling, deliberately not a
/// per-workflow declaration: it bounds *any* run a tenant submits.
pub const RLIMIT_CPU_ENV: &str = "OXO_FLOW_WEB_RLIMIT_CPU_SECS";

/// Operator knob (RB-01a / issue #1129): maximum process count for a
/// web-spawned run, installed as `RLIMIT_NPROC`. Unset (or `0`) installs
/// nothing.
///
/// CAVEAT (documented in the web API reference): `RLIMIT_NPROC` counts
/// every process of the child's *real user id*, not just this subtree. When
/// the server runs as a shared account, a low value also caps unrelated
/// processes of that account — set it only on a per-run account, and above
/// the account's steady-state process count.
pub const RLIMIT_NPROC_ENV: &str = "OXO_FLOW_WEB_RLIMIT_NPROC";

/// Engine configuration knobs the web-spawned CLI is allowed to inherit
/// (RB-01a / issue #1129).
///
/// The CLI and core engine read these from their own environment while a run
/// executes (scratch dirs, module cache, hook timeout, disk-pressure knobs,
/// wildcard relaxation, container-registry mirrors, cloud storage, secret
/// references). The tenant of a web run never sets them, so re-admitting the
/// ones the *server operator* exported preserves existing engine behavior
/// across the spawn — none of them carry a server credential. Every entry is
/// an `OXO_*` variable the CLI/core actually reads; `PATH`/`HOME`/`LANG`/
/// `LC_*` and `OXO_FLOW_ENV_PASSTHROUGH` come from the shared rule-env
/// policy and are deliberately absent here.
///
/// Deliberately EXCLUDED (would defeat the isolation): `DATABASE_URL`,
/// `OXO_FLOW_MASTER_KEY`, `ANTHROPIC_*`/`OPENAI_*`/`OLLAMA_*`/`DEEPSEEK_*`
/// provider credentials, `OXO_FLOW_AI_API_KEY`, `OXO_FLOW_MCP_TOKEN*`, and
/// `OXO_FLOW_ADMIN_PASSWORD`/`*_USER_PASSWORD`/`*_VIEWER_PASSWORD`.
const WEB_SPAWN_ENV_ALLOWLIST: &[&str] = &[
    // Scratch space + module cache.
    "OXO_FLOW_TMPDIR",
    "OXO_FLOW_MODULE_CACHE",
    "OXO_FLOW_ENV_STORE",
    // Execution-time limits the operator may have tuned.
    "OXO_FLOW_HOOK_TIMEOUT_SECS",
    "OXO_FLOW_OUTPUT_TAIL_BYTES",
    "OXO_FLOW_STRICT_PLACEHOLDERS",
    "OXO_FLOW_DISK_TICK_SECS",
    "OXO_FLOW_DISK_HOLD_ROUNDS",
    "OXO_FLOW_SPAWN_WATCHDOG_SECS",
    "OXO_ENV_LOCK_TIMEOUT_SECS",
    // Operator passthrough for RULE shells: the CLI child re-reads this
    // variable when building its own allowlist, so without forwarding it
    // an operator's passthrough names reach rule shells on direct CLI runs
    // but silently NOT on web-spawned runs (review of RB-01a, #1241).
    "OXO_FLOW_ENV_PASSTHROUGH",
    // Wildcard relaxation + registry/cloud mirrors (operator-set policy).
    "OXO_FLOW_UNSAFE_WILDCARDS",
    "OXO_REGISTRY_MIRRORS",
    "OXO_S3_FORCE_PATH_STYLE",
    // The S3 storage stack reads endpoint+region as a pair — forwarding
    // only the endpoint strands region-scoped setups on web-spawned runs.
    "AWS_ENDPOINT_URL",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    // Secret references + the sandbox binary pin the CLI resolves by name.
    "OXO_FLOW_SECRET_REFS",
    "OXO_FLOW_BIN",
];

/// Parse an operator rlimit knob. Unset, empty, unparseable, or `0` all mean
/// "unset — install nothing" (today's behavior, the default for both knobs).
/// A malformed value warns and falls back to `0` rather than guessing.
fn parse_rlimit_knob(raw: Option<&str>, name: &str) -> u64 {
    let Some(raw) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return 0;
    };
    match raw.parse::<u64>() {
        Ok(n) => n,
        Err(_) => {
            warn!("{name} value {raw:?} is not a number — ignoring it (no rlimit installed)");
            0
        }
    }
}

/// The operator's `RLIMIT_CPU` ceiling in CPU-seconds (`0` = unset).
pub fn cpu_limit_secs() -> u64 {
    parse_rlimit_knob(
        std::env::var(RLIMIT_CPU_ENV).ok().as_deref(),
        RLIMIT_CPU_ENV,
    )
}

/// The operator's `RLIMIT_NPROC` process ceiling (`0` = unset).
pub fn nproc_limit() -> u64 {
    parse_rlimit_knob(
        std::env::var(RLIMIT_NPROC_ENV).ok().as_deref(),
        RLIMIT_NPROC_ENV,
    )
}

/// Enforce OS resource ceilings on the spawned CLI and every process it
/// spawns (RB-09 / issue #1133; RB-01a / issue #1129).
///
/// A single `pre_exec` installs up to three rlimits, each only when its
/// value is nonzero (so the default — every knob unset — installs nothing
/// and behavior is unchanged):
///
/// - `RLIMIT_DATA` from the run's declared memory (`memory_limit_mb`,
///   floored at [`MIN_ENFORCED_MEMORY_MB`]): bounds the data segment plus
///   anonymous mappings — the memory rules actually allocate — while leaving
///   file-backed mappings (reference/BAM mmaps) alone. Exceeding it makes
///   the allocation fail, so the offending rule dies with a real error
///   instead of the overage being silently counted.
/// - `RLIMIT_CPU` from [`RLIMIT_CPU_ENV`]: caps cumulative CPU-seconds.
/// - `RLIMIT_NPROC` from [`RLIMIT_NPROC_ENV`]: caps process count.
///
/// Linux only: Darwin rejects `RLIMIT_DATA`/`RLIMIT_AS` changes with EINVAL,
/// so there the ceilings remain advisory and this is a no-op (documented in
/// the web API reference). A declared memory of `0` means the workflow
/// claims no memory and nothing is enforced for that dimension.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)] // pre_exec is the only way to install a rlimit before exec
fn apply_run_rlimits(cmd: &mut Command, memory_limit_mb: u64, cpu_secs: u64, nproc: u64) {
    let memory_bytes = (memory_limit_mb > 0).then(|| {
        memory_limit_mb
            .max(MIN_ENFORCED_MEMORY_MB)
            .saturating_mul(1024 * 1024)
    });
    if memory_bytes.is_none() && cpu_secs == 0 && nproc == 0 {
        return; // nothing opted in — install no pre_exec hook at all
    }
    // SAFETY: the closure runs in the child between fork and exec; it only
    // calls setrlimit (async-signal-safe) and touches no shared state. A
    // failure (e.g. an unexpected kernel restriction) must not abort the
    // spawn — the run proceeds with the ceiling unapplied rather than
    // failing for a reason the user cannot act on.
    unsafe {
        cmd.pre_exec(move || {
            if let Some(bytes) = memory_bytes {
                let limit = nix::libc::rlimit {
                    rlim_cur: bytes as nix::libc::rlim_t,
                    rlim_max: bytes as nix::libc::rlim_t,
                };
                let _ = nix::libc::setrlimit(nix::libc::RLIMIT_DATA, &limit);
            }
            if cpu_secs > 0 {
                let limit = nix::libc::rlimit {
                    rlim_cur: cpu_secs as nix::libc::rlim_t,
                    rlim_max: cpu_secs as nix::libc::rlim_t,
                };
                let _ = nix::libc::setrlimit(nix::libc::RLIMIT_CPU, &limit);
            }
            if nproc > 0 {
                let limit = nix::libc::rlimit {
                    rlim_cur: nproc as nix::libc::rlim_t,
                    rlim_max: nproc as nix::libc::rlim_t,
                };
                let _ = nix::libc::setrlimit(nix::libc::RLIMIT_NPROC, &limit);
            }
            Ok(())
        });
    }
}

#[cfg(not(target_os = "linux"))]
fn apply_run_rlimits(_cmd: &mut Command, _memory_limit_mb: u64, _cpu_secs: u64, _nproc: u64) {}

/// Execution options carried from the run request to the CLI subprocess.
#[derive(Debug, Clone, Default)]
pub struct RunFlags {
    /// Preview mode: spawns `oxo-flow dry-run` instead of executing anything.
    pub dry_run: bool,
    /// Continue executing independent rules when one fails (`-k`).
    pub keep_going: bool,
    /// Explicitly requested parallelism (`-j`); `None` keeps the CLI default.
    pub max_jobs: Option<usize>,
    /// Sample filters (`--samples` filter semantics) — restrict execution
    /// to these samples. Supports names, `first:N`, and `ready`, matching
    /// the CLI. Unknown names warn; a selection matching nothing fails the
    /// run instead of executing a phantom sample. Empty = all samples.
    pub samples: Vec<String>,
    /// Explicit target rules (`-t <name>` each). Empty = engine default.
    pub targets: Vec<String>,
    /// Re-run only rules that failed in the previous run (`--resume-failed`).
    /// Used by the web retry path (issue #82 P0-3): plain `run` would skip
    /// failed rules as already-attempted and the retry would be a no-op.
    pub resume_failed: bool,
    /// Force execution ignoring up-to-date checks (`--rerun`). Paired with
    /// resume_failed, this makes the retry actually execute the failed set
    /// (their outputs exist, so freshness alone would skip them again).
    pub rerun: bool,
    /// CLI config overrides appended verbatim as trailing `KEY=VALUE` argv
    /// entries — all override semantics (declared-key gate, canonicalization,
    /// unknown-key warnings) live inside the CLI. Empty = none.
    pub config_overrides: Vec<String>,
    /// Forward `--allow-protected-drift` (issue #935): proceed despite
    /// protected data tree drift instead of refusing at the CLI gate.
    /// Run-only — the dry-run branch never carries it (there is no gate
    /// to bypass in preview). Web retry/resume keep the gate.
    pub allow_protected_drift: bool,
    /// Forward `--provenance`: record output checksums into the checkpoint
    /// so `provenance verify` (and `GET /api/runs/{id}/provenance-verify`)
    /// can later audit output integrity. Run-only — the dry-run branch
    /// never carries it (the CLI rejects the flag for dry-run).
    pub provenance: bool,
    /// Forward `--from-rule <RULE>` (issue #1017): re-run from RULE
    /// downstream, invalidating the transitive-dependent closure so stale
    /// downstream outputs cannot silently pass the freshness gate. When
    /// set, the engine computes the closure in the DAG and forces
    /// re-execution of the whole downstream chain. `None` = full run.
    pub from_rule: Option<String>,
    /// The run's declared total memory in MB (sum of the rules'
    /// `[rules.resources] memory` declarations; `0` = none declared).
    ///
    /// RB-09 (#1133): this is the ceiling the web executor enforces with an
    /// OS rlimit on the spawned CLI's process tree — the number was
    /// previously only counted against the user's quota and never enforced.
    pub memory_limit_mb: u64,
}

/// Build the CLI argument vector for a run (pure — unit-tested).
pub fn build_cli_args(
    workflow_file: &std::path::Path,
    run_dir: &std::path::Path,
    flags: &RunFlags,
) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    if flags.dry_run {
        // dry-run shares --samples/-t semantics with run (issue #79 P2:
        // the web preview dropped them) and emits a machine-readable
        // instance-level preview via --json.
        args.push("dry-run".into());
        args.push("--json".into());
    } else {
        args.push("run".into());
    }
    args.push(workflow_file.as_os_str().to_owned());
    args.push("--workdir".into());
    args.push(run_dir.as_os_str().to_owned());
    if flags.dry_run {
        if !flags.samples.is_empty() {
            // --samples filters to the named subset (the CLI also accepts
            // first:N / ready). --sample would APPEND phantom samples —
            // the issue #79 P1-07 mis-wiring.
            args.push("--samples".into());
            args.push(flags.samples.join(",").into());
        }
        for target in &flags.targets {
            args.push("-t".into());
            args.push(target.into());
        }
        // dry-run shares the --arg KEY=VALUE surface with run (main.rs).
        // Equals form keeps hyphen-leading VALUES out of clap's flag
        // parser — a space-form `--arg -x` would be eaten as a flag and
        // kill the spawn at argv level instead of reaching the CLI's
        // targeted declared-key gate.
        for kv in &flags.config_overrides {
            args.push(format!("--arg={kv}").into());
        }
    } else {
        if flags.keep_going {
            args.push("--keep-going".into());
        }
        if flags.resume_failed {
            args.push("--resume-failed".into());
        }
        if flags.rerun {
            args.push("--rerun".into());
        }
        if flags.allow_protected_drift {
            // Issue #935: the CLI's run gate refuses protected-tree drift;
            // this is the web escape hatch (create requests only — retry
            // and resume keep the gate, matching the CLI's own resume).
            args.push("--allow-protected-drift".into());
        }
        if flags.provenance {
            // Record output checksums into the checkpoint (issue #276) so
            // `provenance verify` (web: GET .../provenance-verify) can later
            // audit output integrity.
            args.push("--provenance".into());
        }
        if let Some(jobs) = flags.max_jobs {
            args.push("-j".into());
            args.push(jobs.to_string().into());
        }
        if !flags.samples.is_empty() {
            args.push("--samples".into());
            args.push(flags.samples.join(",").into());
        }
        for target in &flags.targets {
            args.push("-t".into());
            args.push(target.into());
        }
        if let Some(ref from_rule) = flags.from_rule {
            // Issue #1017: the CLI computes the transitive-dependent closure
            // and invalidates their checkpoint entries, so downstream rules
            // re-execute with the new upstream outputs. This is the same
            // closure computation the CLI's own `--from-rule` path runs —
            // one engine, two surfaces.
            args.push("--from-rule".into());
            args.push(from_rule.into());
        }
        // Config overrides ride the shared `--arg KEY=VALUE` flag in
        // equals form (mirrors the CLI's own flag/positional merge in
        // main.rs); equals form survives hyphen-leading values, and the
        // CLI's declared-key gate and canonicalization still apply.
        for kv in &flags.config_overrides {
            args.push(format!("--arg={kv}").into());
        }
    }
    args
}

/// The exact CLI invocation and the lease guard that monitors it.
///
/// Bundled because the executor entry point is already wide (task identity
/// plus workdir); adding the vector, the memory ceiling, and the guard as
/// three more parameters exceeds clippy's arity bound. The guard is the one
/// created at **claim time** by the request handler.
pub struct RunInvocation {
    /// The complete CLI argument vector (after the binary).
    pub cli_args: Vec<std::ffi::OsString>,
    /// The run's declared total memory (0 = none): the executor turns it
    /// into an OS ceiling on the spawned process tree where the platform
    /// supports one (RB-09 / issue #1133).
    pub memory_limit_mb: u64,
    /// Claim-time lease guard, moved into the spawn task.
    pub monitor: crate::infra::run_lease::RunMonitorGuard,
}

/// Spawns a background task to execute the workflow in a sandboxed workspace.
///
/// `workdir` is the directory the CLI executes in: a persistent per-pipeline
/// directory for saved-pipeline runs (so checkpoint-driven invalidation
/// applies across re-runs, issue #69) or the per-run sandbox for ad-hoc runs.
/// The caller must have already written `workflow.oxoflow` there.
pub fn spawn_background_run(
    run_id: String,
    username: String,
    auth_type: String,
    os_user: String,
    workdir: Option<PathBuf>,
    flags: RunFlags,
    monitor: crate::infra::run_lease::RunMonitorGuard,
) {
    // The CLI invocation is derived from RunFlags + the workflow file the
    // caller wrote into the workdir.
    let run_dir = match workdir.clone() {
        Some(w) => w,
        None => match get_run_directory(&username, &run_id) {
            Ok(d) => d,
            Err(e) => {
                error!("Invalid run directory for {run_id}: {e}");
                return;
            }
        },
    };
    let args = build_cli_args(&run_dir.join("workflow.oxoflow"), &run_dir, &flags);
    let memory_limit_mb = flags.memory_limit_mb;
    // RB-01a (#1129): the CPU/process ceilings are operator-wide knobs, not
    // per-workflow declarations — the spawn body resolves them from the
    // environment, so the argument vector and the caller stay unchanged.
    spawn_background_run_with_args(
        run_id,
        username,
        auth_type,
        os_user,
        workdir,
        RunInvocation {
            cli_args: args,
            memory_limit_mb,
            monitor,
        },
    );
}

/// Spawn a background CLI run with an explicit argument vector (used by
/// the checkpoint-resume path, issue #81 web exposure).
///
/// The operator-wide `RLIMIT_CPU` / `RLIMIT_NPROC` ceilings are resolved
/// from the environment inside the spawn body (0 = install nothing; RB-01a
/// / issue #1129).
pub fn spawn_background_run_with_args(
    run_id: String,
    username: String,
    auth_type: String,
    os_user: String,
    workdir: Option<PathBuf>,
    invocation: RunInvocation,
) {
    tokio::spawn(async move {
        let RunInvocation {
            cli_args,
            memory_limit_mb,
            monitor,
        } = invocation;
        info!("Starting background run {} for user {}", run_id, username);

        // The lease guard was created at claim time (before the handler's
        // pre-spawn staging) and moved here. Taking ownership keeps the
        // run heartbeated for this task's lifetime; dropping it — normal
        // return, early error, cancellation, or panic — stops the
        // heartbeat and leaves the row to the lease TTL.
        let _monitor = monitor;

        let Some(backend) = crate::infra::db::global_backend() else {
            error!(
                "Storage backend unavailable — cannot run workflow '{run_id}' in background; \
                 marking as failed is impossible so it stays 'queued' for manual cleanup"
            );
            return;
        };

        let now = Utc::now();
        let now_str = now.to_rfc3339();
        match backend.mark_run_running(&run_id, &now_str).await {
            Ok(false) => {
                info!(
                    "Run {run_id} left 'queued' before spawn — honouring the earlier terminal state"
                );
                return;
            }
            Err(e) => {
                error!("Failed to update status to running for {run_id}: {e}");
                // Best effort: finalize now so quota is released. If the
                // database is still unreachable this also fails; the lease
                // is no longer renewed (the guard drops on return), so the
                // reaper finalizes the queued row once the TTL expires.
                mark_run_failed(&run_id).await;
                return;
            }
            Ok(true) => {}
        }

        // Broadcast run start event, scoped to the run owner (issue #82
        // P0-5: SSE subscribers only receive their own runs' events).
        //
        // Wire-shape note (#914 review NIT): the `data.username` field
        // predates the canonical user model and actually carries the
        // canonical users.id — identical to the envelope-level `user`
        // field. It is kept for wire compatibility; new consumers should
        // read the envelope `user` instead.
        broadcast_event_for(
            "run_started",
            &serde_json::json!({
                "run_id": run_id,
                "username": username,
                "status": "running",
                "started_at": now.to_rfc3339(),
            }),
            Some(&username),
        );

        let run_dir = match workdir {
            Some(w) => w,
            None => match get_run_directory(&username, &run_id) {
                Ok(d) => d,
                Err(e) => {
                    error!("Invalid run directory for {run_id}: {e}");
                    mark_run_failed(&run_id).await;
                    return;
                }
            },
        };

        // A retried/resumed run may reuse the predecessor's workdir; a
        // leftover `.exit-code` from that predecessor must never attribute
        // this execution (issue #1402 review, HIGH). The mtime check in
        // `read_exit_record_after` is the second line of defence (the
        // record predates this row's `started_at`).
        clear_exit_record(&run_dir);

        // Validate OS username to prevent injection in sudo mode
        let os_user_regex = Regex::new(r"^[a-z_][a-z0-9_-]*[$]?$")
            .expect("Static regex pattern should always compile");
        if auth_type == "sudo" && !os_user_regex.is_match(&os_user) {
            error!("Invalid OS username format: {os_user}");
            mark_run_failed(&run_id).await;
            return;
        }

        let oxo_bin = find_oxo_flow_binary();

        // The caller supplies the exact CLI arguments: normal runs build
        // them from RunFlags + the workflow file; checkpoint resumes pass
        // `resume <checkpoint>` directly (issue #81 web exposure).
        let oxo_args = cli_args;

        // Inner command: the CLI itself, optionally through sudo.
        let mut payload: Vec<std::ffi::OsString> = Vec::new();
        if auth_type == "sudo" && os_user != "oxo-flow" {
            payload.push("sudo".into());
            payload.push("-n".into());
            payload.push("-u".into());
            payload.push(os_user.into());
        }
        // Cloned so the sandbox configuration below can still read the
        // resolved binary path (grants + self-binary injection).
        payload.push(oxo_bin.clone().into_os_string());
        payload.extend(oxo_args);

        // Wrap the payload in `sh -c` so the CLI's exit code is persisted to
        // `<workdir>/.exit-code`. If the web server crashes before reaping
        // the child, startup recovery reads that record instead of blindly
        // marking the run failed (issue #79 P1-02).
        let exit_file = run_dir.join(".exit-code");
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(EXIT_CODE_WRAPPER_SCRIPT);
        cmd.arg("sh");
        cmd.arg(&exit_file);
        cmd.args(payload);

        // Capture output to files in the run directory
        let log_file_path = run_dir.join("execution.log");
        let log_file = match std::fs::File::create(&log_file_path) {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to create log file for run {run_id}: {e}");
                mark_run_failed(&run_id).await;
                return;
            }
        };
        let err_file = match log_file.try_clone() {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to clone log file handle for run {run_id}: {e}");
                mark_run_failed(&run_id).await;
                return;
            }
        };

        // New process group: signals from cancel/pause/resume handlers reach
        // the CLI and every rule subprocess it spawns (see process_control).
        #[cfg(unix)]
        cmd.process_group(0);

        cmd.stdout(Stdio::from(log_file));
        cmd.stderr(Stdio::from(err_file));

        // RB-01a (#1129): the child must NOT inherit the server's full
        // environment. `DATABASE_URL`, `OXO_FLOW_MASTER_KEY`, provider
        // credentials and MCP tokens would otherwise sit in the `environ` of
        // a process whose rule shells are authored by the requesting tenant.
        // Reuse the engine's rule-shell policy verbatim (clear_env +
        // PATH/HOME/LANG/LC_* + OXO_FLOW_ENV_PASSTHROUGH), re-admitting only
        // the engine's own non-secret configuration knobs so an
        // operator-configured server spawns the CLI identically.
        let mut spawn_envs: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for name in WEB_SPAWN_ENV_ALLOWLIST {
            if let Ok(value) = std::env::var(name) {
                spawn_envs.insert((*name).to_string(), value);
            }
        }
        // The operator's passthrough is a name LIST: the CLI child re-reads
        // `OXO_FLOW_ENV_PASSTHROUGH` when building each rule shell's
        // allowlist and then admits those names from ITS OWN ambient — so
        // the named variables themselves must ride along too, or the
        // passthrough contract silently diverges between direct CLI runs
        // and web-spawned runs (review of RB-01a, #1241). Same trust level
        // as CLI-direct: the server operator named them deliberately.
        if let Ok(passthrough) = std::env::var("OXO_FLOW_ENV_PASSTHROUGH") {
            for name in oxo_flow_core::executor::parse_env_passthrough(Some(&passthrough)) {
                if let Ok(value) = std::env::var(&name) {
                    spawn_envs.insert(name, value);
                }
            }
        }

        // RB-01 (#1129): the opt-in OS-level tenant boundary. The sandbox may
        // inject per-run defaults into `spawn_envs` (per-run scratch dir,
        // self-binary pin) and, when enabled, installs a Landlock ruleset in
        // a pre_exec hook so the CLI and every rule shell it spawns can only
        // reach the granted paths. Fail-closed: a configuration or kernel
        // error refuses the spawn — a run never proceeds unsandboxed when the
        // operator asked for the sandbox.
        let sandbox_plan = match crate::sandbox::configure(
            &run_dir,
            &mut spawn_envs,
            auth_type == "sudo",
            &oxo_bin,
        ) {
            Ok(plan) => plan,
            Err(e) => {
                error!(
                    "Sandbox configuration failed for run {run_id} — refusing to spawn \
                     (fail-closed): {e}"
                );
                let _ = std::fs::write(
                    run_dir.join("execution.log"),
                    format!("run failed before spawn: sandbox unavailable: {e}\n"),
                );
                mark_run_failed(&run_id).await;
                return;
            }
        };
        oxo_flow_core::executor::apply_rule_env_allowlist(&mut cmd, &spawn_envs, false);

        // RB-09 (#1133) + RB-01a (#1129): enforce the run's declared memory
        // as an OS ceiling on the CLI and every rule subprocess it spawns,
        // plus the operator's CPU/process ceilings. Applied before the spawn
        // below; a no-op off Linux (see the helper).
        apply_run_rlimits(&mut cmd, memory_limit_mb, cpu_limit_secs(), nproc_limit());

        // RB-01 (#1129): attach the opt-in Landlock sandbox (filesystem +
        // TCP network) to the same pre-exec chain. Off by default; a no-op
        // when `configure` returned None.
        if let Some(plan) = &sandbox_plan {
            plan.attach(&mut cmd);
        }

        // Transient fork failures (EAGAIN/ENOMEM under full-workspace
        // parallel test load, issue #120) must not permanently fail the run
        // — retry briefly before giving up. While retrying, the run row is
        // 'running' with a NULL pid: a cancel landing in that window must
        // win — abort the spawn rather than executing a workflow the user
        // already stopped (issue #136 tier-2: without the check, the retry
        // would eventually spawn and a cancelled run would silently execute).
        //
        // Test seam: OXO_FLOW_TEST_SPAWN_FAIL deterministically fails the
        // first N spawn attempts (synthetic EAGAIN) so the retry contract
        // is exercisable without inducing fork pressure.
        let fail_first: u32 = std::env::var("OXO_FLOW_TEST_SPAWN_FAIL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut spawn_attempts = 0u32;
        let spawn_result = loop {
            if run_is_cancelled(&run_id).await {
                warn!("Run {run_id} cancelled during spawn retries — aborting spawn");
                return;
            }
            let spawned = if spawn_attempts < fail_first {
                Err(std::io::Error::from_raw_os_error(nix::libc::EAGAIN))
            } else {
                cmd.spawn()
            };
            match spawned {
                Err(e)
                    if matches!(
                        e.raw_os_error(),
                        Some(nix::libc::EAGAIN) | Some(nix::libc::ENOMEM)
                    ) && spawn_attempts < 3 =>
                {
                    spawn_attempts += 1;
                    warn!(
                        "Transient spawn failure for run {run_id} ({e}); \
                         retrying ({spawn_attempts}/3)"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
                other => break other,
            }
        };
        match spawn_result {
            Ok(mut child) => {
                // Register the process group FIRST so a cancel racing the
                // spawn can signal it, then persist the pid for crash
                // recovery.
                if let Some(pid) = child.id() {
                    crate::process_control::register(&run_id, pid as i32);
                }
                if run_is_cancelled(&run_id).await {
                    // The cancel won the race against the final spawn
                    // attempt: nothing was registered at cancel time, so the
                    // group was never signalled — kill it here. A cancelled
                    // run must not execute (issue #136 tier-2).
                    info!("Run {run_id} cancelled while spawning — killing the late-spawned group");
                    if let Some(pid) = child.id() {
                        let _ = crate::process_control::signal_group(
                            pid as i32,
                            crate::process_control::SIGTERM,
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        let _ = crate::process_control::signal_group(
                            pid as i32,
                            crate::process_control::SIGKILL,
                        );
                    }
                }
                if let Some(pid) = child.id()
                    && let Some(backend) = crate::infra::db::global_backend()
                    && let Err(e) = backend.update_run_pid(&run_id, pid as i64).await
                {
                    warn!("Failed to record PID for run {run_id}: {e}");
                }

                // Real resource telemetry (issue #82 P1-2): sample the
                // CLI's process tree every 5s into workdir/metrics.jsonl so
                // the monitor cards show measured memory/CPU, not defaults.
                if let Some(pid) = child.id() {
                    spawn_resource_sampler(run_id.clone(), pid, run_dir.clone());
                }
                spawn_log_tailer(run_id.clone(), run_dir.clone());

                // Wait for process completion
                match child.wait().await {
                    Ok(status) => {
                        // Keep `.exit-code` until the terminal write succeeds:
                        // if the database is unavailable, the reaper retries
                        // from the record instead of losing the true outcome.
                        finalize_run(&run_id, status.code(), &log_file_path).await;
                    }
                    Err(e) => {
                        crate::process_control::unregister(&run_id);
                        error!("Failed to wait on child process for run {run_id}: {e}");
                        mark_run_failed(&run_id).await;
                    }
                }
            }
            Err(e) => {
                error!(
                    "Failed to spawn child process for run {run_id} \
                     after {spawn_attempts} retries: {e}"
                );
                // A pre_exec sandbox failure occurs before the child can
                // write to stdout/stderr. Persist the spawn error in the run
                // log so operators can diagnose fail-closed startup failures.
                if let Ok(mut log) = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&log_file_path)
                {
                    use std::io::Write as _;
                    let _ = writeln!(log, "run failed before child execution: {e}");
                }
                // A cancel that landed during the retry window is the
                // terminal truth — never overwrite 'cancelled' with a
                // fabricated 'failed' (issue #136 tier-2).
                if !run_is_cancelled(&run_id).await {
                    mark_run_failed(&run_id).await;
                }
            }
        }
    });
}

/// Persist the terminal state of a finished run and broadcast the SSE event.
///
/// Shared by the live wait path and crash-recovery monitoring so both
/// attribute runs with identical semantics. `exit_code` is the CLI's exit
/// code; `None` means the process was killed without leaving a record and is
/// attributed as failed. A run already marked 'cancelled' is left untouched:
/// the terminal write happened at cancel time and the kill fallout must not
/// overwrite it back to completed/failed.
/// True when the run row has been cancelled — the terminal state that must
/// never be overwritten nor executed past (a cancel is the user's last
/// word; the kill fallout must not rewrite it).
async fn run_is_cancelled(run_id: &str) -> bool {
    match crate::infra::db::global_backend() {
        Some(backend) => backend.is_run_cancelled(run_id).await.unwrap_or(false),
        None => false,
    }
}

/// Maximum accepted size of a wrapper `.exit-code` record.
///
/// The workdir is rule-writable; a rule could leave a huge or special file
/// at the path. A valid record is an exit code and a newline.
const EXIT_RECORD_MAX_BYTES: u64 = 64;

/// Filesystem mtime granularity allowance when comparing an exit record to
/// the run's `started_at` (the wrapper can write in the same clock tick the
/// run started).
const EXIT_RECORD_MTIME_TOLERANCE_MS: i64 = 2_000;

/// Bound on the finalization-time log read and preview write.
///
/// Both steps precede the terminal DB commit (the dry-run preview must exist
/// before the run is observable as terminal), so a hung workdir must not be
/// able to park the run forever. A 16 MiB local read and a KB-scale write
/// are well under a second; on expiry the preview and invalidation summary
/// are skipped with a warning and the terminal write proceeds.
const FINALIZE_PREVIEW_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Atomically persist the dry-run preview in `dir`.
///
/// Writes a uniquely named temp file in the same directory, then renames it
/// over `dry-run-preview.json`. Readers therefore observe either the old
/// document or the complete new one, never a truncated write — which matters
/// because the preview write precedes the terminal commit and a competing
/// finalizer may write the same path. The unique temp name also keeps a
/// planted symlink or FIFO in the rule-writable workdir from being opened.
///
/// A caller-side timeout may drop this future mid-write, leaving the hidden
/// temp file behind; it is never renamed into place, so the endpoint still
/// sees the previous complete document or none at all.
async fn persist_preview_atomically(run_id: &str, dir: &std::path::Path, json: String) {
    let final_path = dir.join("dry-run-preview.json");
    let tmp_path = dir.join(format!(
        ".dry-run-preview.json.{}.{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    if let Err(e) = tokio::fs::write(&tmp_path, json).await {
        // Not fatal: the run's state and log are unaffected, but the
        // preview endpoint will answer NO_PREVIEW for this run.
        tracing::warn!(
            "Run {run_id}: failed to write dry-run preview temp file {}: {e}",
            tmp_path.display()
        );
        // `fs::write` can fail after creating the file (partial write).
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return;
    }
    if let Err(e) = tokio::fs::rename(&tmp_path, &final_path).await {
        tracing::warn!("Run {run_id}: failed to persist dry-run preview: {e}");
        let _ = tokio::fs::remove_file(&tmp_path).await;
    }
}

/// Read the wrapper's `.exit-code` record, accepting it only when it is a
/// regular file written for **this** execution.
///
/// The record is a stale artefact when a retried/resumed run reuses a
/// predecessor's workdir: the old `.exit-code` would otherwise attribute a
/// never-executed run as `completed`. Three guards:
///
/// - the run must have a `started_at` (a `queued` row never ran, so it is
///   never attributed from a file);
/// - the record's mtime must not predate `started_at` (2 s tolerance for
///   filesystem granularity);
/// - the path must be a regular file of bounded size — checked with
///   `symlink_metadata` (no-follow) before a capped read. A writer-less FIFO
///   would block a plain read (issue #735); a symlink is rejected.
///
/// This is a read-only check: the record is removed only after a terminal
/// write succeeds (`finalize_run`), so a competing reaper or a transient
/// database error cannot discard the outcome. Callers on a hot async path
/// (the reaper) wrap it in `spawn_blocking` because the workdir may be on a
/// slow shared filesystem; startup recovery runs it synchronously before
/// serving.
pub(crate) fn read_exit_record_after(workdir: &std::path::Path, started_at: &str) -> Option<i32> {
    let started_ms = chrono::DateTime::parse_from_rfc3339(started_at)
        .ok()?
        .timestamp_millis();
    let path = workdir.join(".exit-code");
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.file_type().is_file() || meta.len() > EXIT_RECORD_MAX_BYTES {
        return None;
    }
    let modified_ms = meta
        .modified()
        .ok()?
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    if modified_ms + EXIT_RECORD_MTIME_TOLERANCE_MS < started_ms {
        return None;
    }
    use std::io::Read as _;
    let mut text = String::new();
    std::fs::File::open(&path)
        .ok()?
        .take(EXIT_RECORD_MAX_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    text.trim().parse().ok()
}

/// Delete the wrapper's `.exit-code` record (best-effort).
///
/// Called before every spawn (a reused workdir must not carry a predecessor
/// record into the new execution) and by `finalize_run` after a terminal
/// write succeeds.
pub(crate) fn clear_exit_record(workdir: &std::path::Path) {
    let path = workdir.join(".exit-code");
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warn!(
            "Failed to clear stale .exit-code at {}: {e}",
            path.display()
        ),
    }
}

pub(crate) async fn finalize_run(run_id: &str, exit_code: Option<i32>, log_path: &std::path::Path) {
    crate::infra::run_lease::unregister_monitored(run_id);
    crate::process_control::unregister(run_id);
    let success = exit_code == Some(0);
    let final_state = final_status_from_exit(success);
    // The wrapper record lives beside the log; remove it only after a
    // terminal write succeeds (or the row is already terminal). A failed
    // write leaves it for the reaper's next attempt.
    let exit_dir = log_path.parent().map(std::path::Path::to_path_buf);

    // Release the run's quota reservation on EVERY terminal path — the
    // cancelled early-return below must not leak the reservation (issue:
    // cancelled runs burned active_runs/threads/memory until restart).
    crate::infra::quota::release_and_complete(run_id).await;

    if run_is_cancelled(run_id).await {
        info!("Run {run_id} exited after cancel; keeping 'cancelled'");
        if let Some(dir) = &exit_dir {
            clear_exit_record(dir);
        }
        return;
    }

    let end = Utc::now();
    let end_str = end.to_rfc3339();
    // One backend for the whole helper: the terminal write and the owner
    // lookup below would otherwise re-enter global_backend() and re-derive
    // the same value (OnceLock makes it cheap, but the dual paths forced a
    // success flag just to tell "backend missing" from "write failed").
    let Some(backend) = crate::infra::db::global_backend() else {
        error!(
            "Storage backend unavailable — cannot finalize \
             run {run_id} state in the database"
        );
        return;
    };
    // Test seam: force the terminal write to fail so a test can prove the
    // wrapper record survives for the reaper's retry (issue #1402 review).
    if std::env::var("OXO_FLOW_TEST_FINALIZE_TERMINAL_FAIL").is_ok_and(|v| v == "1") {
        error!("Run {run_id}: forced terminal write failure (test seam)");
        return;
    }
    // Read the log once, bounded to the head window: both consumers below
    // (dry-run preview JSON, invalidation summary) print at run start,
    // before any rule executes — a tail window would miss them, and the
    // old whole-file read buffered a multi-GB log at every run's terminal
    // path (issue #734). The read now precedes the terminal commit (so the
    // preview exists before the run is observable as terminal), which makes
    // a pathological workdir able to delay the commit — bound the wait and
    // proceed without the preview/summary instead of parking the run.
    let head_path = log_path.to_path_buf();
    let log_content = match tokio::time::timeout(
        FINALIZE_PREVIEW_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            checkpoint_status::read_head_bounded(&head_path, 16 * 1024 * 1024)
        }),
    )
    .await
    {
        Ok(joined) => joined.unwrap_or_default(),
        Err(_) => {
            tracing::warn!(
                "Run {run_id}: timed out after {}s reading the log head; \
                 the dry-run preview and invalidation summary are unavailable",
                FINALIZE_PREVIEW_TIMEOUT.as_secs()
            );
            String::new()
        }
    };
    let log = (!log_content.is_empty()).then_some(log_content);
    // Persist the dry-run preview (instance-level plan) next to the log so
    // /api/runs/{id}/preview can serve it without re-parsing the log.
    //
    // Written BEFORE the terminal row is committed: a client that observes
    // the run as terminal must be able to read the preview immediately.
    // The reverse order raced — the terminal row was visible while the
    // preview file was still being written, so an immediate GET could
    // answer NO_PREVIEW for a completed dry-run under load.
    //
    // The write is an atomic temp-file replace: a losing finalizer (the
    // already-terminal `Ok(false)` path) or a concurrent GET must never
    // observe a truncated document, and rename replaces a planted symlink
    // instead of writing through it.
    if let Some(dir) = log_path.parent()
        && let Some(preview) = log.as_deref().and_then(extract_dry_run_preview)
    {
        match serde_json::to_string_pretty(&preview) {
            Ok(json) => {
                if tokio::time::timeout(
                    FINALIZE_PREVIEW_TIMEOUT,
                    persist_preview_atomically(run_id, dir, json),
                )
                .await
                .is_err()
                {
                    tracing::warn!(
                        "Run {run_id}: timed out persisting the dry-run preview; \
                         the terminal write proceeds"
                    );
                }
            }
            Err(e) => {
                tracing::warn!("Run {run_id}: failed to serialize dry-run preview: {e}");
            }
        }
    }
    match backend
        .record_run_terminal(run_id, final_state, final_state, &end_str)
        .await
    {
        Ok(true) => {
            if let Some(dir) = &exit_dir {
                clear_exit_record(dir);
            }
        }
        Ok(false) => {
            // A terminal state (cancel, or a competing finalizer) landed
            // first. Broadcasting a completion/failure for it would lie.
            // The record is no longer needed: the row is terminal.
            info!("Run {run_id} is already terminal; keeping its persisted state");
            if let Some(dir) = &exit_dir {
                clear_exit_record(dir);
            }
            return;
        }
        Err(e) => {
            // Write failure is logged but not fatal: the run's output is
            // done, so the remaining terminal bookkeeping (broadcast,
            // webhook) still runs — only the DB row is stale. The wrapper
            // record is deliberately left in place so the reaper can
            // attribute the true outcome on its next sweep.
            error!("Failed to update final status for run {run_id} via backend: {e}");
        }
    }
    info!("Run {run_id} finished: {final_state}");

    // Broadcast the terminal event (documented in the SSE API):
    // run_completed on success, run_failed otherwise.
    let event = if success {
        "run_completed"
    } else {
        "run_failed"
    };
    // Surfacing the CLI's invalidation summary (issue #69): config changes,
    // rule-definition edits, and input-set changes that invalidated
    // checkpoint records this run.
    let summary = log.as_deref().and_then(extract_invalidation_summary);
    // Scope the terminal event to the run owner (issue #82 P0-5); the
    // backend resolved above serves this lookup too.
    let user_id: Option<String> = backend
        .get_run(run_id)
        .await
        .ok()
        .flatten()
        .map(|r| r.user_id);
    broadcast_event_for(
        event,
        &serde_json::json!({
            "run_id": run_id,
            "status": final_state,
            "finished_at": end.to_rfc3339(),
            "summary": summary,
        }),
        user_id.as_deref(),
    );

    // Configured webhooks fire on terminal states (issue #82 P1-12).
    crate::domains::observability::webhook::notify_terminal(run_id, final_state).await;
}

/// Monitor a run that was re-attached after a server restart (crash
/// recovery, issue #79 P1-02).
///
/// The web server is no longer the CLI's parent, so completion is detected
/// by polling the `.exit-code` record (written by the wrapper shell the
/// moment the CLI exits — the primary signal) and process liveness (the
/// fallback for a process killed without a record). Finalization goes
/// through `finalize_run` so semantics match the live wait path exactly.
pub fn resume_monitoring(run_id: String, pid: i32, workdir: PathBuf, started_at: Option<String>) {
    // Crash-recovery re-attach also samples resources and tails the log
    // (issue #82 P1-2 / P1-18).
    if pid > 0 {
        spawn_resource_sampler(run_id.clone(), pid as u32, workdir.clone());
    }
    spawn_log_tailer(run_id.clone(), workdir.clone());
    tokio::spawn(async move {
        // Re-attached runs are monitored by this task, so they heartbeat
        // (issue #1402 review, MEDIUM); the guard unregisters on exit.
        let _monitor = crate::infra::run_lease::RunMonitorGuard::new(run_id.clone());
        let log_path = workdir.join("execution.log");
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            // Use the same hardened reader as the reaper: no-follow
            // metadata, size cap, and the mtime-vs-`started_at` check. A
            // predecessor's stale record in a reused workdir is ignored
            // (the live CLI will overwrite it when it exits); never blindly
            // delete the file — the CLI may have just written the live
            // execution's true outcome (issue #1402 review, LOW).
            let code = match started_at.as_deref() {
                Some(started) => {
                    let workdir = workdir.clone();
                    let started = started.to_string();
                    tokio::task::spawn_blocking(move || read_exit_record_after(&workdir, &started))
                        .await
                        .ok()
                        .flatten()
                }
                None => None,
            };
            if let Some(code) = code {
                finalize_run(&run_id, Some(code), &log_path).await;
                break;
            }
            if !crate::process_control::probe_alive(pid) {
                // Killed without leaving a record (SIGKILL to the group).
                finalize_run(&run_id, None, &log_path).await;
                break;
            }
        }
    });
}

/// Tail `workdir/execution.log` and broadcast per-rule SSE events
/// (issue #82 P1-18: previously only terminal events existed and the
/// frontend polled to fill the gap). Parses the CLI's stable log lines:
/// `Running: <rule>` / `✓ <rule>` / `✗ rule '<rule>' failed` /
/// `⊝ <rule>` (skipped).
fn spawn_log_tailer(run_id: String, workdir: PathBuf) {
    tokio::spawn(async move {
        let backend = match crate::infra::db::global_backend() {
            Some(backend) => backend,
            None => {
                warn!("Storage backend unavailable — log tailer disabled for run {run_id}");
                return;
            }
        };
        let log_path = workdir.join("execution.log");
        let mut offset: u64 = 0;
        let user_id: Option<String> = backend
            .get_run(&run_id)
            .await
            .ok()
            .flatten()
            .map(|r| r.user_id);
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(1500));
        ticker.tick().await; // fire immediately
        loop {
            ticker.tick().await;
            // The backend was resolved once at spawn; per-tick re-entry would
            // re-read the OnceLock every 1.5 s for the same value.
            let active: Option<String> = backend
                .get_run(&run_id)
                .await
                .ok()
                .flatten()
                .map(|r| r.status);
            match active.as_deref() {
                Some("running") | Some("paused") | Some("queued") => {}
                _ => break,
            }
            drain_log_events(&log_path, &mut offset, &run_id, user_id.as_deref()).await;
        }
        // The run just reached a terminal state, but the CLI may have
        // narrated rule completions after the final 1.5 s poll (the live
        // check showed the last rule_completed dropped when the tailer
        // exited on the status flip). Drain once more before exiting.
        drain_log_events(&log_path, &mut offset, &run_id, user_id.as_deref()).await;
    });
}

/// Read the run log from `offset` and broadcast any rule events found.
async fn drain_log_events(
    log_path: &std::path::Path,
    offset: &mut u64,
    run_id: &str,
    user_id: Option<&str>,
) {
    let Ok(meta) = tokio::fs::metadata(log_path).await else {
        return;
    };
    let len = meta.len();
    if len < *offset {
        *offset = 0; // log rotated/truncated
    }
    if len == *offset {
        return;
    }
    let Ok(mut file) = tokio::fs::File::open(log_path).await else {
        return;
    };
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    if file.seek(std::io::SeekFrom::Start(*offset)).await.is_err() {
        return;
    }
    // Bound the per-tick delta: a rule that emitted hundreds of MB
    // between ticks must not buffer all of it for SSE parsing
    // (issue #734). Anything past the cap is skipped — the next
    // tick resumes from the new offset.
    const MAX_TAILER_TICK_BYTES: u64 = 4 * 1024 * 1024;
    let mut buf = String::new();
    if file
        .take(MAX_TAILER_TICK_BYTES)
        .read_to_string(&mut buf)
        .await
        .is_err()
    {
        return;
    }
    *offset = len;
    for line in buf.lines() {
        let event = parse_rule_event(line);
        if let Some((event_type, rule)) = event
            && !rule.is_empty()
        {
            broadcast_event_for(
                event_type,
                &serde_json::json!({"run_id": run_id, "rule": rule}),
                user_id,
            );
        }
    }
}

/// Map one run-log line to an SSE rule event.
///
/// Mirrors the exact formats `progress_narrate` writes into the run log
/// (crates/oxo-flow-cli/src/commands/run/task.rs and scheduler_loop.rs):
/// the lines carry a two-space indent, a rule-completion line is always
/// `✓ {rule} ({:.1}s)`, and `✗ rule '<name>' failed: …` wraps the rule in
/// quotes — all ANSI-stripped by the CLI before the line reaches the log.
fn parse_rule_event(line: &str) -> Option<(&'static str, String)> {
    let line = line.trim_start();
    if let Some(rest) = line.strip_prefix("Running: ") {
        return Some(("rule_started", rest.trim().to_string()));
    }
    if let Some(rest) = line.strip_prefix('✓') {
        // The duration suffix is always present on rule completion; other
        // ✓ narrations (report snapshot written, output files verified) are
        // not rules and must not become rule_completed events.
        let rest = rest.trim();
        if let Some((name, duration)) = rest.rsplit_once(" (")
            && duration
                .strip_suffix("s)")
                .and_then(|d| d.trim().parse::<f64>().ok())
                .is_some()
        {
            return Some(("rule_completed", name.trim().to_string()));
        }
        return None;
    }
    if line.starts_with('⊝') {
        return Some((
            "rule_skipped",
            line.trim_start_matches('⊝').trim().to_string(),
        ));
    }
    line.strip_prefix("✗ rule '")
        .and_then(|l| l.split_once("'"))
        .map(|(rule, _)| ("rule_failed", rule.to_string()))
}

/// Sample the run's process-tree memory/CPU every 5 seconds into
/// `workdir/metrics.jsonl` (issue #82 P1-2: real telemetry behind the
/// monitor trend cards, replacing the fabricated defaults). Stops when the
/// run reaches a terminal DB state.
fn spawn_resource_sampler(run_id: String, cli_pid: u32, workdir: PathBuf) {
    tokio::spawn(async move {
        let backend = match crate::infra::db::global_backend() {
            Some(backend) => backend,
            None => {
                warn!("Storage backend unavailable — resource sampler disabled for run {run_id}");
                return;
            }
        };
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
        ticker.tick().await; // fire immediately, then every 5s
        // The sampler is the only writer of metrics.jsonl, so counting the
        // pre-existing lines once lets append_metrics skip the O(file)
        // read+rewrite on every tick and only trim at the 2000-line cap.
        let mut metrics_lines: usize = tokio::fs::read_to_string(workdir.join("metrics.jsonl"))
            .await
            .map(|content| content.lines().count())
            .unwrap_or(0);
        // The workdir is rule-writable: a pre-planted FIFO at metrics.jsonl
        // must not hang the sampler's opens below (issue #735) — the file
        // is only ever written by this sampler, so bail if it exists as a
        // non-regular file.
        if tokio::fs::metadata(workdir.join("metrics.jsonl"))
            .await
            .map(|m| !m.is_file())
            .unwrap_or(false)
        {
            warn!(
                "metrics.jsonl in run workdir {workdir:?} is not a regular file — resource sampling disabled for run {run_id}"
            );
            return;
        }
        loop {
            ticker.tick().await;
            // Resolved once at spawn (same backend as the log tailer).
            let active: Option<String> = backend
                .get_run(&run_id)
                .await
                .ok()
                .flatten()
                .map(|r| r.status);
            match active.as_deref() {
                Some("running") | Some("paused") | Some("queued") => {}
                _ => break,
            }
            if let Some((memory_mb, cpu_pct, processes)) = crate::sys::process_tree_usage(cli_pid) {
                let line = format!(
                    r#"{{"ts":"{}","memory_mb":{:.1},"cpu_pct":{:.1},"processes":{}}}"#,
                    Utc::now().to_rfc3339(),
                    memory_mb,
                    cpu_pct,
                    processes
                );
                metrics_lines = append_metrics(&workdir, &line, metrics_lines).await;
            }
        }
    });
}

/// Append one sample to `workdir/metrics.jsonl`, keeping at most the last
/// 2000 lines (≈2.7 h at 5 s ticks) so long runs cannot grow the file
/// unboundedly. Returns the number of lines now in the file.
///
/// `existing_lines` is the caller's running count (the sampler is the sole
/// writer): below the cap this is an O(1) append; the read +
/// truncate-rewrite only runs once the cap is actually reached.
async fn append_metrics(workdir: &std::path::Path, line: &str, existing_lines: usize) -> usize {
    const MAX_METRICS_LINES: usize = 2000;
    let path = workdir.join("metrics.jsonl");

    // Gate every tick, not just sampler startup: a rule could swap a
    // non-regular file in between the stat and the open (issue #735).
    if tokio::fs::metadata(&path)
        .await
        .map(|m| !m.is_file())
        .unwrap_or(false)
    {
        return existing_lines;
    }

    if existing_lines < MAX_METRICS_LINES {
        use tokio::io::AsyncWriteExt;
        // One retry absorbs transient open failures under parallel load
        // (CI flake: append_metrics_appends_below_cap saw the second
        // append silently dropped — the old code swallowed open errors).
        for attempt in 0..2 {
            let Ok(mut file) = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await
            else {
                if attempt == 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    continue;
                }
                tracing::warn!(
                    "failed to open metrics file {} for append — dropping one metrics line",
                    path.display()
                );
                return existing_lines;
            };
            return if file.write_all(format!("{line}\n").as_bytes()).await.is_ok() {
                existing_lines + 1
            } else {
                existing_lines
            };
        }
        unreachable!("the loop always returns");
    }

    // Over the cap: trim to the newest MAX_METRICS_LINES and rewrite once.
    let content = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    let mut lines: Vec<&str> = content.lines().collect();
    lines.push(line);
    let keep = if lines.len() > MAX_METRICS_LINES {
        &lines[lines.len() - MAX_METRICS_LINES..]
    } else {
        &lines[..]
    };
    let _ = tokio::fs::write(&path, format!("{}\n", keep.join("\n"))).await;
    keep.len()
}

/// Mark a run as failed with the current timestamp.
async fn mark_run_failed(run_id: &str) {
    let end = Utc::now();
    let end_str = end.to_rfc3339();
    // One backend for the whole helper, mirroring finalize_run: the
    // unavailable path still releases the in-memory quota reservation so a
    // missing pool can't pin resources until restart.
    let Some(backend) = crate::infra::db::global_backend() else {
        error!("Storage backend unavailable — cannot mark run {run_id} failed");
        crate::infra::quota::release_and_complete(run_id).await;
        return;
    };

    match backend
        .record_run_terminal(run_id, "failed", "failed", &end_str)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            // The run already reached a terminal state (e.g. the user's
            // cancel won the race) — broadcasting a failure would lie.
            info!("Run {run_id} is already terminal; not marking it failed");
            crate::infra::quota::release_and_complete(run_id).await;
            return;
        }
        Err(e) => {
            // Write failure is logged but not fatal — the broadcast below
            // still tells the owner the run died.
            error!("Failed to mark run {run_id} as failed via backend: {e}");
        }
    }

    // Broadcast run failure event, scoped to the run owner (issue #82 P0-5).
    let user_id: Option<String> = backend
        .get_run(run_id)
        .await
        .ok()
        .flatten()
        .map(|r| r.user_id);
    broadcast_event_for(
        "run_failed",
        &serde_json::json!({
            "run_id": run_id,
            "status": "failed",
            "finished_at": end.to_rfc3339(),
        }),
        user_id.as_deref(),
    );

    crate::infra::quota::release_and_complete(run_id).await;
    crate::domains::observability::webhook::notify_terminal(run_id, "failed").await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reused workdir carries its predecessor's `.exit-code`: the record
    /// predates this run's `started_at` and must not attribute it.
    #[test]
    fn exit_record_predating_the_run_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".exit-code"), "0").expect("stale record");
        let started_at = (Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
        assert_eq!(
            read_exit_record_after(dir.path(), &started_at),
            None,
            "a record older than the run start belongs to a previous execution"
        );
    }

    #[test]
    fn exit_record_after_start_is_accepted_and_consumed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let record = dir.path().join(".exit-code");
        std::fs::write(&record, "0").expect("fresh record");
        let started_at = (Utc::now() - chrono::Duration::minutes(1)).to_rfc3339();
        assert_eq!(read_exit_record_after(dir.path(), &started_at), Some(0));
        assert!(
            record.exists(),
            "attribution is read-only; the record is cleared only after a terminal write"
        );

        clear_exit_record(dir.path());
        assert!(
            !record.exists(),
            "clearing removes the record so a later run cannot reuse it"
        );
    }

    #[test]
    fn exit_record_rejects_oversized_and_non_regular_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let record = dir.path().join(".exit-code");
        let started_at = (Utc::now() - chrono::Duration::minutes(1)).to_rfc3339();

        // A 100-byte record parses as 0 after trim but exceeds the size cap.
        std::fs::write(&record, format!("0{}", " ".repeat(120))).expect("oversized record");
        assert_eq!(read_exit_record_after(dir.path(), &started_at), None);

        // A symlink is rejected by the no-follow metadata check (unix).
        #[cfg(unix)]
        {
            let target = dir.path().join("real-exit");
            std::fs::write(&target, "0").expect("target");
            std::fs::remove_file(&record).expect("remove oversized record");
            std::os::unix::fs::symlink(&target, &record).expect("symlink");
            assert_eq!(
                read_exit_record_after(dir.path(), &started_at),
                None,
                "a symlink must not be followed"
            );
        }
    }

    #[tokio::test]
    async fn wrapper_script_records_exit_code() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exit_file = dir.path().join(".exit-code");
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(EXIT_CODE_WRAPPER_SCRIPT);
        cmd.arg("sh");
        cmd.arg(&exit_file);
        cmd.arg("/bin/sh").arg("-c").arg("exit 3");
        let status = cmd.status().await.expect("wrapper runs");
        assert_eq!(status.code(), Some(3));
        assert_eq!(
            std::fs::read_to_string(&exit_file).unwrap().trim(),
            "3",
            "exit record must carry the CLI's exit code"
        );
    }

    #[tokio::test]
    async fn wrapper_script_forwards_arguments_positionally() {
        // Metacharacters must survive the wrapper untouched — that is the
        // point of positional ("$@") forwarding.
        let dir = tempfile::tempdir().expect("tempdir");
        let exit_file = dir.path().join(".exit-code");
        let value = "a b'c\"d $HOME; rm -rf /";
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(EXIT_CODE_WRAPPER_SCRIPT);
        cmd.arg("sh");
        cmd.arg(&exit_file);
        cmd.arg("/bin/echo").arg(value);
        let out = cmd.output().await.expect("wrapper runs");
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), value);
        assert_eq!(std::fs::read_to_string(&exit_file).unwrap().trim(), "0");
    }

    #[test]
    fn build_cli_args_includes_samples_and_targets() {
        let flags = RunFlags {
            dry_run: false,
            resume_failed: false,
            rerun: false,
            keep_going: true,
            max_jobs: Some(4),
            samples: vec!["S1".into(), "S2".into()],
            targets: vec!["align".into()],
            config_overrides: vec!["min_quality=30".into()],
            allow_protected_drift: false,
            provenance: false,
            from_rule: None,
            memory_limit_mb: 0,
        };
        let args = build_cli_args(
            std::path::Path::new("wf.oxoflow"),
            std::path::Path::new("dir"),
            &flags,
        );
        let strs: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(strs[0], "run");
        assert!(strs.contains(&"--keep-going".to_string()));
        assert!(strs.contains(&"--samples".to_string()));
        assert!(
            strs.contains(&"S1,S2".to_string()),
            "samples join into one --samples value: {strs:?}"
        );
        assert!(strs.contains(&"-t".to_string()));
        assert!(strs.contains(&"align".to_string()));
        // Config overrides ride the shared `--arg KEY=VALUE` flag in equals
        // form in both branches — the CLI's declared-key gate still applies
        // at run time, and equals form survives hyphen-leading values.
        assert!(strs.contains(&"--arg=min_quality=30".to_string()));
        // dry-run shares --samples/-t (issue #79 P2: the preview dropped
        // them) and asks for the machine-readable instance-level plan,
        // but omits execution-only flags (-j / --keep-going).
        let dry = RunFlags {
            dry_run: true,
            resume_failed: false,
            rerun: false,
            ..flags
        };
        let args = build_cli_args(std::path::Path::new("w"), std::path::Path::new("d"), &dry);
        let strs: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(strs[0], "dry-run");
        assert!(strs.contains(&"--json".to_string()));
        assert!(strs.contains(&"--samples".to_string()));
        assert!(strs.contains(&"S1,S2".to_string()));
        assert!(strs.contains(&"-t".to_string()));
        assert!(strs.contains(&"align".to_string()));
        assert!(!strs.contains(&"--keep-going".to_string()));
        assert!(!strs.contains(&"-j".to_string()));
        // dry-run also carries --arg passthrough (main.rs dry-run shares
        // the --arg flag), so the override reaches the preview too.
        assert!(strs.contains(&"--arg=min_quality=30".to_string()));
    }

    #[test]
    fn build_cli_args_empty_overrides_emit_nothing() {
        let flags = RunFlags {
            config_overrides: vec![],
            ..RunFlags::default()
        };
        let args = build_cli_args(
            std::path::Path::new("wf.oxoflow"),
            std::path::Path::new("dir"),
            &flags,
        );
        assert!(
            !args
                .iter()
                .any(|a| a.to_string_lossy().starts_with("--arg")),
            "empty overrides must not emit --arg: {args:?}"
        );
    }

    #[test]
    fn build_cli_args_allow_protected_drift_run_only() {
        // Issue #935: the flag forwards only on the run branch; dry-run has
        // no gate to bypass, and Default keeps it off for retry/resume.
        let flags = RunFlags {
            allow_protected_drift: true,
            ..RunFlags::default()
        };
        let args = build_cli_args(
            std::path::Path::new("wf.oxoflow"),
            std::path::Path::new("dir"),
            &flags,
        );
        let strs: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(strs[0], "run");
        assert!(strs.contains(&"--allow-protected-drift".to_string()));

        let dry = RunFlags {
            dry_run: true,
            ..flags
        };
        let args = build_cli_args(std::path::Path::new("w"), std::path::Path::new("d"), &dry);
        assert!(
            !args
                .iter()
                .any(|a| a.to_string_lossy() == "--allow-protected-drift"),
            "dry-run must not carry --allow-protected-drift: {args:?}"
        );

        let off = RunFlags::default();
        let args = build_cli_args(
            std::path::Path::new("wf.oxoflow"),
            std::path::Path::new("dir"),
            &off,
        );
        assert!(
            !args
                .iter()
                .any(|a| a.to_string_lossy() == "--allow-protected-drift"),
            "default must not emit --allow-protected-drift: {args:?}"
        );
    }

    #[test]
    fn build_cli_args_provenance_run_only() {
        // Issue #276: the flag forwards only on the run branch; dry-run
        // rejects it inside the CLI ("not supported by dry-run"), and
        // Default keeps it off for retry/resume.
        let flags = RunFlags {
            provenance: true,
            ..RunFlags::default()
        };
        let args = build_cli_args(
            std::path::Path::new("wf.oxoflow"),
            std::path::Path::new("dir"),
            &flags,
        );
        let strs: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(strs[0], "run");
        assert!(strs.contains(&"--provenance".to_string()));

        let dry = RunFlags {
            dry_run: true,
            ..flags
        };
        let args = build_cli_args(std::path::Path::new("w"), std::path::Path::new("d"), &dry);
        assert!(
            !args.iter().any(|a| a.to_string_lossy() == "--provenance"),
            "dry-run must not carry --provenance: {args:?}"
        );

        let off = RunFlags::default();
        let args = build_cli_args(
            std::path::Path::new("wf.oxoflow"),
            std::path::Path::new("dir"),
            &off,
        );
        assert!(
            !args.iter().any(|a| a.to_string_lossy() == "--provenance"),
            "default must not emit --provenance: {args:?}"
        );
    }

    #[test]
    fn exit_success_maps_to_completed() {
        assert_eq!(final_status_from_exit(true), "completed");
        assert_eq!(final_status_from_exit(false), "failed");
    }

    #[test]
    fn summary_extracts_config_change_block() {
        let log = "DAG: 2 rules in dependency order\n\
                   Config change:\n\
                   \x20 min_quality: 20 → 30\n\
                   \x20 → invalidated 1 (1 directly affected), re-running 1/2 this run, skipping 1\n\
                   \x20 Running: affected\n\
                   \x20 ✓ affected (0.1s)\n";
        let summary = extract_invalidation_summary(log).expect("config change must be found");
        assert!(summary.contains("Config change:"));
        assert!(summary.contains("min_quality: 20 → 30"));
        assert!(summary.contains("re-running 1/2 this run, skipping 1"));
        assert!(!summary.contains("Running: affected"));
        assert!(!summary.contains("✓ affected"));
    }

    #[test]
    fn summary_extracts_input_manifest_invalidations() {
        let log = "  ↻ input changes invalidated 2 rules: gather, report\n\
                   Note: checkpoint predates input tracking: recorded baseline input manifests for 1 completed rule; future input changes will invalidate them automatically\n";
        let summary = extract_invalidation_summary(log).expect("invalidation lines must be found");
        assert!(summary.contains("input changes invalidated 2 rules: gather, report"));
        assert!(summary.contains("recorded baseline input manifests"));
    }

    #[test]
    fn summary_handles_ansi_colored_output() {
        // The CLI colors "Config change:" and the "↻" marker when stderr is a
        // tty; substring matching must survive the escape wrappers.
        let log = "\u{1b}[1;36mConfig change:\u{1b}[0m\n\
                   \x20 n_threads: 4 → 8\n\
                   \x20 → invalidated 2 (1 directly affected), re-running 2/3 this run, skipping 1\n\
                   \u{1b}[33m↻\u{1b}[0m input changes invalidated 1 rule: align\n";
        let summary = extract_invalidation_summary(log).expect("must parse ANSI-wrapped lines");
        assert!(summary.contains("Config change:"));
        assert!(summary.contains("n_threads: 4 → 8"));
        assert!(summary.contains("input changes invalidated 1 rule: align"));
    }

    #[test]
    fn summary_returns_none_without_invalidation_activity() {
        let log = "DAG: 2 rules in dependency order\nRunning: a\n✓ a (0.1s)\nDone: 2 succeeded\n";
        assert_eq!(extract_invalidation_summary(log), None);
    }

    /// The SSE log tailer must recognize the exact lines the CLI narrates
    /// into the run log — including the two-space indent every
    /// `progress_narrate` line carries. The bare-prefix matching used to
    /// drop every rule event (live user-sim finding: /api/events emitted
    /// only run_started/run_completed).
    #[test]
    fn parse_rule_event_matches_indented_run_log_lines() {
        // Verbatim shapes from a real execution.log (ANSI already stripped
        // by the CLI before the line is written).
        assert_eq!(
            parse_rule_event("  Running: slow_make"),
            Some(("rule_started", "slow_make".to_string()))
        );
        assert_eq!(
            parse_rule_event("  ✓ slow_make (2.0s)"),
            Some(("rule_completed", "slow_make".to_string()))
        );
        assert_eq!(
            parse_rule_event("  ⊝ make_* (already completed)"),
            Some(("rule_skipped", "make_* (already completed)".to_string()))
        );
        assert_eq!(
            parse_rule_event("  ✗ rule 'slow_b' failed: exit status 1"),
            Some(("rule_failed", "slow_b".to_string()))
        );
        // Unindented variants keep parsing (historical logs).
        assert_eq!(
            parse_rule_event("Running: a"),
            Some(("rule_started", "a".to_string()))
        );
        assert_eq!(
            parse_rule_event("✓ a (0.1s)"),
            Some(("rule_completed", "a".to_string()))
        );
        // Non-event lines return None.
        assert_eq!(parse_rule_event("DAG: 2 rules in dependency order"), None);
        assert_eq!(parse_rule_event("Done: 2 succeeded, 0 failed"), None);
        assert_eq!(
            parse_rule_event("  ✓ Report snapshot: .oxo-flow/reports/r.json"),
            None
        );
        assert_eq!(
            parse_rule_event("  ✓ 2 output files verified (22B total)"),
            None
        );
    }

    #[test]
    fn sudo_username_regex_accepts_valid() {
        let re = Regex::new(r"^[a-z_][a-z0-9_-]*[$]?$").unwrap();
        assert!(re.is_match("admin"));
        assert!(re.is_match("user_001"));
        assert!(re.is_match("test-user"));
        assert!(re.is_match("bioinfo$"));
    }

    #[test]
    fn sudo_username_regex_rejects_injection() {
        let re = Regex::new(r"^[a-z_][a-z0-9_-]*[$]?$").unwrap();
        assert!(!re.is_match("admin; rm -rf /"));
        assert!(!re.is_match("user$(whoami)"));
        assert!(!re.is_match("root /etc/passwd"));
        assert!(!re.is_match(""));
        assert!(!re.is_match("UPPERCASE"));
    }

    #[tokio::test]
    async fn append_metrics_appends_below_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut lines = append_metrics(dir.path(), "l1", 0).await;
        lines = append_metrics(dir.path(), "l2", lines).await;
        assert_eq!(lines, 2);
        let content = tokio::fs::read_to_string(dir.path().join("metrics.jsonl"))
            .await
            .unwrap();
        assert_eq!(content, "l1\nl2\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn append_metrics_skips_non_regular_file() {
        // #735: a FIFO planted at metrics.jsonl (the workdir is
        // rule-writable) must not be opened for append — write-open blocks
        // until a reader appears. The test hangs if the gate regresses.
        let dir = tempfile::tempdir().expect("tempdir");
        let fifo = dir.path().join("metrics.jsonl");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo available on unix");
        assert!(status.success());

        let lines = append_metrics(dir.path(), "l1", 7).await;
        assert_eq!(lines, 7, "FIFO target must be skipped, count unchanged");
    }

    #[tokio::test]
    async fn append_metrics_trims_at_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("metrics.jsonl");
        let body: String = (0..2005).map(|i| format!("old{i}\n")).collect();
        tokio::fs::write(&path, body).await.unwrap();
        let lines = append_metrics(dir.path(), "new", 2005).await;
        assert_eq!(lines, 2000);
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let kept: Vec<&str> = content.lines().collect();
        assert_eq!(kept.len(), 2000);
        assert_eq!(kept.last(), Some(&"new"));
        assert!(!kept.contains(&"old5"), "oldest lines are trimmed first");
    }

    #[test]
    fn probe_parses_cli_version_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stub = dir.path().join("oxo-flow");
        std::fs::write(&stub, "#!/bin/sh\necho 'oxo-flow 0.20.1'\n").unwrap();
        make_executable(&stub);

        let probe = probe_engine_binary(&stub).expect("probe must parse the stub output");
        assert_eq!(probe.version, "0.20.1");
        assert_eq!(probe.binary, stub.to_string_lossy());
        // An explicit path with a parent directory counts as a target-dir hit.
        assert_eq!(probe.source, BinarySource::TargetDir);
    }

    #[test]
    fn probe_rejects_non_version_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stub = dir.path().join("oxo-flow");
        std::fs::write(&stub, "#!/bin/sh\necho 'usage: oxo-flow <cmd>'\nexit 1\n").unwrap();
        make_executable(&stub);

        assert!(
            probe_engine_binary(&stub).is_none(),
            "non-numeric output must not become a version"
        );
    }

    #[test]
    fn probe_fails_soft_on_missing_binary() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(probe_engine_binary(&dir.path().join("no-such-binary")).is_none());
    }

    #[test]
    fn major_minor_mismatch_ignores_patch_and_tails() {
        let probe = EngineProbe {
            version: "0.20.9".into(),
            binary: String::new(),
            source: BinarySource::TargetDir,
        };
        assert!(!probe.major_minor_mismatch("0.20.1"));
        assert!(probe.major_minor_mismatch("0.18.0"));
        assert!(probe.major_minor_mismatch("1.0.0"));

        // Malformed server versions degrade to (0, 0) rather than panicking.
        assert!(probe.major_minor_mismatch(""));
        assert!(!probe.major_minor_mismatch("0.20"));
    }

    #[test]
    fn version_less_than_compares_release_triples() {
        assert!(version_less_than("0.18.0", "0.20.1"));
        assert!(!version_less_than("0.20.1", "0.20.1"));
        assert!(!version_less_than("0.21.0", "0.20.1"));
        // Non-numeric tails are ignored, not errors.
        assert!(version_less_than("0.20.0-rc1", "0.20.1"));
        assert!(!version_less_than("garbage", "0.20.1") || version_less_than("0.1", "0.20.1"));
    }

    /// Mark a stub script executable (ignore on filesystems without modes).
    fn make_executable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    #[test]
    fn extracts_preview_from_mixed_log() {
        // Banner + human lines interleave with the JSON document (the real
        // execution.log shape).
        let log = "oxo-flow v0.11.0 — banner line\n\
                   Plan: would run: 2 | skip: 0 | completed: 0 (DAG size: 2)\n\
                   {\"checkpoint_preview\":{\"summary\":{\"will_run\":2,\"will_skip\":0},\"plan\":[{\"name\":\"gather_cohort_S1\",\"status\":\"run-never-completed\"},{\"name\":\"gather_cohort_S2\",\"status\":\"run-never-completed\"}]},\"execution_order\":[\"gather_cohort_S1\",\"gather_cohort_S2\"]}\n";
        let preview = extract_dry_run_preview(log).expect("preview must parse");
        assert_eq!(preview["checkpoint_preview"]["summary"]["will_run"], 2);
        let plan = preview["checkpoint_preview"]["plan"].as_array().unwrap();
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0]["name"], "gather_cohort_S1");
    }

    #[test]
    fn returns_none_for_run_logs_without_preview() {
        let log = "DAG: 2 rules in dependency order\nRunning: a\n✓ a (0.1s)\nDone: 2 succeeded\n";
        assert_eq!(extract_dry_run_preview(log), None);
    }

    #[test]
    fn handles_nested_objects_until_parse_succeeds() {
        let log = "{\"checkpoint_preview\":{\"summary\":{\"will_run\":1,\"will_skip\":0,\"protected_outside\":0},\"plan\":[{\"name\":\"a\",\"status\":\"run-never-completed\",\"cascaded_from\":null}],\"cascade_chains\":[]},\"command\":\"dry-run\",\"execution_order\":[\"a\"]}";
        let preview = extract_dry_run_preview(log).expect("nested preview must parse");
        assert_eq!(preview["command"], "dry-run");
    }

    /// The workdir is rule-writable: a planted symlink at the final preview
    /// path must be replaced, not written through, and no temp file may
    /// linger after a successful persist.
    #[cfg(unix)]
    #[tokio::test]
    async fn persists_preview_atomically_replacing_a_planted_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.txt");
        std::fs::write(&victim, "keep me").unwrap();
        let final_path = dir.path().join("dry-run-preview.json");
        std::os::unix::fs::symlink(&victim, &final_path).unwrap();

        persist_preview_atomically("run-test", dir.path(), "{\"ok\":true}".to_string()).await;

        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        assert!(
            std::fs::symlink_metadata(&final_path)
                .unwrap()
                .file_type()
                .is_file()
        );
        assert_eq!(
            std::fs::read_to_string(&final_path).unwrap(),
            "{\"ok\":true}"
        );
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }
}
