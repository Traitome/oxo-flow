//! Owner leases for in-flight web runs (issue #1402).
//!
//! The web process that spawns a run is the only component that can
//! attribute its outcome: it waits on the CLI, reads the wrapper's
//! `.exit-code` record, and writes the terminal row. When that process is
//! killed (measured: `kill -9` under a running `sleep` rule), nothing in the
//! current design ever finalizes the row — it stays `running` forever, a
//! ghost run. Manual cancellation still works, but automatic reconciliation
//! was missing.
//!
//! # Lease protocol
//!
//! Every process has a process-global [`instance_id`]. A run row records the
//! instance that inserted it (`owner_instance`) and a `heartbeat_at`
//! timestamp. While the process lives, a background task renews the
//! heartbeat of every `queued`/`running`/`paused` row it owns. A periodic
//! reaper finalizes rows whose owner has not heartbeated for
//! [`LEASE_TTL_SECS`] — an owner that has been silent that long cannot
//! attribute the run any more. When the run's workdir is visible from the
//! reaping instance and carries the wrapper's `.exit-code` record, the run
//! is attributed to that real outcome (`completed` for 0); otherwise it is
//! marked `failed`.
//!
//! Safety boundaries (deliberate, not incidental):
//!
//! - A process never reaps its own rows, even when its own heartbeats are
//!   stale (e.g. a long database partition). It is still monitoring them
//!   in-process, and when the database returns it can attribute them.
//! - Terminal rows are never selected: the storage query filters the
//!   live set, and the terminal write itself is first-wins.
//! - Rows written before the lease migration have a NULL owner and are not
//!   selected. They remain manually cancellable, exactly as before; a
//!   rolling upgrade from a pre-lease binary does not misclassify an old
//!   instance's live run.
//!
//! The lease is evidence of liveness of the *owner*, not of the run's
//! process. After the owning web process dies its CLI child may keep
//! running; the reaper finalizes the ledger entry (the orphaned process is
//! not signalled, which is safe across instances and hosts). This tradeoff
//! is documented in the campaign evidence.
//!
//! Heartbeats are written by the owning process and compared by a reaping
//! process, so lease correctness assumes loosely synchronized clocks (NTP,
//! as on any normal server fleet). The 90 s TTL tolerates tens of seconds of
//! skew; a host whose clock is off by more than half the TTL can reap early.
//! The measured campaign is single-host; cross-host clock skew and a
//! multi-host deployment remain unmeasured residuals.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, OnceLock};

use crate::infra::db::StorageBackend;

/// How often a live process renews its runs' heartbeat.
pub const HEARTBEAT_INTERVAL_SECS: u64 = 10;

/// How long an owner may stay silent before another instance may finalize
/// its runs. Nine missed heartbeats: long enough to ride out a transient
/// database stall, short enough that a crashed instance is reconciled
/// within about two minutes.
pub const LEASE_TTL_SECS: u64 = 90;

/// How often every instance looks for stale leases.
pub const REAPER_INTERVAL_SECS: u64 = 30;

/// Upper bound on rows finalized per sweep, so one unresponsive database
/// region cannot stall the reaper loop behind an unbounded batch.
const REAP_BATCH_LIMIT: u32 = 50;

/// Runs in this process that an executor task is actually monitoring
/// (local spawn, SQLite recovery re-attach, or remote dispatch).
///
/// The heartbeat renews only these runs. If `mark_run_running` fails before
/// a spawn (e.g. the pool acquire times out during a database partition),
/// the row would otherwise stay `queued` while this process renews its
/// lease forever; other instances exclude it and the reaper can never
/// recover it (issue #1402 review, MEDIUM).
static MONITORED_RUNS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn register_monitored(run_id: &str) {
    if let Ok(mut runs) = MONITORED_RUNS.lock() {
        runs.insert(run_id.to_string());
    }
}

/// Stop heartbeating a run. Idempotent.
pub(crate) fn unregister_monitored(run_id: &str) {
    if let Ok(mut runs) = MONITORED_RUNS.lock() {
        runs.remove(run_id);
    }
}

fn monitored_run_ids() -> Vec<String> {
    MONITORED_RUNS
        .lock()
        .map(|runs| runs.iter().cloned().collect())
        .unwrap_or_default()
}

/// RAII registration for an executor task.
///
/// Dropping the guard (normal return, early error, task cancellation, or
/// panic) stops the run's heartbeat, leaving it to the lease TTL if no
/// terminal write succeeded.
pub(crate) struct RunMonitorGuard {
    run_id: String,
}

