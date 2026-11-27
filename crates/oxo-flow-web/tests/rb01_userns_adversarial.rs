//! Two-tenant adversarial suite for per-run user + PID namespace isolation
//! (RB-01 residual / issue #1401, the `OXO_FLOW_WEB_ISOLATION=userns`
//! mechanism).
//!
//! Tenant A submits workflows through the REAL web spawn path
//! (`POST /api/runs` → `spawn_background_run_with_args` → `sh -c` wrapper →
//! `unshare` → engine binary) while tenant B runs a live workflow and the
//! test holds a private host process. The suite proves, per dimension the
//! mode claims:
//!
//! | Dimension | Probe (run inside the namespace) | Asserted |
//! |---|---|---|
//! | uid mapping | `id -u` | 0 inside; the server uid outside (control) |
//! | /proc visibility | `test -d /proc/<server-pid>` | hidden |
//! | /proc disclosure | `cat /proc/<server-pid>/environ` | denied |
//! | Host signals | `kill -0 <server pid>` | denied |
//! | Cross-tenant signals | `kill -0 <tenant B's live pid>` | denied |
//! | Private /proc | numeric entry count in `/proc` | far below the host count |
//! | RLIMIT inheritance | `ulimit -u` | the configured `RLIMIT_NPROC` |
//! | Exit-code wrapper | run whose CLI exits 7 | run recorded `failed` |
//! | Cancel | POST cancel on tenant B's long run | no surviving process |
//! | Sanity | write the run's own workdir | pass |
//!
//! Non-vacuity: the SAME probe stub first runs with the mode OFF (control
//! run) and every probe LEAKS — so each denial assertion is one that a
//! regression flips back to a leak and fails. The cross-tenant signal probe
//! targets a REAL long-running run of tenant B, not a synthetic sleep.
//!
//! Residuals NOT asserted here (documented in the web API reference):
//! filesystem/network confinement (namespaces do not provide it — use
//! `OXO_FLOW_WEB_SANDBOX=landlock`), UDP/ICMP, and per-uid `RLIMIT_NPROC`
//! accounting (the suite asserts inheritance of the limit, not per-tenant
//! partitioning).
//!
//! ## Running the suite
//!
//! The mode needs unprivileged user namespaces. On a host where the kernel
//! or LSM forbids them, the test prints the reason and returns (same "skip
//! with a named reason" convention as the PostgreSQL tests). On Ubuntu
//! 24.04 the server/test binary needs an AppArmor profile granting
//! `userns,` — run the built test binary under it, e.g.
//! `sudo aa-exec -p oxo-flow-userns -- <test binary> --nocapture`.
//!
//! One test per binary: the harness mutates process-global environment
//! (`OXO_FLOW_WORKSPACE`, `OXO_FLOW_BIN`, `HOME`, the isolation knob, the
//! passthrough probes), so a second parallel test would race these
//! `set_var` calls.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tower::ServiceExt;

mod common;

#[tokio::test]
async fn userns_run_is_isolated_from_host_and_tenants() {
    if let Err(reason) = probe_userns() {
        eprintln!(
            "skipping rb01_userns_adversarial: {reason}. The mode is Linux-only and needs \
             unprivileged user namespaces (kernel.unprivileged_userns_clone=1, and on \
             Ubuntu 24.04 an AppArmor `userns,` profile for the server binary)."
        );
        return;
    }
    run_userns_suite().await;
}

/// Probe the EXACT wrapper argv the executor uses (`--kill-child=SIGTERM`
/// and the `--` separator included) with `true` as the payload. Any failure
/// — missing binary, EPERM from the kernel/AppArmor, mount-proc failure —
/// skips the suite with the reason.
fn probe_userns() -> Result<(), String> {
    let out = std::process::Command::new("unshare")
        .args([
            "--user",
            "--map-root-user",
            "--pid",
            "--fork",
            "--mount-proc",
            "--kill-child=SIGTERM",
            "--",
            "true",
        ])
        .output()
        .map_err(|e| format!("unshare could not be executed: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(format!(
        "unshare probe exited with {}: {}",
        out.status,
        stderr.trim()
    ))
}

fn server_uid() -> String {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// True while any live process command line contains `needle`.
fn process_alive_with(needle: &str) -> bool {
    std::process::Command::new("ps")
        .args(["-e", "-o", "args="])
        .output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|line| line.contains(needle))
        })
        .unwrap_or(false)
}

