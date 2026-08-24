# P0: Execution Truth & Control — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the web control plane truthful about run state and able to actually control runs (cancel/pause/resume signal the real process; statuses, audits, node status, ownership, OAuth state are correct).

**Architecture:** Add a small process-group registry (`process_control.rs`) used by the executor and the execution handlers; unify the run status vocabulary on `completed`; make per-node status derive from the CLI's checkpoint file instead of a never-written DB table; unify the two `audit_logs` schemas; verify OAuth `state` server-side.

**Tech Stack:** Rust (axum, tokio, sqlx/sqlite, libc for signals), oxo-flow-core `CheckpointState`.

**Spec:** `docs/superpowers/specs/2026-08-14-web-full-lifecycle-design.md` §6.1 (defects B1–B5, B8–B10).

## Global Constraints

- Conventional commits (`fix: …` / `feat: …` / `test: …`), no attribution trailer.
- `make ci` gate: `cargo fmt --check` + `cargo clippy --workspace --all-targets -- -D warnings` + tests + audit. Run the clippy/fmt/test cycle per task.
- Run status vocabulary: `queued | running | paused | completed | failed | cancelled` (write `completed`, never `success`).
- Web handler conventions: `err(status, code, msg)` for errors, `now_iso()` for timestamps, `crate::infra::db::sqlite::try_pool()` for the pool, `crate::broadcast_event(name, &json)` for SSE.
- Do not touch: legacy `handlers/` (except nothing here needs it), CLI crate, frontend (P0 is backend-only).
- TDD: red test first, verify it fails, then implement (project testing rule).

---

### Task 1: Process-group registry (`process_control.rs`) — foundation for B1

**Files:**
- Create: `crates/oxo-flow-web/src/process_control.rs`
- Modify: `crates/oxo-flow-web/src/lib.rs` (add `pub mod process_control;`)
- Modify: `crates/oxo-flow-web/Cargo.toml` (add `libc = "0.2"` under `[dependencies]` — check first whether `[workspace.dependencies]` already pins it; reuse that)
- Test: unit tests inline in `process_control.rs`

**Interfaces:**
- Consumes: nothing (leaf module).
- Produces:
  - `pub fn register(run_id: &str, pgid: i32)`
  - `pub fn unregister(run_id: &str)`
  - `pub fn pgid(run_id: &str) -> Option<i32>`
  - `pub fn signal_group(pgid: i32, sig: i32) -> std::io::Result<()>` — sends `sig` to the whole process group (`libc::kill(-pgid, sig)`)
  - constants re-exported for callers: `pub const SIGTERM: i32 = libc::SIGTERM; pub const SIGKILL: i32 = libc::SIGKILL; pub const SIGSTOP: i32 = libc::SIGSTOP; pub const SIGCONT: i32 = libc::SIGCONT;`

- [ ] **Step 1: Write the failing test** — in the new module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
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
        let child = spawn_sleep_child();
        let pid = child.id() as i32;
        register("run-1", pid);
        assert_eq!(pgid("run-1"), Some(pid));
        unregister("run-1");
        assert_eq!(pgid("run-1"), None);
        let _ = signal_group(pid, SIGKILL);
        let _ = child.wait();
    }

    #[test]
    fn sigstop_freezes_group_and_sigcont_resumes() {
        let mut child = spawn_sleep_child();
        let pgid = child.id() as i32;
        // STOP the group; the child must NOT exit while stopped.
        signal_group(pgid, SIGSTOP).expect("sigstop");
        thread::sleep(Duration::from_millis(300));
        assert!(child.try_wait().expect("try_wait").is_none(), "stopped child must not exit");
        // CONT the group; the child exits shortly after (sleep 5 → kill).
        signal_group(pgid, SIGCONT).expect("sigcont");
        signal_group(pgid, SIGKILL).expect("sigkill");
        child.wait().expect("wait");
    }
}
```

- [ ] **Step 2: Run to verify failure**
  Run: `cargo test -p oxo-flow-web process_control`
  Expected: compile error — module does not exist.

- [ ] **Step 3: Implement the module**

```rust
//! Registry of running CLI subprocess groups, keyed by run id.
//!
//! The executor spawns each run's `oxo-flow` CLI in its own process group
//! (`process_group(0)`), so signaling the group reaches the CLI and every
//! rule subprocess it spawned — the same group semantics the engine's own
//! timeout enforcement uses. Handlers look the group up here to
//! cancel (SIGTERM → SIGKILL) or pause/resume (SIGSTOP/SIGCONT) a run.

use std::collections::HashMap;
use std::io;
use std::sync::{OnceLock, RwLock};

pub const SIGTERM: i32 = libc::SIGTERM;
pub const SIGKILL: i32 = libc::SIGKILL;
pub const SIGSTOP: i32 = libc::SIGSTOP;
pub const SIGCONT: i32 = libc::SIGCONT;