impl RunMonitorGuard {
    pub(crate) fn new(run_id: impl Into<String>) -> Self {
        let run_id = run_id.into();
        register_monitored(&run_id);
        Self { run_id }
    }
}

impl Drop for RunMonitorGuard {
    fn drop(&mut self) {
        unregister_monitored(&self.run_id);
    }
}

/// Process-global instance identity.
///
/// `hostname:pid:uuid` — the UUID makes identity unique across restarts (a
/// restarted process must not silently inherit its predecessor's rows) and
/// across hosts; the host/pid prefix makes an owner identifiable in
/// operator output (the hostname comes from the process environment and is
/// `unknown` when the service manager does not export `HOSTNAME`; pid+uuid
/// remain unique). This is deliberately not configurable: an operator-set
/// id could collide across instances and defeat the lease.
pub fn instance_id() -> &'static str {
    static INSTANCE: OnceLock<String> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        format!(
            "{}:{}:{}",
            oxo_flow_core::executor::process::hostname(),
            std::process::id(),
            uuid::Uuid::new_v4()
        )
    })
}

/// Current UTC timestamp in the RFC 3339 form the run columns store.
pub fn heartbeat_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// RFC 3339 cutoff for stale leases: heartbeats older than this are stale.
pub fn stale_cutoff() -> String {
    (chrono::Utc::now() - chrono::Duration::seconds(LEASE_TTL_SECS as i64)).to_rfc3339()
}

/// Upper bound on run ids per heartbeat statement (SQLite's default
/// variable limit is 999 on older builds; PostgreSQL's is much higher).
/// The monitored set is normally far below this.
const HEARTBEAT_BATCH: usize = 500;

/// Renew the heartbeat of every live run this process is actively
/// monitoring. A no-op when no executor task is registered.
pub async fn heartbeat(backend: &dyn StorageBackend) -> Result<u64, String> {
    let ids = monitored_run_ids();
    if ids.is_empty() {
        return Ok(0);
    }
    let now = heartbeat_timestamp();
    let mut touched = 0u64;
    for chunk in ids.chunks(HEARTBEAT_BATCH) {
        touched += backend.heartbeat_runs(instance_id(), chunk, &now).await?;
    }
    Ok(touched)
}

/// Finalize every live run whose owner lease is stale.
///
/// The orphan's outcome is attributed from the wrapper's `.exit-code`
/// record when the workdir is reachable from this instance — a CLI that
/// outlived the web process and finished cleanly is recorded `completed`,
/// not falsely `failed`. Without a record (still running, or the workdir is
/// not visible from this host) the run is `failed`. Finalization goes
/// through `crate::executor::finalize_run`, the same function the live
/// executor and restart recovery use, so quota release, the terminal
/// first-wins guard, the SSE event, and the webhook all behave identically.
///
/// Returns the number of rows this sweep moved out of the live set.
pub async fn reap_stale(backend: &dyn StorageBackend) -> Result<usize, String> {
    let stale = backend
        .stale_run_leases(instance_id(), &stale_cutoff(), REAP_BATCH_LIMIT)
        .await?;
    let mut reaped = 0usize;
    for run in stale {
        let log_path = run
            .workdir
            .as_deref()
            .map(|workdir| std::path::Path::new(workdir).join("execution.log"))
            .unwrap_or_default();
        // A queued row never executed, so it is never attributed from an
        // exit record (a reused workdir may still carry the predecessor's).
        // For a running row, accept the record only when it postdates the
        // run's `started_at`; consume it so a later run cannot reuse it.
        let exit_code = match (run.workdir.as_deref(), run.started_at.as_deref()) {
            (Some(workdir), Some(started_at)) => {
                let workdir = std::path::PathBuf::from(workdir);
                let started_at = started_at.to_string();
                tokio::task::spawn_blocking(move || {
                    crate::executor::consume_exit_record(&workdir, &started_at)
                })
                .await
                .ok()
                .flatten()
            }
            _ => None,
        };
        tracing::warn!(
            "Run {} lease expired (owner {:?}, last heartbeat {:?}, exit record {:?}) — \
             attributing the orphaned run",
            run.id,
            run.owner_instance,
            run.heartbeat_at,
            exit_code
        );
        // A run the user cancelled in the meantime keeps its `cancelled`
        // row: the terminal write is first-wins. This is a system
        // reconciliation, not a user action, so it writes no `audit_logs`
        // row (the audit chain records API mutations by an acting user).
        crate::executor::finalize_run(&run.id, exit_code, &log_path).await;
        if backend
            .get_run(&run.id)
            .await
            .ok()
            .flatten()
            .is_some_and(|row| crate::infra::db::models::is_terminal_run_status(&row.status))
        {
            reaped += 1;
        }
    }
    Ok(reaped)
}