/// Host pid of the first live process whose command line contains `needle`.
fn find_pid_with(needle: &str) -> Option<u32> {
    let out = std::process::Command::new("ps")
        .args(["-e", "-o", "pid=,args="])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse::<u32>().ok()?;
            let args = fields.collect::<Vec<_>>().join(" ");
            args.contains(needle).then_some(pid)
        })
}

/// A stub `oxo-flow` standing in for the CLI. It parses `--workdir` from its
/// argv and records `probe: outcome` lines into the run workdir (the one
/// tree the run owns). Exits 0 unless `RB01U_EXIT_CODE` is set, which proves
/// the `.exit-code` wrapper still observes the CLI's status through
/// `unshare`.
fn write_stub(stub: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let script = r#"#!/bin/bash
dir=""
while [ $# -gt 0 ]; do
  case "$1" in
    --workdir) dir="$2"; shift 2 ;;
    *) shift ;;
  esac
done
res="$dir/probe-results.txt"
: > "$res" || exit 1
echo "workdir-write: pass" >> "$res"
echo "uid: $(id -u)" >> "$res"
echo "nproc: $(ulimit -u)" >> "$res"

if [ -d "/proc/$RB01U_SERVER_PID" ]; then
  echo "proc-server-visible: VISIBLE" >> "$res"
else
  echo "proc-server-visible: hidden" >> "$res"
fi

if cat "/proc/$RB01U_SERVER_PID/environ" >/dev/null 2>&1; then
  echo "proc-environ: LEAKED" >> "$res"
else
  echo "proc-environ: denied" >> "$res"
fi

for probe in server victim tenantb; do
  case "$probe" in
    server) pid="$RB01U_SERVER_PID" ;;
    victim) pid="$RB01U_VICTIM_PID" ;;
    tenantb) pid="$RB01U_B_PID" ;;
  esac
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    echo "signal-$probe: SIGNALLED" >> "$res"
  else
    echo "signal-$probe: denied" >> "$res"
  fi
done

n=$(ls /proc 2>/dev/null | grep -c '^[0-9]')
echo "proc-pid-count: ${n:-0}" >> "$res"
exit "${RB01U_EXIT_CODE:-0}"
"#;
    std::fs::write(stub, script).expect("write stub");
    let mut perms = std::fs::metadata(stub)
        .expect("stub metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(stub, perms).expect("chmod stub");
}

/// A long-running stub for tenant B: replaces itself (argv[0] = marker) with
/// `sleep`, so the test can locate its HOST pid and probe signals against a
/// genuine other-tenant process. It is terminated only by the cancel leg.
fn write_sleeper(sleeper: &std::path::Path, marker: &str) {
    use std::os::unix::fs::PermissionsExt;
    let script = format!("#!/bin/bash\nexec -a {marker} sleep 180\n");
    std::fs::write(sleeper, script).expect("write sleeper stub");
    let mut perms = std::fs::metadata(sleeper)
        .expect("sleeper metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(sleeper, perms).expect("chmod sleeper");
}

fn parse_results(run_dir: &std::path::Path) -> std::collections::HashMap<String, String> {
    let text = std::fs::read_to_string(run_dir.join("probe-results.txt"))
        .expect("stub wrote its probe results");
    text.lines()
        .filter_map(|l| l.split_once(": "))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Print the recorded probes in a deterministic order so a `--nocapture`
/// run produces self-contained evidence (the run workdir is temporary and
/// is not part of the shipped tree).
fn print_probe_summary(label: &str, results: &std::collections::HashMap<String, String>) {
    let mut keys: Vec<&String> = results.keys().collect();
    keys.sort();
    eprintln!("RB01U {label} probe results:");
    for key in keys {
        eprintln!("RB01U   {key}: {}", results[key]);
    }
}

fn assert_probe(
    results: &std::collections::HashMap<String, String>,
    probe: &str,
    expected: &str,
    run_dir: &std::path::Path,
) {
    assert_eq!(
        results.get(probe).map(String::as_str),
        Some(expected),
        "probe {probe:?} expected {expected:?} — results in {}:\n{}",
        run_dir.join("probe-results.txt").display(),
        std::fs::read_to_string(run_dir.join("probe-results.txt")).unwrap_or_default()
    );
}

async fn seed_identity(token: &str, id: &str, username: &str, role: &str) {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT OR REPLACE INTO users (id, username, role, auth_type, created_at) \
         VALUES (?, ?, ?, 'password', ?)",
    )
    .bind(id)
    .bind(username)
    .bind(role)
    .bind(&now)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT OR REPLACE INTO sessions (token, user_id, created_at, expires_at) \
         VALUES (?, ?, ?, '2999-01-01T00:00:00Z')",
    )
    .bind(token)
    .bind(id)
    .bind(&now)
    .execute(pool)
    .await
    .unwrap();
}

