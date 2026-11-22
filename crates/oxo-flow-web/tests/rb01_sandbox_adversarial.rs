//! Two-tenant adversarial suite for the OS-level run sandbox (RB-01 / issue
//! #1129, the `OXO_FLOW_WEB_SANDBOX=landlock` mechanism).
//!
//! Tenant A submits workflows through the REAL web spawn path
//! (`POST /api/runs` → `spawn_background_run_with_args` → `sh -c` wrapper →
//! engine binary) while tenant B holds private data on the same host. The
//! suite proves, per dimension the sandbox claims:
//!
//! | Dimension | Probe (run by the sandboxed process tree) | Asserted |
//! |---|---|---|
//! | Another tenant's workspace (read) | `cat <B's input file>` | denied |
//! | Another tenant's workspace (write) | `touch <B's inputs dir>/attempt.txt` | denied |
//! | The server database | `cat <the SQLite file backing this test server>` | denied |
//! | Host credentials | `cat $HOME/.ssh/id_rsa` (server account home) | denied |
//! | Arbitrary host files | `cat /etc/hostname` | denied |
//! | Server secrets via `/proc/<server-pid>/environ` | the RB-01a env allowlist is worthless if `/proc` is readable — probed explicitly | denied |
//! | Network | TCP connect to a live listener the test holds open | denied (kernel ≥ 6.7) |
//! | Sanities | writing the run's own workdir; executing a system binary | still work |
//!
//! Non-vacuity: the SAME probe workflow first runs with the sandbox OFF
//! (control run) and every probe LEAKS — so each denial assertion above is
//! one that a sandbox regression (removing the `pre_exec` attach, breaking
//! the grant set) flips back to a leak and fails. A second real-engine leg
//! replaces the stub with the actual `oxo-flow` CLI: a rule that reads B's
//! file fails the run, while a sibling rule writing the run's own workdir
//! still succeeds — the enforcement holds through the engine's own rule-shell
//! spawn chain.
//!
//! Platform gating (mirrors the OXO_PG_TEST_URL convention of "skip with a
//! named reason"): the suite requires Linux with Landlock (5.13+); anywhere
//! else it prints the reason and returns. The TCP-deny assertion is
//! additionally gated on Landlock ABI v4 (kernel 6.7+), because the network
//! dimension simply does not exist on older kernels — the run asserts what
//! the kernel can enforce, nothing more.
//!
//! One test per binary: the harness mutates process-global environment
//! (`OXO_FLOW_WORKSPACE`, `OXO_FLOW_BIN`, `HOME`, the passthrough probes),
//! so a second parallel test would race these `set_var` calls.

#[cfg(target_os = "linux")]
use axum::body::Body;
#[cfg(target_os = "linux")]
use axum::http::{Request, StatusCode};
#[cfg(target_os = "linux")]
use oxo_flow_web::server;
#[cfg(target_os = "linux")]
use serde_json::{Value, json};
#[cfg(target_os = "linux")]
use tower::ServiceExt;

#[cfg(target_os = "linux")]
mod common;

#[tokio::test]
async fn sandboxed_run_cannot_reach_other_tenants_or_the_host() {
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!(
            "skipping rb01_sandbox_adversarial: the OXO_FLOW_WEB_SANDBOX mechanism is \
             Landlock-based and Linux-only; on this platform the suite would prove nothing"
        );
    }
    #[cfg(target_os = "linux")]
    run_adversarial_suite().await;
}

/// The kernel's Landlock ABI version, or `None` when unavailable. Mirrors the
/// executor's probe (the kernel interface is 5 lines; the test probes the
/// same syscalls directly because the sandbox's linux module is private).
#[cfg(target_os = "linux")]
fn kernel_landlock_abi() -> Option<u32> {
    // SAFETY: `landlock_create_ruleset(NULL, 0, VERSION)` is a pure query —
    // no pointers are dereferenced and no fd is created.
    let v = unsafe {
        nix::libc::syscall(
            nix::libc::SYS_landlock_create_ruleset,
            std::ptr::null::<nix::libc::c_void>(),
            0usize,
            1u32, // LANDLOCK_CREATE_RULESET_VERSION
        )
    };
    if v < 0 { None } else { Some(v as u32) }
}

