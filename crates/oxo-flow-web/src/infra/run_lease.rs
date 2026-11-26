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

use std::sync::OnceLock;

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

/// Renew the heartbeat of every live run owned by this process. A no-op
/// error-free path when the process owns no live runs.
pub async fn heartbeat(backend: &dyn StorageBackend) -> Result<u64, String> {
    backend
        .heartbeat_runs(instance_id(), &heartbeat_timestamp())
        .await
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
        let workdir = run.workdir.as_deref().map(std::path::Path::new);
        let log_path = workdir
            .map(|workdir| workdir.join("execution.log"))
            .unwrap_or_default();
        let exit_code = workdir.and_then(crate::executor::read_exit_record);
        tracing::warn!(
            "Run {} lease expired (owner {:?}, last heartbeat {:?}, exit record {:?}) — \
             attributing the orphaned run",
            run.id,
            run.owner_instance,
            run.heartbeat_at,
            exit_code
        );
        // A run the user cancelled in the meantime keeps its `cancelled`
        // row: the terminal write is first-wins.
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
        loop {
            ticker.tick().await;
            let Some(backend) = crate::infra::db::global_backend() else {
                continue;
            };
            if let Err(e) = heartbeat(backend.as_ref()).await {
                // One warning per failed beat: a prolonged database outage
                // would otherwise log every 10 s; this is the signal an
                // operator needs, and the lease TTL handles the rest.
                tracing::warn!("Run-lease heartbeat failed: {e}");
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