async fn post_run(app: &axum::Router, token: &str, toml: &str) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/runs")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"toml_content": toml}).to_string()))
                .unwrap(),
        )
        .await
        .expect("router response");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .expect("body bytes");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn post_cancel(app: &axum::Router, token: &str, run_id: &str) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/runs/{run_id}/cancel"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .expect("router response");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .expect("body bytes");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn wait_terminal(run_id: &str, secs: u64) -> String {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let status: String = sqlx::query_scalar("SELECT status FROM runs WHERE id = ?")
            .bind(run_id)
            .fetch_one(pool)
            .await
            .expect("run row exists");
        if status != "queued" && status != "running" {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "run {run_id} never reached a terminal state (stuck at {status})"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Wait until `needle` disappears from the host process table; false on
/// timeout.
fn wait_process_gone(needle: &str, secs: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if !process_alive_with(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    !process_alive_with(needle)
}

/// The full suite body — runs only where the wrapper probe succeeds.
async fn run_userns_suite() {
    const A_TOKEN: &str = "rb01u-a-token";
    const A_ID: &str = "rb01u-a-id";
    const A_USER: &str = "rb01u-a-user";
    const B_TOKEN: &str = "rb01u-b-token";
    const B_ID: &str = "rb01u-b-id";
    const B_USER: &str = "rb01u-b-user";

    /// Trivial workflow: the stub binary carries the probes, the shell text
    /// only has to pass admission.
    const PROBE_WORKFLOW: &str = r#"
[workflow]
name = "rb01u-probe"

[[rules]]
name = "probe"
output = ["out.txt"]
shell = "echo rb01u-probe"
"#;

    let tmp = std::env::var("CARGO_TARGET_TMPDIR")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    let root = std::path::PathBuf::from(tmp).join(format!("rb01u-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let workspace = root.join("workspace");
    let home = root.join("home");
    std::fs::create_dir_all(home.join(".ssh")).expect("create server-home skeleton");
    let engine_bin_dir = root.join("engine-bin");
    std::fs::create_dir_all(&engine_bin_dir).expect("create engine bin dir");
    let stub = engine_bin_dir.join("oxo-flow-stub");
    write_stub(&stub);
    let marker = format!("rb01u-b-marker-{}", std::process::id());
    let sleeper = engine_bin_dir.join("oxo-flow-sleeper");
    write_sleeper(&sleeper, &marker);

    // A live host process owned by the test (same uid as the server): the
    // "any host process" victim for the signal probes.
    let mut victim = std::process::Command::new("sleep")
        .arg("120")
        .spawn()
        .expect("spawn victim");
    let victim_pid = victim.id();

    // SAFETY: this binary contains exactly one test; every `set_var` runs
    // before the router (and therefore any spawn task) exists.
    unsafe {
        std::env::set_var("OXO_FLOW_WORKSPACE", &workspace);
        std::env::set_var("OXO_FLOW_BIN", &stub);
        std::env::set_var("OXO_FLOW_AI_CONFIG", root.join("hermetic/ai_config.json"));
        std::env::set_var("HOME", &home);
        std::env::set_var(
            "OXO_FLOW_ENV_PASSTHROUGH",
            "RB01U_SERVER_PID,RB01U_VICTIM_PID,RB01U_B_PID,RB01U_EXIT_CODE",
        );
        std::env::set_var("RB01U_SERVER_PID", std::process::id().to_string());
        std::env::set_var("RB01U_VICTIM_PID", victim_pid.to_string());
    }

    common::ensure_db().await;
    oxo_flow_web::domains::execution::handlers::set_run_rate_limit_for_tests(0);
    let app = server::build_router("team");
    seed_identity(A_TOKEN, A_ID, A_USER, "user").await;
    seed_identity(B_TOKEN, B_ID, B_USER, "user").await;

    // ── Fail-closed leg: a requested-but-unavailable host ─────────────────
    // PATH is prepended with a FAILING `unshare`, so the availability probe
    // fails for this spawn. The executor must refuse to run anything, mark
    // the run failed with the reason, and release its quota reservation.
    // The probe caches only SUCCESS, so removing the stub lets the later
    // legs probe again and succeed.
    let fake_bin = root.join("fake-bin");
    std::fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_unshare = fake_bin.join("unshare");
    std::fs::write(
        &fake_unshare,
        "#!/bin/bash\necho 'simulated userns unavailable' >&2\nexit 1\n",
    )
    .expect("write failing unshare stub");
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&fake_unshare)
            .expect("fake unshare metadata")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake_unshare, perms).expect("chmod fake unshare");
    }
    let original_path = std::env::var_os("PATH");
    // SAFETY: this binary contains exactly one test, and the control/isolated
    // spawn tasks below wait for this run's terminal state before any later
    // `set_var` call.
    unsafe {
        let mut patched = fake_bin.clone().into_os_string();
        if let Some(path) = &original_path {
            patched.push(":");
            patched.push(path);
        }
        std::env::set_var("PATH", patched);
        std::env::set_var("OXO_FLOW_WEB_ISOLATION", "userns");
    }
    let (status, body) = post_run(&app, A_TOKEN, PROBE_WORKFLOW).await;
    assert_eq!(status, StatusCode::OK, "fail-closed run accepted: {body}");
    let fail_id = body["run_id"].as_str().expect("run_id").to_string();
    assert_eq!(
        wait_terminal(&fail_id, 30).await,
        "failed",
        "a requested-but-unavailable userns mode must fail the run"
    );
    let fail_dir =
        oxo_flow_web::workspace::get_run_directory(A_ID, &fail_id).expect("fail-closed run dir");
    let fail_log = std::fs::read_to_string(fail_dir.join("execution.log")).unwrap_or_default();
    assert!(
        fail_log.contains("namespace isolation unavailable"),
        "the refusal reason must be persisted: {fail_log}"
    );
    assert!(
        !fail_dir.join("probe-results.txt").exists() && !fail_dir.join(".exit-code").exists(),
        "no CLI or wrapper may execute when the requested isolation is unavailable"
    );
    let backend = oxo_flow_web::infra::db::global_backend();
    let usage = oxo_flow_web::infra::quota::usage(backend.as_deref(), A_ID)
        .await
        .expect("quota usage");
    assert_eq!(
        usage.active_runs, 0,
        "a failed spawn must release the quota reservation"
    );
    eprintln!(
        "RB01U fail-closed: run={fail_id} status=failed log_names_reason=true \
         no_cli_executed=true quota_active_runs={}",
        usage.active_runs
    );
    // SAFETY: the fail-closed run is terminal; its spawn task is gone.
    unsafe {
        match original_path {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
        std::env::remove_var("OXO_FLOW_WEB_ISOLATION");
    }

    // ── Tenant B: a live run inside its own namespace ─────────────────────
    // SAFETY: no spawn task reads this variable before the control run is
    // posted (the control run below waits for its terminal state first).
    unsafe {
        std::env::set_var("OXO_FLOW_WEB_ISOLATION", "userns");
        std::env::set_var("OXO_FLOW_BIN", &sleeper);
    }
    let (status, body) = post_run(&app, B_TOKEN, PROBE_WORKFLOW).await;
    assert_eq!(status, StatusCode::OK, "tenant B run accepted: {body}");
    let b_id = body["run_id"].as_str().expect("run_id").to_string();
    let b_deadline = Instant::now() + Duration::from_secs(30);
    let b_pid = loop {
        if let Some(pid) = find_pid_with(&marker) {
            break pid;
        }
        assert!(
            Instant::now() < b_deadline,
            "tenant B's namespaced process never appeared (run {b_id}):\n{}",
            std::fs::read_to_string(
                oxo_flow_web::workspace::get_run_directory(B_ID, &b_id)
                    .expect("B run dir")
                    .join("execution.log")
            )
            .unwrap_or_default()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    // SAFETY: the B spawn has completed (its process is visible); no spawn
    // task is between reading the environment and cloning it.
    unsafe { std::env::set_var("RB01U_B_PID", b_pid.to_string()) };

    // ── Control run: isolation OFF — every probe must LEAK ────────────────
    // SAFETY: the B spawn has completed; the control spawn reads the
    // environment only when it starts.
    unsafe {
        std::env::remove_var("OXO_FLOW_WEB_ISOLATION");
        std::env::set_var("OXO_FLOW_BIN", &stub);
    }
    let (status, body) = post_run(&app, A_TOKEN, PROBE_WORKFLOW).await;
    assert_eq!(status, StatusCode::OK, "control run accepted: {body}");
    let control_id = body["run_id"].as_str().expect("run_id").to_string();
    assert_eq!(
        wait_terminal(&control_id, 30).await,
        "completed",
        "the stub exits 0 in both modes"
    );
    let control_dir =
        oxo_flow_web::workspace::get_run_directory(A_ID, &control_id).expect("run dir");
    let control = parse_results(&control_dir);
    print_probe_summary("CONTROL", &control);
    assert_probe(&control, "workdir-write", "pass", &control_dir);
    assert_eq!(
        control.get("uid").map(String::as_str),
        Some(server_uid().as_str()),
        "control run must keep the server uid"
    );
    // Non-vacuity: every denied dimension must be reachable unisolated.
    assert_probe(&control, "proc-server-visible", "VISIBLE", &control_dir);
    assert_probe(&control, "proc-environ", "LEAKED", &control_dir);
    assert_probe(&control, "signal-server", "SIGNALLED", &control_dir);
    assert_probe(&control, "signal-victim", "SIGNALLED", &control_dir);
    assert_probe(&control, "signal-tenantb", "SIGNALLED", &control_dir);

    // ── Isolated run: OXO_FLOW_WEB_ISOLATION=userns ───────────────────────
    // SAFETY: the control run is terminal; its spawn already cloned its env.
    unsafe {
        std::env::set_var("OXO_FLOW_WEB_ISOLATION", "userns");
        // The rlimit composition leg: an operator knob installed on the
        // wrapper must be inherited by the CLI inside the namespace.
        std::env::set_var("OXO_FLOW_WEB_RLIMIT_NPROC", "4096");
    }
    let (status, body) = post_run(&app, A_TOKEN, PROBE_WORKFLOW).await;
    assert_eq!(status, StatusCode::OK, "isolated run accepted: {body}");
    let isolated_id = body["run_id"].as_str().expect("run_id").to_string();
    assert_eq!(
        wait_terminal(&isolated_id, 60).await,
        "completed",
        "isolated run failed; execution.log:\n{}",
        std::fs::read_to_string(
            oxo_flow_web::workspace::get_run_directory(A_ID, &isolated_id)
                .expect("isolated run dir")
                .join("execution.log")
        )
        .unwrap_or_default()
    );
    let isolated_dir =
        oxo_flow_web::workspace::get_run_directory(A_ID, &isolated_id).expect("run dir");
    let isolated = parse_results(&isolated_dir);
    print_probe_summary("ISOLATED", &isolated);

    assert_probe(&isolated, "workdir-write", "pass", &isolated_dir);
    assert_probe(&isolated, "uid", "0", &isolated_dir);
    assert_probe(&isolated, "nproc", "4096", &isolated_dir);
    assert_probe(&isolated, "proc-server-visible", "hidden", &isolated_dir);
    for probe in [
        "proc-environ",
        "signal-server",
        "signal-victim",
        "signal-tenantb",
    ] {
        assert_probe(&isolated, probe, "denied", &isolated_dir);
    }
    let host_pids: u64 = control
        .get("proc-pid-count")
        .and_then(|v| v.parse().ok())
        .expect("control run recorded a pid count");
    let ns_pids: u64 = isolated
        .get("proc-pid-count")
        .and_then(|v| v.parse().ok())
        .expect("isolated run recorded a pid count");
    assert!(
        ns_pids < host_pids,
        "the namespace /proc must show far fewer processes (host {host_pids}, \
         namespace {ns_pids}) — mount-proc did not take effect"
    );
    // Ownership, not just `id -u`: a file created INSIDE the namespace must
    // be owned by the server uid on the host (the single-uid mapping) — the
    // run's workdir artifacts stay writable by the engine outside.
    {
        use std::os::unix::fs::MetadataExt;
        let created = isolated_dir.join("probe-results.txt");
        let owner = std::fs::metadata(&created)
            .expect("probe results file exists")
            .uid();
        assert_eq!(
            owner,
            server_uid().parse::<u32>().expect("numeric server uid"),
            "files created inside the namespace must keep the server uid outside"
        );
    }

    // ── Exit-status propagation through the wrapper ───────────────────────
    // The CLI exits 7; the `.exit-code` wrapper and the web executor must
    // still observe a failed run (unshare propagates its child's status).
    // SAFETY: the isolated run is terminal; no spawn is mid-clone.
    unsafe { std::env::set_var("RB01U_EXIT_CODE", "7") };
    let (status, body) = post_run(&app, A_TOKEN, PROBE_WORKFLOW).await;
    assert_eq!(status, StatusCode::OK, "exit-code run accepted: {body}");
    let exit_id = body["run_id"].as_str().expect("run_id").to_string();
    let exit_status = wait_terminal(&exit_id, 30).await;
    eprintln!("RB01U exit-code run: status={exit_status} (CLI exit 7 through unshare)");
    assert_eq!(
        exit_status, "failed",
        "the CLI's status must reach the web executor through unshare"
    );
    // SAFETY: the exit-code run is terminal; no further run needs the knob.
    unsafe { std::env::remove_var("RB01U_EXIT_CODE") };

    // ── Cancel: the whole namespace dies, no survivor ─────────────────────
    let (status, body) = post_cancel(&app, B_TOKEN, &b_id).await;
    assert_eq!(status, StatusCode::OK, "cancel response: {body}");
    assert_eq!(body["status"], "cancelled");
    let b_status = wait_terminal(&b_id, 30).await;
    assert_eq!(b_status, "cancelled");
    let survivor_gone = wait_process_gone(&marker, 15);
    eprintln!(
        "RB01U cancel: run={b_id} status={b_status} tenant_b_host_pid={b_pid} \
         marker={marker} namespace_has_no_survivor={survivor_gone}"
    );
    assert!(
        survivor_gone,
        "a process inside tenant B's namespace survived cancel (SIGTERM → SIGKILL): \
         the supervisor's group kill must tear down the whole namespace"
    );

    // ── SIGTERM-only teardown, no SIGKILL involved ────────────────────────
    // A CLI that traps SIGTERM must get a graceful window: this leg sends
    // SIGTERM (never SIGKILL) to the wrapper's group and requires the whole
    // namespace to be gone well inside the supervisor's 5 s SIGKILL
    // escalation. The inner marker deliberately IGNORES SIGTERM, so it can
    // only disappear when the namespace init exits and the kernel tears the
    // namespace down.
    let plan = oxo_flow_web::userns::configure(Some("userns"), false, false)
        .expect("userns configure")
        .expect("mode requested");
    let sig_marker = format!("rb01u-sigterm-{}", std::process::id());
    let script = format!(
        "trap 'exit 0' TERM; bash -c 'trap \"\" TERM; exec -a {sig_marker} sleep 300' & wait"
    );
    let exit_file = root.join("sigterm-leg-exit-code");
    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c")
        .arg(oxo_flow_web::executor::EXIT_CODE_WRAPPER_SCRIPT);
    cmd.arg("sh").arg(&exit_file);
    cmd.args(plan.wrap_payload(vec![
        std::ffi::OsString::from("bash"),
        std::ffi::OsString::from("-c"),
        std::ffi::OsString::from(script),
    ]));
    cmd.process_group(0)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut sig_child = cmd.spawn().expect("spawn SIGTERM-only leg");
    let sig_pgid = sig_child.id() as i32;
    oxo_flow_web::process_control::register("rb01u-sigterm-leg", sig_pgid);
    let start_deadline = Instant::now() + Duration::from_secs(10);
    while !process_alive_with(&sig_marker) {
        assert!(
            Instant::now() < start_deadline,
            "the SIGTERM-only leg never started"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let t0 = Instant::now();
    oxo_flow_web::process_control::signal_group(sig_pgid, oxo_flow_web::process_control::SIGTERM)
        .expect("SIGTERM to the namespace group");
    assert!(
        wait_process_gone(&sig_marker, 4),
        "SIGTERM alone must tear down the namespace without waiting for the \
         supervisor's SIGKILL escalation"
    );
    let elapsed = t0.elapsed();
    assert!(
        elapsed < Duration::from_secs(5),
        "graceful teardown took {elapsed:?}; the escalation window is 5 s, so this \
         leg proves the SIGTERM path itself"
    );
    eprintln!(
        "RB01U sigterm-only teardown: elapsed={:?} namespace_has_no_survivor=true",
        elapsed
    );
    let _ = oxo_flow_web::process_control::signal_group(
        sig_pgid,
        oxo_flow_web::process_control::SIGKILL,
    );
    let _ = sig_child.wait();
    oxo_flow_web::process_control::unregister("rb01u-sigterm-leg");

    // ── Cleanup ───────────────────────────────────────────────────────────
    let _ = victim.kill();
    let _ = victim.wait();
}