/// run id → process group id of the live CLI subprocess.
static REGISTRY: OnceLock<RwLock<HashMap<String, i32>>> = OnceLock::new();

fn registry() -> &'static RwLock<HashMap<String, i32>> {
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Track a live subprocess group under `run_id` (the child's PID doubles as
/// its pgid when spawned with `process_group(0)`).
pub fn register(run_id: &str, pgid: i32) {
    registry().write().expect("registry poisoned").insert(run_id.to_string(), pgid);
}

/// Drop tracking after the subprocess has exited.
pub fn unregister(run_id: &str) {
    registry().write().expect("registry poisoned").remove(run_id);
}

/// Current process group id for `run_id`, if the subprocess is still tracked.
pub fn pgid(run_id: &str) -> Option<i32> {
    registry().read().expect("registry poisoned").get(run_id).copied()
}

/// Send `sig` to the entire process group identified by `pgid`.
pub fn signal_group(pgid: i32, sig: i32) -> io::Result<()> {
    // SAFETY: libc::kill with a negative pid targets the process group.
    let rc = unsafe { libc::kill(-pgid, sig) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
```

- [ ] **Step 4: Run to verify pass**
  Run: `cargo test -p oxo-flow-web process_control`
  Expected: both tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/oxo-flow-web/src/process_control.rs crates/oxo-flow-web/src/lib.rs crates/oxo-flow-web/Cargo.toml
git commit -m "feat(web): process-group registry for real run control"
```

---

### Task 2: Executor registers the child and defers to cancel (B1 part 1)

**Files:**
- Modify: `crates/oxo-flow-web/src/executor.rs` (spawn: `process_group(0)` + register; after wait: unregister + status check before writing terminal state)
- Test: inline unit test for the extracted status helper

**Interfaces:**
- Consumes: `process_control::{register, unregister, pgid}`
- Produces: `fn final_status_from_exit(success: bool) -> &'static str` (Task 4 also uses this), and the executor's spawn path is signal-able via the registry.

- [ ] **Step 1: Write the failing test** (for the helper extracted in Step 3):

```rust
    #[test]
    fn exit_success_maps_to_completed() {
        assert_eq!(final_status_from_exit(true), "completed");
        assert_eq!(final_status_from_exit(false), "failed");
    }
```

- [ ] **Step 2: Run to verify failure**
  Run: `cargo test -p oxo-flow-web executor::tests::exit_success_maps_to_completed`
  Expected: compile error — function not defined.

- [ ] **Step 3: Implement**
  In `executor.rs`, add the helper next to `RunFlags`:

```rust
/// Map a subprocess exit status to the run status vocabulary.
fn final_status_from_exit(success: bool) -> &'static str {
    if success { "completed" } else { "failed" }
}
```

  In `spawn_background_run`, before `cmd.stdout(Stdio::from(log_file));`:

```rust
        // New process group: signals from cancel/pause/resume handlers reach
        // the CLI and every rule subprocess it spawns (see process_control).
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
```

  In the `Ok(mut child)` arm, right after the PID-record block:

```rust
                // Register the process group so cancel/pause/resume can signal it.
                if let Some(pid) = child.id() {
                    crate::process_control::register(&run_id, pid as i32);
                }
```

  Replace the `match child.wait().await` success arm's body: keep the same structure but
  (a) call `crate::process_control::unregister(&run_id);` before deciding the final state,
  (b) use `let final_state = final_status_from_exit(status.success());`,
  (c) **before** the final `UPDATE runs SET status = …`:
  check whether a concurrent cancel already set a terminal status, and skip the write + terminal broadcast if so:

```rust
                        // A concurrent cancel sets status='cancelled' and emits
                        // run_cancelled; the exit here is the SIGKILL fallout
                        // and must not overwrite it back to completed/failed.
                        let cancelled: bool =
                            sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE id = ? AND status = 'cancelled'")
                                .bind(&run_id)
                                .fetch_one(db::pool())
                                .await
                                .map(|n: i64| n > 0)
                                .unwrap_or(false);
                        if !cancelled {
                            // ...existing UPDATE + broadcast_event(final event) go here...
                        } else {
                            info!("Run {run_id} exited after cancel; keeping 'cancelled'");
                        }
```

  Keep the `extract_invalidation_summary` computation inside the `!cancelled` branch.

- [ ] **Step 4: Run to verify pass**
  Run: `cargo test -p oxo-flow-web executor::tests` then `cargo clippy -p oxo-flow-web --all-targets -- -D warnings`
  Expected: all pass, no warnings (the `#[cfg(unix)]` block needs `process_group`; on macOS/Linux this compiles; keep the cfg so non-unix builds still work).

- [ ] **Step 5: Commit**

```bash
git add crates/oxo-flow-web/src/executor.rs
git commit -m "feat(web): executor registers run process group, defers to cancel state"
```

---

### Task 3: Real cancel/pause/resume through the handlers (B1 part 2)

**Files:**
- Modify: `crates/oxo-flow-web/src/domains/execution/handlers.rs` (cancel_run :762, pause_run :816, resume_run :882)
- Test: `crates/oxo-flow-web/tests/process_control_integration.rs` (new; follows the `tests/web_integration.rs` harness pattern — note: that harness exists in the repo per memory; match its setup, incl. `OXO_FLOW_BIN` shim)

**Interfaces:**
- Consumes: `process_control::{pgid, signal_group, SIGTERM, SIGKILL, SIGSTOP, SIGCONT}`
- Produces: handlers now emit `run_cancelled` SSE; cancel leaves the executor's exit path respecting `cancelled` (Task 2).

- [ ] **Step 1: Write the failing test** — new integration file (pattern: spawn router in "personal" mode with a temp workspace; create a run row whose registry entry points at a `sleep 30` child spawned in its own group by the test):

```rust
//! Real process control: cancel/pause/resume must signal the CLI subprocess.
//!
//! The executor's CLI lookup honors OXO_FLOW_BIN first, but these tests
//! bypass spawning entirely: they register a `sleep` child's pgid under a
//! run id the same way the executor does, then drive the HTTP handlers.

use oxo_flow_web::process_control;

async fn test_router() -> axum::Router {
    // Mirror tests/web_integration.rs router setup for "personal" mode.
    oxo_flow_web::server::build_router("personal")
}

fn insert_run_row(pool: &sqlx::SqlitePool, id: &str, status: &str) {
    // Match the infra/db/sqlite.rs runs DDL (12 columns, defaults on the rest).
    futures::executor::block_on(async {
        sqlx::query(
            "INSERT INTO runs (id, user_id, workflow_name, status, pid, started_at, created_at)
             VALUES (?, 'default', 'control-test', ?, NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .bind(id)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
    });
}
```

  Concrete assertions:
  - `cancel_signals_the_process_group`: register a `sleep 30` child pgid under run id "pc-cancel"; `POST /api/runs/pc-cancel/cancel` → 200, response `status == "cancelled"`; the child must exit within ~7 s (SIGTERM kills `sleep` immediately, so assert exit < 7s); DB row status `cancelled` and `finished_at` set; registry no longer returns the pgid.
  - `pause_freezes_then_resume_continues`: register `sleep 30` under "pc-pause"; `POST /api/runs/pc-pause/pause` → 200; the child must NOT exit within 1 s; `POST /api/runs/pc-pause/resume` → 200; then `signal_group(pgid, SIGKILL)` from the test to clean up and assert the child exits.
  - `cancel_unknown_run_is_404`: `POST /api/runs/does-not-exist/cancel` → 404, code `NOT_FOUND`.

- [ ] **Step 2: Run to verify failure**
  Run: `cargo test -p oxo-flow-web --test process_control_integration`
  Expected: fail — today cancel only writes the DB (the `sleep` child stays alive; first assertion times out).

- [ ] **Step 3: Implement**
  In `cancel_run`, replace the `Some(_r) =>` block body with:

```rust
        Some(_r) => {
            // Signal the live process group first (grace SIGTERM, then SIGKILL).
            if let Some(pgid) = crate::process_control::pgid(&id) {
                if let Err(e) = crate::process_control::signal_group(pgid, crate::process_control::SIGTERM) {
                    tracing::warn!("SIGTERM failed for run {id} pgid {pgid}: {e}");
                }
                // Grace window before the kill escalation.
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                if crate::process_control::pgid(&id).is_some()
                    && let Err(e) = crate::process_control::signal_group(pgid, crate::process_control::SIGKILL)
                {
                    tracing::warn!("SIGKILL failed for run {id} pgid {pgid}: {e}");
                }
                crate::process_control::unregister(&id);
            } else {
                tracing::warn!("cancel for run {id}: no live process group registered (already finished or server restarted)");
            }
            let now = now_iso();
            // ...existing UPDATE to 'cancelled' + finished_at...
            crate::broadcast_event(
                "run_cancelled",
                &serde_json::json!({"run_id": id, "cancelled_at": now}),
            );
            // ...existing Ok(Json(...))...
        }
```

  In `pause_run`, after the run-exists check and before the DB UPDATE:

```rust
    if let Some(pgid) = crate::process_control::pgid(&id) {
        if let Err(e) = crate::process_control::signal_group(pgid, crate::process_control::SIGSTOP) {
            return Err(err(
                StatusCode::CONFLICT,
                "PAUSE_ERROR",
                format!("Failed to pause run {id}: {e}"),
            ));
        }
    }
```

  In `resume_run`, symmetrically with `SIGCONT` and code `RESUME_ERROR`.

- [ ] **Step 4: Run to verify pass**
  Run: `cargo test -p oxo-flow-web --test process_control_integration` then `cargo clippy -p oxo-flow-web --all-targets -- -D warnings`
  Expected: all pass. (If the pause test's 1 s window is flaky under load, widen to 2 s and rerun twice — do not weaken the assertion below 1 s.)

- [ ] **Step 5: Commit**

```bash
git add crates/oxo-flow-web/src/domains/execution/handlers.rs crates/oxo-flow-web/tests/process_control_integration.rs
git commit -m "feat(web): cancel/pause/resume signal the live run process group"
```

---

### Task 4: Status vocabulary unification (B2)

**Files:**
- Modify: `crates/oxo-flow-web/src/executor.rs` (done in Task 2 — verify `final_status_from_exit` is the only writer)
- Modify: `crates/oxo-flow-web/src/infra/db/sqlite.rs` (legacy-row migration in the init path near `rebuild_runs_table`)
- Modify: `crates/oxo-flow-web/src/infra/db/models.rs` (doc comment for status vocabulary, `:36`)
- Test: sqlite unit test for the migration

**Interfaces:**
- Consumes: nothing new.
- Produces: every terminal run row is one of `completed | failed | cancelled`.

- [ ] **Step 1: Write the failing test** — in `sqlite.rs` tests module (uses the file-scoped in-memory pool helpers already present):

```rust
    #[tokio::test]
    async fn legacy_success_status_migrates_to_completed() {
        let pool = /* existing helper creating the pool + schema */;
        sqlx::query("INSERT INTO runs (id, user_id, workflow_name, status, pid, started_at, finished_at, created_at) VALUES ('legacy-1', 'default', 'wf', 'success', NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', '2026-01-01T00:00:00Z')")
            .execute(&pool).await.unwrap();
        run_status_migration(&pool).await;          // function added in Step 3
        let status: String = sqlx::query_scalar("SELECT status FROM runs WHERE id = 'legacy-1'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(status, "completed");
        // Non-success rows untouched.
        sqlx::query("UPDATE runs SET status = 'failed' WHERE id = 'legacy-1'")
            .execute(&pool).await.unwrap();
        run_status_migration(&pool).await;
        let status: String = sqlx::query_scalar("SELECT status FROM runs WHERE id = 'legacy-1'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(status, "failed");
    }
```

- [ ] **Step 2: Run to verify failure**
  Run: `cargo test -p oxo-flow-web infra::db::sqlite`
  Expected: compile error — `run_status_migration` undefined.

- [ ] **Step 3: Implement**
  In `sqlite.rs`, near `rebuild_runs_table`:

```rust
/// One-time vocabulary fix: legacy executors wrote `success` for completed
/// runs; the canonical terminal set is `completed | failed | cancelled`.
/// Idempotent — safe to run on every startup.
pub async fn run_status_migration(pool: &SqlitePool) -> Result<(), String> {
    sqlx::query("UPDATE runs SET status = 'completed' WHERE status = 'success'")
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}
```

  Call it inside the existing init (right after `rebuild_runs_table`).
  Update `models.rs` status doc: `"queued"|"running"|"paused"|"completed"|"failed"|"cancelled"`.
  Grep the web crate for remaining `"success"` writes into `runs.status`: `grep -rn "SET status = 'success'\|'success'" crates/oxo-flow-web/src --include=*.rs` and fix any (node-level success strings are unrelated — only the `runs` table matters).

- [ ] **Step 4: Run to verify pass**
  Run: `cargo test -p oxo-flow-web infra::db::sqlite` + `cargo clippy -p oxo-flow-web --all-targets -- -D warnings`
  Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/oxo-flow-web/src/infra/db/sqlite.rs crates/oxo-flow-web/src/infra/db/models.rs crates/oxo-flow-web/src/executor.rs
git commit -m "fix(web): unify run status vocabulary on completed"
```

---

### Task 5: Checkpoint-derived node status; drop `run_nodes` (B5)

**Files:**
- Create: `crates/oxo-flow-web/src/domains/execution/checkpoint_status.rs`
- Modify: `crates/oxo-flow-web/src/domains/execution/mod.rs` (re-export the new module)
- Modify: `crates/oxo-flow-web/src/domains/execution/handlers.rs` (5 read sites: :306 get_run_status, :398 get_dag_status, :528 get_diagnostics, :711 get_run, :974 get_ai_status)
- Modify: `crates/oxo-flow-web/src/infra/db/mod.rs` (:83-92 write APIs + `get_run_nodes`), `crates/oxo-flow-web/src/infra/db/sqlite.rs` (drop `run_nodes` DDL + impls), `crates/oxo-flow-web/src/infra/db/postgres.rs` (drop impls), `crates/oxo-flow-web/src/infra/db/models.rs` (`RunNodeRow`)
- Test: unit tests inline in `checkpoint_status.rs` (fixture checkpoint JSON + fixture log)

**Interfaces:**
- Consumes: `oxo_flow_core::executor::checkpoint::{CheckpointState, BenchmarkRecord}` — `CheckpointState::load_from_file`, `CheckpointState::default_path(workdir)`; existing `NodeStatusItem`/`NodeStatus` from `execution/types.rs`.
- Produces:
  - `pub fn load_node_statuses(run_dir: &std::path::Path, is_running: bool) -> Vec<NodeStatusItem>`

- [ ] **Step 1: Write the failing tests** — inline in the new module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_checkpoint(dir: &std::path::Path, json: &str) {
        let dir = dir.join(".oxo-flow");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("checkpoint.json"), json).unwrap();
    }

    #[test]
    fn maps_completed_failed_and_pending_from_checkpoint() {
        let dir = std::env::temp_dir().join("cp-test-1");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_checkpoint(&dir, r#"{
            "completed_rules": ["fastqc"],
            "failed_rules": ["align"],
            "benchmarks": {"fastqc": {"rule": "fastqc", "wall_time_secs": 1.5, "retries": 0}}
        }"#);
        let items = load_node_statuses(&dir, false);
        assert_eq!(items.len(), 2);
        let fastqc = items.iter().find(|i| i.rule == "fastqc").unwrap();
        assert!(matches!(fastqc.status, NodeStatus::Success));
        assert_eq!(fastqc.duration_ms, Some(1500));
        let align = items.iter().find(|i| i.rule == "align").unwrap();
        assert!(matches!(align.status, NodeStatus::Failed));
    }

    #[test]
    fn running_rules_come_from_execution_log() {
        let dir = std::env::temp_dir().join("cp-test-2");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_checkpoint(&dir, r#"{"completed_rules": ["fastqc"], "failed_rules": [], "benchmarks": {}}"#);
        fs::write(dir.join("execution.log"), "Running: align\n✓ fastqc (0.1s)\n").unwrap();
        let items = load_node_statuses(&dir, true);
        let align = items.iter().find(|i| i.rule == "align").unwrap();
        assert!(matches!(align.status, NodeStatus::Running));
        // Without a live run, the same log must not claim anything is running.
        let items = load_node_statuses(&dir, false);
        let align = items.iter().find(|i| i.rule == "align").unwrap();
        assert!(matches!(align.status, NodeStatus::Pending));
    }

    #[test]
    fn missing_checkpoint_yields_empty() {
        let dir = std::env::temp_dir().join("cp-test-3");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(load_node_statuses(&dir, false).is_empty());
    }
}
```

- [ ] **Step 2: Run to verify failure**
  Run: `cargo test -p oxo-flow-web checkpoint_status`
  Expected: compile error — module missing.

- [ ] **Step 3: Implement**
  `checkpoint_status.rs`:

```rust
//! Per-rule run status derived from the engine's own checkpoint state.
//!
//! The CLI owns execution and writes `.oxo-flow/checkpoint.json` after each
//! rule completes. This module reads that state directly — it is the single
//! source of truth for which rules completed or failed, with per-rule wall
//! time from the benchmark records. Currently-running rules are surfaced by
//! matching the CLI's "Running: <rule>" lines in execution.log (valid only
//! while the run is live). There is no web-side state to drift.

use std::path::Path;

use oxo_flow_core::executor::checkpoint::CheckpointState;

use super::types::{NodeStatus, NodeStatusItem};

/// Derive node statuses from the run's checkpoint file.
///
/// `is_running` gates the execution.log scan: a finished run must not report
/// anything as still running.
pub fn load_node_statuses(run_dir: &Path, is_running: bool) -> Vec<NodeStatusItem> {
    let checkpoint = CheckpointState::load_from_file(&CheckpointState::default_path(run_dir))
        .unwrap_or_else(|_| CheckpointState::new());

    let mut running: Vec<String> = Vec::new();
    if is_running
        && let Ok(log) = std::fs::read_to_string(run_dir.join("execution.log"))
    {
        for line in log.lines() {
            if let Some(rest) = line.strip_prefix("Running: ") {
                running.push(rest.trim().to_string());
            }
        }
    }

    let mut items: Vec<NodeStatusItem> = Vec::new();
    for rule in &checkpoint.completed_rules {
        items.push(NodeStatusItem {
            rule: rule.clone(),
            status: NodeStatus::Success,
            started_at: None,
            duration_ms: checkpoint
                .benchmarks
                .get(rule)
                .map(|b| (b.wall_time_secs * 1000.0).round() as u64),
            exit_code: None,
            progress_pct: None,
        });
    }
    for rule in &checkpoint.failed_rules {
        items.push(NodeStatusItem {
            rule: rule.clone(),
            status: NodeStatus::Failed,
            started_at: None,
            duration_ms: None,
            exit_code: None,
            progress_pct: None,
        });
    }
    for rule in &running {
        items.push(NodeStatusItem {
            rule: rule.clone(),
            status: NodeStatus::Running,
            started_at: None,
            duration_ms: None,
            exit_code: None,
            progress_pct: None,
        });
    }
    items
}
```

  (Verify the exact `NodeStatusItem`/`NodeStatus` field names against `execution/types.rs` and adjust; the test asserts the semantics above.)

  Then replace the 5 `run_nodes` reads: fetch the run row (already done at each site) and compute
  `let node_items = checkpoint_status::load_node_statuses(&run_dir, run.status == "running");`
  where `run_dir` comes from `workspace.rs` (`get_run_directory(&run.username?, …)`) — check how `get_run_logs` (:604) resolves the run dir and reuse that resolution (extract a small `fn run_dir_for(run: &models::RunRow) -> PathBuf` in handlers.rs if not already present).
  Remove the now-unused `run_nodes` queries + `NodeStatusItem` mapping code at each site; `get_dag_status` keeps its DAG from `pipeline_snapshot` and colors nodes from the derived statuses (`Success→green`, `Running→blue`, `Failed→red`, `Pending→lightgray`).
  Finally delete `run_nodes` DDL + impls + `RunNodeRow` + trait methods (`create_run_node`/`update_run_node`/`get_run_nodes`) from `infra/db/{mod,sqlite,postgres}.rs`.

- [ ] **Step 4: Run to verify pass**
  Run: `cargo test -p oxo-flow-web` + `cargo clippy --workspace --all-targets -- -D warnings`
  Expected: full crate green; no unused-import warnings from the removed code paths.

- [ ] **Step 5: Commit**

```bash
git add crates/oxo-flow-web/src/domains/execution/ crates/oxo-flow-web/src/infra/db/
git commit -m "feat(web): node status derived from engine checkpoint; drop dead run_nodes table"
```

---

### Task 6: Audit schema unification + complete insert_run (B3, B4)

**Files:**
- Modify: `crates/oxo-flow-web/src/db.rs` (audit_logs DDL :~115-125, `log_action`, `insert_run` :455-469)
- Modify: `crates/oxo-flow-web/src/infra/db/sqlite.rs` (audit_logs DDL :290-298)
- Test: integration test in `tests/` (or sqlite unit test) that calls the sqlite `log_action` after `db::init_db` created the schema first

**Interfaces:**
- Consumes: existing init functions.
- Produces: one `audit_logs` shape — `(id, user_id, action, target, result TEXT NOT NULL DEFAULT 'success', metadata TEXT, timestamp)`.

- [ ] **Step 1: Write the failing test** — add to `crates/oxo-flow-web/tests/phase1_integration.rs` (or a new `audit_integration.rs`):

```rust
#[tokio::test]
async fn audit_log_action_survives_both_init_paths() {
    // Production boots db::init_db (creates audit_logs first) then
    // infra::db::sqlite::init_pool. Previously the two schemas disagreed on
    // columns and log_action's INSERT failed at runtime.
    let tmp = std::env::temp_dir().join(format!("oxo-audit-{}", uuid::Uuid::new_v4()));
    // (Set the DB path env/state the web crate uses — follow the existing
    // integration harness setup so init_db + init_pool target tmp.)
    oxo_flow_web::db::init_db();            // runs first, wins the CREATE TABLE
    oxo_flow_web::infra::db::sqlite::init_pool(); // second, must no-op cleanly
    let pool = oxo_flow_web::infra::db::sqlite::try_pool().unwrap();
    let backend = oxo_flow_web::infra::db::sqlite::SqliteBackend::from_pool(pool.clone());
    backend.log_action("default", "test.action", "test-target").await
        .expect("log_action must succeed with unified schema");
}
```

  (Check the actual `SqliteBackend` constructor + trait import names in `infra/db/mod.rs` and adjust.)

- [ ] **Step 2: Run to verify failure**
  Run: `cargo test -p oxo-flow-web --test phase1_integration audit_log_action_survives_both_init_paths`
  Expected: fail — "table audit_logs has no column named metadata" (when db.rs creates first).

- [ ] **Step 3: Implement**
  In `db.rs` audit DDL add the column after `target`:

```sql
        CREATE TABLE IF NOT EXISTS audit_logs (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            action TEXT NOT NULL,
            target TEXT NOT NULL,
            result TEXT NOT NULL DEFAULT 'success',
            metadata TEXT,
            timestamp TEXT NOT NULL,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
        );
```

  In `db.rs` `log_action`, extend the INSERT to bind `metadata = NULL` (keep `result = 'success'`).
  In `sqlite.rs` audit DDL add `result TEXT NOT NULL DEFAULT 'success',` before `metadata`, and extend `log_action`'s INSERT to include `result` bound to `"success"`.
  In `db.rs` `insert_run`, insert all columns:

```rust
    sqlx::query(
        "INSERT INTO runs (id, user_id, pipeline_id, pipeline_snapshot, workflow_name, status, phase, pid, workdir, started_at, finished_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&run.id)
    .bind(&run.user_id)
    .bind(run.pipeline_id.as_deref().unwrap_or(""))
    .bind(&run.pipeline_snapshot)
    .bind(&run.workflow_name)
    .bind(&run.status)
    .bind(&run.phase)
    .bind(run.pid)
    .bind(run.workdir.as_deref().unwrap_or(""))
    .bind(run.started_at)
    .bind(run.finished_at)
    .bind(run.created_at)
    .execute(pool())
    .await?;
```

  (Match the exact `Run` struct field names/types in `db.rs` first — adjust `pipeline_id`/`workdir` Option handling to the struct.)

- [ ] **Step 4: Run to verify pass**
  Run: `cargo test -p oxo-flow-web` + `cargo clippy -p oxo-flow-web --all-targets -- -D warnings`
  Expected: green.

- [ ] **Step 5: Commit**

```bash
git add crates/oxo-flow-web/src/db.rs crates/oxo-flow-web/src/infra/db/sqlite.rs crates/oxo-flow-web/tests/
git commit -m "fix(web): unify audit_logs schema across init paths; insert_run fills all columns"
```

---

### Task 7: OAuth state verification (B8)

**Files:**
- Modify: `crates/oxo-flow-web/src/domains/auth/service.rs` (`initiate_oauth` :178 — persists the state; `handle_oauth_callback` :193 — verifies + consumes)
- Modify: `crates/oxo-flow-web/src/domains/auth/handlers.rs` (`oauth_callback` :355 — keep the non-empty check as defense in depth; the service now does the real check)
- Modify: `crates/oxo-flow-web/src/infra/db/sqlite.rs` (DDL: `CREATE TABLE IF NOT EXISTS oauth_states (state TEXT PRIMARY KEY, created_at TEXT NOT NULL);`)
- Test: integration test — authorize stores state, callback with a wrong state fails `OAUTH_INVALID_STATE`, callback with the issued state is consumed (second use fails)

**Interfaces:**
- Consumes: existing `initiate_oauth(provider, redirect_uri)` / `handle_oauth_callback(provider, code, state, redirect_uri)` signatures (keep them; the state already flows through).
- Produces: same signatures; new behavior only.

- [ ] **Step 1: Write the failing test** — in a new `tests/oauth_state_integration.rs` (follow phase3 harness for building the router; no real OAuth network needed — the wrong-state path must fail **before** any token exchange):

```rust
#[tokio::test]
async fn callback_rejects_unissued_state() {
    let app = /* personal/team router */;
    let resp = app.clone().oneshot(
        Request::builder().method("POST").uri("/api/auth/oauth/callback")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"provider":"orcid","code":"x","state":"attacker-state"}"#))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    // body code == "OAUTH_INVALID_STATE"
}
```

  (Also a positive-path test exercising only the storage/verification helpers directly — `service`-level unit tests — since a full OAuth exchange needs a provider.)

- [ ] **Step 2: Run to verify failure**
  Run: `cargo test -p oxo-flow-web --test oauth_state_integration`
  Expected: fail — today a non-empty state passes through to the exchange attempt.

- [ ] **Step 3: Implement**
  In `auth/service.rs` `initiate_oauth`, after `let state = generate_token();`:

```rust
    // Persist the pending state so the callback can verify it (CSRF defense).
    if let Ok(pool) = crate::infra::db::sqlite::try_pool() {
        let created = crate::domains::auth::handlers::now_iso();
        let _ = sqlx::query("INSERT INTO oauth_states (state, created_at) VALUES (?, ?)")
            .bind(&state)
            .bind(created)
            .execute(pool)
            .await;
    }
```

  In `handle_oauth_callback`, before the token exchange (replace the `let _ = state;`):

```rust
    // Verify the CSRF state was issued by this server and consume it (single use).
    let pool = crate::infra::db::sqlite::try_pool()
        .map_err(|_| "Database unavailable".to_string())?;
    let deleted: i64 = sqlx::query_scalar("DELETE FROM oauth_states WHERE state = ? RETURNING COUNT(*)")
        .bind(state)
        .fetch_one(pool)
        .await
        .map_err(|e| format!("State verification failed: {e}"))?;
    if deleted == 0 {
        return Err("Invalid or expired CSRF state".into());
    }
```

  (If the sqlx feature set lacks `RETURNING` support for this query shape, use a SELECT + DELETE pair — prefer `DELETE … RETURNING` first since sqlx/sqlite supports it.)
  Add the `oauth_states` DDL in `sqlite.rs` next to `sessions`.
  Keep the handler's non-empty check.

- [ ] **Step 4: Run to verify pass**
  Run: `cargo test -p oxo-flow-web` + `cargo clippy --workspace --all-targets -- -D warnings`
  Expected: green. Also run the existing `phase3_collaboration_integration` auth tests — they must still pass.

- [ ] **Step 5: Commit**

```bash
git add crates/oxo-flow-web/src/domains/auth/ crates/oxo-flow-web/src/infra/db/sqlite.rs crates/oxo-flow-web/tests/oauth_state_integration.rs
git commit -m "fix(web): verify and consume OAuth state server-side"
```

---

### Task 8: save_pipeline ownership + real effective AI config (B9, B10)

**Files:**
- Modify: `crates/oxo-flow-web/src/server.rs` (auth middleware :498-583 — attach the authenticated user id as `Extension<Option<String>>` on success; use `axum::Extension`)
- Modify: `crates/oxo-flow-web/src/domains/workflow/handlers.rs` (`save_pipeline` :280-310 — resolve owner: extension user → `default`)
- Modify: `crates/oxo-flow-web/src/domains/ai/handlers.rs` (`get_ai_config_effective` :340 — read the user row (`user_id='default'` or session user) and server row from `ai_provider_config`; fill `user_provider`; keep the env tier)

**Interfaces:**
- Consumes: existing `require_auth` middleware, `ai_provider_config` table.
- Produces: `save_pipeline` rows owned by the acting user; `config/effective.tiers.user_provider` real.

- [ ] **Step 1: Write the failing tests**
  - `tests/phase4_v09_integration.rs` (or new file): `save_pipeline_uses_default_owner_in_personal_mode` — POST a pipeline, then GET `/api/pipelines` and assert the row exists (owner invisible through the API in personal mode; assert at DB level: `SELECT user_id FROM pipelines WHERE id = ?` → `"default"`, **not** the admin uuid). Today it returns the admin uuid — that's the red assertion.
  - `get_ai_config_effective` unit/integration: with a user row inserted (`user_id='default'`, provider `deepseek`), the response `tiers.user_provider` must be `"deepseek"` (today: always null — red).

- [ ] **Step 2: Run to verify failure**
  Run: both new tests
  Expected: red per above.

- [ ] **Step 3: Implement**
  Middleware: in the success branch of token validation, before forwarding:
  `request.extensions_mut().insert(user_id.clone());` — as `Extension<String>`? Prefer `request.extensions_mut().insert::<Option<String>>(Some(user_id));` so handlers use `Option<Extension<Option<String>>>` (absent in personal mode). Read it in `save_pipeline` as:

```rust
    let user_id = match axum::extract::Extension::<Option<String>>::from_request_parts(&mut parts, &state) {
        // (resolve via the request parts pattern the handler signature supports;
        //  alternatively change save_pipeline to take `Option<Extension<Option<String>>>`)
    };
```

  (Pick the concrete extractor shape by checking how the handler currently declares its parameters — if `save_pipeline(Json(req))` only, change it to `save_pipeline(Json(req), Option<Extension<Option<String>>> ext)` and use `ext.flatten().flatten().unwrap_or_else(|| "default".into())`; verify the exact axum version's `Option<Extension<T>>` behavior — with axum 0.7/0.8, `Option<Extension<T>>` works when the layer is always applied; if the middleware applies the layer unconditionally with `Extension<Option<String>>` inserted always (None when unauthenticated), the extractor is just `Extension<Option<String>>`.)
  `get_ai_config_effective`: query the user row like `get_server_ai_config` does (`WHERE user_id = 'default' ORDER BY updated_at DESC LIMIT 1`) and merge into `tiers.user_provider` + the effective resolution (user row → server row → env → default, per the spec's tier order).

- [ ] **Step 4: Run to verify pass**
  Run: `cargo test -p oxo-flow-web` + clippy
  Expected: green.

- [ ] **Step 5: Commit**

```bash
git add crates/oxo-flow-web/src/server.rs crates/oxo-flow-web/src/domains/workflow/handlers.rs crates/oxo-flow-web/src/domains/ai/handlers.rs crates/oxo-flow-web/tests/
git commit -m "fix(web): attribute pipeline ownership to acting user; compute real effective AI config"
```

---

### Final Gate (after Task 8)

- [ ] Run `make ci` (fmt + clippy -D warnings + build + test + audit) — full workspace.
- [ ] Grep sweep: `grep -rn "'success'" crates/oxo-flow-web/src | grep -i "status"` — only node-level success strings may remain; no `runs` status writes.
- [ ] Manual smoke: start the server, create + cancel a real run via curl, confirm the CLI subprocess actually dies (`pgrep -f oxo-flow`).
- [ ] Commit any fixups; update the spec's §2.2 with "fixed in P0 (commit …)" annotations.