/// Spawn the heartbeat and reaper loops. Called once from
/// [`crate::start_background_tasks`].
pub fn spawn_background_tasks() {
    tokio::spawn(async {
        let mut ticker =
            tokio::time::interval(std::time::Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // A database outage would otherwise warn every 10 s; log the first
        // failure and then once a minute (every sixth beat).
        let mut failures: u32 = 0;
        loop {
            ticker.tick().await;
            let Some(backend) = crate::infra::db::global_backend() else {
                continue;
            };
            match heartbeat(backend.as_ref()).await {
                Ok(_) => failures = 0,
                Err(e) => {
                    failures = failures.saturating_add(1);
                    if failures == 1 || failures.is_multiple_of(6) {
                        tracing::warn!("Run-lease heartbeat failed ({failures} consecutive): {e}");
                    }
                }
            }
        }
    });
    tokio::spawn(async {
        let mut ticker =
            tokio::time::interval(std::time::Duration::from_secs(REAPER_INTERVAL_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let Some(backend) = crate::infra::db::global_backend() else {
                continue;
            };
            match reap_stale(backend.as_ref()).await {
                Ok(0) => {}
                Ok(n) => tracing::warn!("Reaped {n} run(s) with an expired owner lease"),
                Err(e) => tracing::warn!("Run-lease reaper failed: {e}"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::db::StorageBackend;
    use crate::infra::db::models::RunRow;
    use crate::infra::db::sqlite::SqliteBackend;

    async fn lease_backend() -> (SqliteBackend, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let url = format!("sqlite:{}?mode=rwc", dir.path().join("lease.db").display());
        let backend = SqliteBackend::new(&url).await.expect("backend");
        backend.init().await.expect("schema");
        (backend, dir)
    }

    fn run(id: &str, status: &str, workdir: &std::path::Path) -> RunRow {
        RunRow {
            id: id.to_string(),
            user_id: "default".to_string(),
            pipeline_id: None,
            pipeline_snapshot: "x".to_string(),
            workflow_name: None,
            status: status.to_string(),
            phase: "executing".to_string(),
            dry_run: false,
            pid: None,
            workdir: Some(workdir.to_string_lossy().into_owned()),
            config_overrides: None,
            artifacts_incomplete: None,
            cluster_id: None,
            started_at: Some(chrono::Utc::now().to_rfc3339()),
            finished_at: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    async fn force_lease(backend: &SqliteBackend, id: &str, owner: Option<&str>, heartbeat: &str) {
        sqlx::query("UPDATE runs SET owner_instance = ?, heartbeat_at = ? WHERE id = ?")
            .bind(owner)
            .bind(heartbeat)
            .bind(id)
            .execute(backend.inner_pool())
            .await
            .expect("force lease");
    }

    async fn heartbeat_of(backend: &SqliteBackend, id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT heartbeat_at FROM runs WHERE id = ?")
            .bind(id)
            .fetch_one(backend.inner_pool())
            .await
            .expect("read heartbeat")
    }

    /// MEDIUM regression: only runs with a live executor task are
    /// heartbeated. A queued row left behind by a failed `mark_run_running`
    /// must stop being renewed so the lease expires and another instance
    /// (or this process after the owner dies) can reap it.
    #[tokio::test]
    async fn heartbeat_renews_only_registered_live_runs() {
        let (backend, dir) = lease_backend().await;
        for (id, status) in [
            ("unit-heartbeat-registered", "running"),
            ("unit-heartbeat-unregistered", "queued"),
            ("unit-heartbeat-terminal", "completed"),
        ] {
            let workdir = dir.path().join(id);
            std::fs::create_dir_all(&workdir).expect("workdir");
            backend
                .create_run(&run(id, status, &workdir))
                .await
                .expect("create run");
        }
        let old = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        for id in [
            "unit-heartbeat-registered",
            "unit-heartbeat-unregistered",
            "unit-heartbeat-terminal",
        ] {
            force_lease(&backend, id, Some(instance_id()), &old).await;
        }

        // Only the running row has a registered executor task.
        let _guard = RunMonitorGuard::new("unit-heartbeat-registered");
        let _terminal_guard = RunMonitorGuard::new("unit-heartbeat-terminal");
        let touched = heartbeat(&backend).await.expect("heartbeat");
        assert!(touched >= 1, "the registered live run is renewed");

        assert_ne!(
            heartbeat_of(&backend, "unit-heartbeat-registered")
                .await
                .as_deref(),
            Some(old.as_str()),
            "a registered running row is heartbeated"
        );
        assert_eq!(
            heartbeat_of(&backend, "unit-heartbeat-unregistered")
                .await
                .as_deref(),
            Some(old.as_str()),
            "a queued row with no executor task is NOT heartbeated"
        );
        assert_eq!(
            heartbeat_of(&backend, "unit-heartbeat-terminal")
                .await
                .as_deref(),
            Some(old.as_str()),
            "terminal rows are never heartbeated"
        );
    }

    /// The stale sweep never selects a pre-lease NULL-owner row, selects a
    /// dead owner's stale row, and the claim precondition refuses to
    /// re-own a live instance's fresh row.
    #[tokio::test]
    async fn stale_selection_and_claim_preconditions() {
        let (backend, dir) = lease_backend().await;
        for id in [
            "unit-lease-null-owner",
            "unit-lease-dead-owner",
            "unit-lease-live-owner",
        ] {
            let workdir = dir.path().join(id);
            std::fs::create_dir_all(&workdir).expect("workdir");
            backend
                .create_run(&run(id, "running", &workdir))
                .await
                .expect("create run");
        }
        let old = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        force_lease(&backend, "unit-lease-null-owner", None, &old).await;
        force_lease(&backend, "unit-lease-dead-owner", Some("dead"), &old).await;
        force_lease(
            &backend,
            "unit-lease-live-owner",
            Some("live"),
            &chrono::Utc::now().to_rfc3339(),
        )
        .await;

        let future = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        let stale = backend
            .stale_run_leases("reaper-peer", &future, 100)
            .await
            .expect("stale leases");
        assert!(
            stale.iter().any(|row| row.id == "unit-lease-dead-owner"),
            "a dead owner's live row is selected"
        );
        assert!(
            !stale.iter().any(|row| row.id == "unit-lease-null-owner"),
            "a NULL-owner pre-lease row is never selected"
        );

        // Claim precondition: a live owner's fresh row is not re-owned by a
        // past cutoff, but a stale or NULL owner can be claimed.
        assert!(
            !backend
                .claim_run_owner(
                    "unit-lease-live-owner",
                    "claimant",
                    &chrono::Utc::now().to_rfc3339(),
                    &stale_cutoff(),
                )
                .await
                .expect("claim live"),
            "a live owner's fresh heartbeat cannot be re-owned"
        );
        assert!(
            backend
                .claim_run_owner(
                    "unit-lease-null-owner",
                    "claimant",
                    &chrono::Utc::now().to_rfc3339(),
                    &stale_cutoff(),
                )
                .await
                .expect("claim null owner"),
            "an unowned live row is claimable"
        );
        assert!(
            backend
                .claim_run_owner(
                    "unit-lease-dead-owner",
                    "claimant",
                    &chrono::Utc::now().to_rfc3339(),
                    &future,
                )
                .await
                .expect("claim stale"),
            "a stale owner's row is claimable"
        );
    }

    #[test]
    fn instance_id_is_stable_and_identifying() {
        let id = instance_id();
        assert_eq!(id, instance_id(), "the process identity never changes");
        assert!(
            id.contains(&std::process::id().to_string()),
            "the pid is part of the identity: {id}"
        );
        assert!(
            id.rsplit(':').next().is_some_and(|uuid| uuid.len() >= 32),
            "a random suffix makes restarts distinct: {id}"
        );
    }

    #[test]
    fn stale_cutoff_is_lease_ttl_in_the_past() {
        let cutoff = chrono::DateTime::parse_from_rfc3339(&stale_cutoff())
            .expect("cutoff is RFC 3339")
            .with_timezone(&chrono::Utc);
        let age = chrono::Utc::now() - cutoff;
        assert!(
            (age.num_seconds() - LEASE_TTL_SECS as i64).abs() <= 2,
            "cutoff must sit LEASE_TTL_SECS in the past: {age}"
        );
    }
}