/// Locate a workspace binary from the target directory (same pattern as
/// `process_control_integration.rs::workspace_bin` — the CLI is not a
/// dependency of the web crate). Requires `cargo build --workspace` first.
#[cfg(target_os = "linux")]
fn workspace_bin(name: &str) -> std::path::PathBuf {
    let target_dir = std::env::current_exe()
        .expect("cannot find current test executable path")
        .parent()
        .expect("no parent dir for test exe")
        .parent()
        .expect("no grandparent dir for test exe")
        .to_path_buf();
    let candidate = target_dir.join(name);
    if candidate.exists() {
        return candidate;
    }
    let candidate = target_dir.join("deps").join(name);
    if candidate.exists() {
        return candidate;
    }
    panic!(
        "could not find binary '{name}' in target directory; \
         run `cargo build --workspace` first"
    );
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
async fn wait_terminal(run_id: &str, secs: u64) -> String {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
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
            std::time::Instant::now() < deadline,
            "run {run_id} never reached a terminal state (stuck at {status})"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// A stub `oxo-flow` standing in for the CLI. It parses `--workdir` from its
/// argv (the web builds `run <workflow> --workdir <dir> …`), runs every
/// adversarial probe against the paths the passthrough chain carried in, and
/// records `probe: outcome` lines into the run workdir (the one tree the
/// sandbox grants). It exits 0 either way — the assertions read the
/// recorded outcomes, not the exit code.
#[cfg(target_os = "linux")]
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

if cat "$RB01X_B_FILE" >/dev/null 2>&1; then
  echo "b-read: LEAKED" >> "$res"
else
  echo "b-read: denied" >> "$res"
fi

if touch "$RB01X_B_DIR/attempt.txt" 2>/dev/null; then
  echo "b-write: WROTE" >> "$res"
else
  echo "b-write: denied" >> "$res"
fi

if cat "$RB01X_DB_FILE" >/dev/null 2>&1; then
  echo "db-read: LEAKED" >> "$res"
else
  echo "db-read: denied" >> "$res"
fi

if cat /etc/hostname >/dev/null 2>&1; then
  echo "host-etc: LEAKED" >> "$res"
else
  echo "host-etc: denied" >> "$res"
fi

if cat "$HOME/.ssh/id_rsa" >/dev/null 2>&1; then
  echo "host-home: LEAKED" >> "$res"
else
  echo "host-home: denied" >> "$res"
fi

if cat "/proc/$RB01X_SERVER_PID/environ" >/dev/null 2>&1; then
  echo "proc-environ: LEAKED" >> "$res"
else
  echo "proc-environ: denied" >> "$res"
fi

if command -v bash >/dev/null 2>&1 && bash -c "exec 3<>/dev/tcp/127.0.0.1/$RB01X_TCP_PORT" 2>/dev/null; then
  echo "net-tcp: CONNECTED" >> "$res"
else
  echo "net-tcp: denied" >> "$res"
fi

if /usr/bin/env >/dev/null 2>&1; then
  echo "system-exec: pass" >> "$res"
else
  echo "system-exec: FAIL" >> "$res"
fi
exit 0
"#;
    std::fs::write(stub, script).expect("write stub");
    let mut perms = std::fs::metadata(stub)
        .expect("stub metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(stub, perms).expect("chmod stub");
}

#[cfg(target_os = "linux")]
fn parse_results(run_dir: &std::path::Path) -> std::collections::HashMap<String, String> {
    let text = std::fs::read_to_string(run_dir.join("probe-results.txt"))
        .expect("stub wrote its probe results");
    text.lines()
        .filter_map(|l| l.split_once(": "))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[cfg(target_os = "linux")]
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

/// The full suite body — compiled (and run) only where the mechanism exists.
#[cfg(target_os = "linux")]
async fn run_adversarial_suite() {
    const A_TOKEN: &str = "rb01x-a-token";
    const A_ID: &str = "rb01x-a-id";
    const A_USER: &str = "rb01x-a-user";
    const B_TOKEN: &str = "rb01x-b-token";
    const B_ID: &str = "rb01x-b-id";
    const B_USER: &str = "rb01x-b-user";

    const B_SECRET_FILE: &str = "rb01x-b-private.txt";
    const B_SECRET_BODY: &str = "tenant-b-original-contents";

    /// A server-process secret whose exposure channel is `/proc/<pid>/environ`
    /// — the env allowlist (RB-01a) is not enough if the sandboxed tree can
    /// read the server's proc entry, so this dimension gets its own probe.
    const MASTER_KEY_SENTINEL: &str = "rb01x-master-key-sentinel-do-not-leak";

    /// A trivial workflow: the stub binary carries the probes, the shell
    /// text only has to pass admission.
    const PROBE_WORKFLOW: &str = r#"
[workflow]
name = "rb01x-probe"

[[rules]]
name = "probe"
output = ["out.txt"]
shell = "echo rb01x-probe"
"#;

    /// The real-engine leg: one rule that must succeed (writes the run's own
    /// workdir) and one that must fail (reads tenant B's file). The input
    /// chaining forces `steal` to run AFTER `sanity`, so a successful sanity
    /// write is observable even though the run ends failed.
    const REAL_CLI_WORKFLOW_TEMPLATE: &str = r#"
[workflow]
name = "rb01x-real-cli"

[[rules]]
name = "sanity"
output = ["sanity.txt"]
shell = "echo sandbox-ok > sanity.txt"

[[rules]]
name = "steal"
input = ["sanity.txt"]
output = ["stolen.txt"]
shell = "cat {B_FILE} > stolen.txt"
"#;

    let Some(abi) = kernel_landlock_abi().filter(|&abi| abi >= 1) else {
        eprintln!(
            "skipping rb01_sandbox_adversarial: this kernel has no usable Landlock \
             (Linux 5.13+ with CONFIG_SECURITY_LANDLOCK required)"
        );
        return;
    };

    let tmp = std::env::var("CARGO_TARGET_TMPDIR")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    let root = std::path::PathBuf::from(tmp).join(format!("rb01x-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let workspace = root.join("workspace");
    let home = root.join("home");
    std::fs::create_dir_all(&workspace).expect("create workspace root");
    std::fs::create_dir_all(home.join(".ssh")).expect("create server-home skeleton");
    let home_key = home.join(".ssh").join("id_rsa");
    std::fs::write(&home_key, "rb01x-host-credential-sentinel").expect("seed host credential");

    let stub = root.join("oxo-flow-stub");
    write_stub(&stub);

    // Tenant B's private data lives in B's workspace inputs dir.
    let b_inputs = workspace.join("users").join(B_ID).join("inputs");
    std::fs::create_dir_all(&b_inputs).expect("create B inputs dir");
    let b_file = b_inputs.join(B_SECRET_FILE);
    std::fs::write(&b_file, B_SECRET_BODY).expect("seed B file");

    // A live TCP listener the probes will try to reach.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let tcp_port = listener.local_addr().expect("listener addr").port();

    // SAFETY: this binary contains exactly one test, and every `set_var`
    // here runs before the router (and therefore any spawn task) exists.
    unsafe {
        std::env::set_var("OXO_FLOW_WORKSPACE", &workspace);
        std::env::set_var("OXO_FLOW_BIN", &stub);
        std::env::set_var("OXO_FLOW_AI_CONFIG", root.join("hermetic/ai_config.json"));
        // The server account home the probes must NOT reach.
        std::env::set_var("HOME", &home);
        std::env::set_var("OXO_FLOW_MASTER_KEY", MASTER_KEY_SENTINEL);
        // The passthrough chain carries the probe parameters to the stub —
        // the same contract the existing rb01_tenant_isolation suite pins.
        // RB01X_DB_FILE is the SQLite file backing THIS test server (the
        // "server database" dimension; `ensure_db` must run first).
        std::env::set_var(
            "OXO_FLOW_ENV_PASSTHROUGH",
            "RB01X_B_FILE,RB01X_B_DIR,RB01X_DB_FILE,RB01X_SERVER_PID,RB01X_TCP_PORT",
        );
        std::env::set_var("RB01X_B_FILE", &b_file);
        std::env::set_var("RB01X_B_DIR", &b_inputs);
        let db_url = common::db_url();
        let db_path = db_url
            .strip_prefix("sqlite:")
            .and_then(|p| p.split('?').next())
            .expect("db url carries a file path");
        std::env::set_var("RB01X_DB_FILE", db_path);
        std::env::set_var("RB01X_SERVER_PID", std::process::id().to_string());
        std::env::set_var("RB01X_TCP_PORT", tcp_port.to_string());
    }

    common::ensure_db().await;
    oxo_flow_web::domains::execution::handlers::set_run_rate_limit_for_tests(0);
    let app = server::build_router("team");

    seed_identity(A_TOKEN, A_ID, A_USER, "user").await;
    seed_identity(B_TOKEN, B_ID, B_USER, "user").await;

    // ── Control run: sandbox OFF — every probe must LEAK ───────────────────
    // This is the non-vacuity leg: it proves each probe would catch a
    // sandbox regression, and it documents today's unsandboxed behavior.
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
    assert_probe(&control, "workdir-write", "pass", &control_dir);
    for probe in [
        "b-read",
        "b-write",
        "db-read",
        "host-etc",
        "host-home",
        "proc-environ",
    ] {
        assert_ne!(
            control.get(probe).map(String::as_str),
            Some("denied"),
            "CONTROL run must LEAK on {probe} — the probe is broken if it cannot \
             reach the target unsandboxed:\n{}",
            std::fs::read_to_string(control_dir.join("probe-results.txt")).unwrap_or_default()
        );
    }
    assert!(
        b_inputs.join("attempt.txt").exists(),
        "CONTROL run must have written into tenant B's workspace"
    );
    std::fs::remove_file(b_inputs.join("attempt.txt")).expect("clean B dir between runs");

    // ── Sandbox run: OXO_FLOW_WEB_SANDBOX=landlock — every probe DENIED ────
    // SAFETY: the control run has fully reached a terminal state (its spawn
    // already read the environment), so no spawn task can race this write.
    unsafe { std::env::set_var("OXO_FLOW_WEB_SANDBOX", "landlock") };

    let (status, body) = post_run(&app, A_TOKEN, PROBE_WORKFLOW).await;
    assert_eq!(status, StatusCode::OK, "sandbox run accepted: {body}");
    let sandbox_id = body["run_id"].as_str().expect("run_id").to_string();
    assert_eq!(
        wait_terminal(&sandbox_id, 30).await,
        "completed",
        "the stub exits 0 in both modes"
    );
    let sandbox_dir =
        oxo_flow_web::workspace::get_run_directory(A_ID, &sandbox_id).expect("run dir");
    let sandbox = parse_results(&sandbox_dir);

    // Sanities: the sandbox must not break the run's own operations.
    assert_probe(&sandbox, "workdir-write", "pass", &sandbox_dir);
    assert_probe(&sandbox, "system-exec", "pass", &sandbox_dir);
    // The claimed dimensions.
    for probe in [
        "b-read",
        "b-write",
        "db-read",
        "host-etc",
        "host-home",
        "proc-environ",
    ] {
        assert_probe(&sandbox, probe, "denied", &sandbox_dir);
    }
    // Network: denied where the kernel can deny TCP (ABI v4, kernel 6.7+);
    // on older kernels the dimension does not exist and the control
    // behavior persists — recorded, not hidden.
    if abi >= 4 {
        assert_probe(&sandbox, "net-tcp", "denied", &sandbox_dir);
    } else {
        assert_probe(&sandbox, "net-tcp", "CONNECTED", &sandbox_dir);
        eprintln!(
            "NOTE: kernel reports Landlock ABI {abi} (<4) — TCP denial is NOT testable \
             here; the web API reference records this residual"
        );
    }
    // Tenant B's data is byte-identical and B's workspace has no new file.
    assert_eq!(
        std::fs::read_to_string(&b_file).expect("B file readable by the TEST (unsandboxed)"),
        B_SECRET_BODY,
        "tenant A's sandboxed run must not mutate tenant B's file"
    );
    assert!(
        !b_inputs.join("attempt.txt").exists(),
        "tenant A's sandboxed run must not create files in tenant B's workspace"
    );

    // ── Real-engine leg: the actual `oxo-flow` CLI under the sandbox ───────
    // The stub proves kernel-level enforcement of the spawn process; this
    // leg proves the real engine runs end-to-end inside it (rule shells,
    // checkpointing, output writes all inside the granted set) and that a
    // tenant-authored rule READING B's file fails the run.
    let real_cli = workspace_bin("oxo-flow");
    // SAFETY: no spawn task reads OXO_FLOW_BIN between the sandbox run's
    // terminal state and this point.
    unsafe { std::env::set_var("OXO_FLOW_BIN", &real_cli) };
    let workflow = REAL_CLI_WORKFLOW_TEMPLATE.replace("{B_FILE}", &b_file.to_string_lossy());
    let (status, body) = post_run(&app, A_TOKEN, &workflow).await;
    assert_eq!(status, StatusCode::OK, "real-CLI run accepted: {body}");
    let real_id = body["run_id"].as_str().expect("run_id").to_string();
    let real_status = wait_terminal(&real_id, 120).await;
    let real_dir = oxo_flow_web::workspace::get_run_directory(A_ID, &real_id).expect("run dir");
    assert_eq!(
        real_status,
        "failed",
        "the 'steal' rule (cat <B's file>) must fail under the sandbox; \
         execution.log:\n{}",
        std::fs::read_to_string(real_dir.join("execution.log")).unwrap_or_default()
    );
    assert!(
        real_dir.join("sanity.txt").exists(),
        "the rule writing the run's OWN workdir must still succeed under the sandbox"
    );
    assert!(
        !real_dir.join("stolen.txt").exists(),
        "no exfiltrated copy of tenant B's file may exist in the run workdir"
    );
    assert_eq!(
        std::fs::read_to_string(&b_file).expect("B file"),
        B_SECRET_BODY,
        "tenant B's file is untouched by the real-engine leg"
    );

    // The server itself is unsandboxed: its own secret file is still there.
    assert!(home_key.exists(), "host credential sentinel intact");
}
