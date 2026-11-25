//! Hash-chained audit records (issue #1372): SQLite semantics, concurrency,
//! idempotence, and the tamper matrix at the storage level.
//!
//! Each test that needs isolation builds its own file-backed SQLite database
//! (the process-global backend is shared by every `#[tokio::test]` in a binary
//! and cannot be truncated safely). The PostgreSQL arm is gated on
//! `OXO_PG_TEST_URL` and asserts the same chain semantics against a live
//! server; it skips cleanly when the variable is unset.

use std::sync::Arc;

use oxo_flow_core::audit_chain::{
    AuditChainExport, AuditChainVerification, FINDING_HASH_MISMATCH, FINDING_LOG_MISMATCH,
    FINDING_MISSING_LOG_ROW, FINDING_SEQ_MISMATCH, FINDING_UNCHAINED_LOG_ROW, GENESIS_HASH,
    PHASE_FAILURE, PHASE_PENDING, PHASE_SUCCESS, verify_export,
};
use oxo_flow_web::infra::db::StorageBackend;
use oxo_flow_web::infra::db::sqlite::SqliteBackend;
#[cfg(feature = "postgres")]
use uuid::Uuid;

struct FreshBackend {
    // Dropped before the directory: the pool closes, then the tempdir goes.
    backend: SqliteBackend,
    _dir: tempfile::TempDir,
}

async fn fresh_backend() -> FreshBackend {
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("sqlite:{}?mode=rwc", dir.path().join("chain.db").display());
    let backend = SqliteBackend::new(&url).await.expect("sqlite backend");
    backend.init().await.expect("schema init");
    FreshBackend { _dir: dir, backend }
}

async fn snapshot_export(backend: &dyn StorageBackend) -> AuditChainExport {
    backend
        .audit_chain_snapshot()
        .await
        .expect("chain snapshot")
        .into_export()
}

fn codes(verification: &AuditChainVerification) -> Vec<&str> {
    verification
        .findings
        .iter()
        .map(|finding| finding.code.as_str())
        .collect()
}

/// The GLOBAL chain invariant: one gapless sequence starting at 1, every event
/// linked to the immediately preceding event (not merely to a sibling of the
/// same audit id), and a head that equals the last event.
///
/// This is the assertion that makes the concurrency tests meaningful —
/// per-audit adjacency is NOT part of the chain contract, because concurrent
/// writers may interleave events from other audit ids between a pending and
/// its terminal.
fn assert_global_chain_integrity(export: &AuditChainExport) {
    assert!(!export.events.is_empty(), "the chain must have events");
    assert_eq!(
        export.events[0].prev_hash, GENESIS_HASH,
        "the first event must link to genesis"
    );
    for pair in export.events.windows(2) {
        assert_eq!(
            pair[1].seq,
            pair[0].seq + 1,
            "seq {} -> {} has a gap or duplicate",
            pair[0].seq,
            pair[1].seq
        );
        assert_eq!(
            pair[1].prev_hash, pair[0].hash,
            "event {} does not link to the immediately preceding event {}",
            pair[1].seq, pair[0].seq
        );
    }
    let last = export.events.last().expect("non-empty chain");
    assert_eq!(
        export.head.seq, last.seq,
        "head seq must match the last event"
    );
    assert_eq!(
        export.head.hash, last.hash,
        "head hash must match the last event"
    );
}

/// One audit id written by a begin+finalize pair: exactly one pending and one
/// terminal event, in that order. Events of other audit ids may sit between
/// them; adjacency is deliberately NOT required.
fn assert_audit_pair(export: &AuditChainExport, audit_id: &str, terminal_phase: &str) {
    let events: Vec<_> = export
        .events
        .iter()
        .filter(|event| event.audit_id == audit_id)
        .collect();
    assert_eq!(
        events.len(),
        2,
        "expected one pending + one terminal for '{audit_id}', got {events:?}"
    );
    let pending = events
        .iter()
        .find(|event| event.phase == PHASE_PENDING)
        .unwrap_or_else(|| panic!("no pending event for '{audit_id}': {events:?}"));
    let terminal = events
        .iter()
        .find(|event| event.phase == terminal_phase)
        .unwrap_or_else(|| panic!("no '{terminal_phase}' event for '{audit_id}': {events:?}"));
    assert!(
        pending.seq < terminal.seq,
        "pending (seq {}) must precede terminal (seq {}) for '{audit_id}'",
        pending.seq,
        terminal.seq
    );
    assert_eq!(terminal.result, terminal_phase);
    assert!(
        events
            .iter()
            .all(|event| event.phase != PHASE_FAILURE || terminal_phase == PHASE_FAILURE),
        "unexpected failure event for '{audit_id}': {events:?}"
    );
}

async fn append_pair(backend: &dyn StorageBackend, id: &str, actor: &str) {
    backend
        .begin_audit_intent(id, actor, "POST /api/runs", "/api/runs", None)
        .await
        .expect("intent");
    backend
        .finalize_audit_intent(id, "success", Some(r#"{"status":200,"route":"/api/runs"}"#))
        .await
        .expect("finalize");
}

#[tokio::test]
async fn events_are_chained_sequentially_and_verify_clean() {
    let fresh = fresh_backend().await;
    let backend = &fresh.backend;

    append_pair(backend, "chain-1", "chain-actor").await;
    backend
        .begin_audit_intent("chain-2", "chain-actor", "GET /x", "/x", None)
        .await
        .unwrap();
    backend
        .finalize_audit_intent("chain-2", "failure", Some(r#"{"zz":1,"aa":2}"#))
        .await
        .unwrap();
    backend
        .record_audit_log(
            "chain-actor",
            "POST /api/legacy",
            "/api/legacy",
            "success",
            Some(r#"{"b":2,"a":1}"#),
        )
        .await
        .unwrap();

    let export = snapshot_export(backend).await;
    assert_eq!(export.events.len(), 5, "2 pairs + 1 legacy terminal");
    let seqs: Vec<u64> = export.events.iter().map(|event| event.seq).collect();
    assert_eq!(seqs, vec![1, 2, 3, 4, 5], "one gapless chain");
    assert_eq!(export.head.seq, 5);
    assert_eq!(export.head.hash, export.events.last().unwrap().hash);

    let verification = verify_export(&export);
    assert!(verification.ok, "{:?}", verification.findings);
    assert_eq!(verification.events, 5);

    // Stored audit metadata is canonical in BOTH the log row and the event.
    let row = export
        .audit_logs
        .iter()
        .find(|row| row.id == "chain-2")
        .expect("chain-2 log row");
    assert_eq!(row.result, "failure");
    assert_eq!(
        row.metadata.as_ref().map(ToString::to_string).as_deref(),
        Some(r#"{"aa":2,"zz":1}"#)
    );
    let events: Vec<_> = export
        .events
        .iter()
        .filter(|event| event.audit_id == "chain-2")
        .collect();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[1]
            .metadata
            .as_ref()
            .map(ToString::to_string)
            .as_deref(),
        Some(r#"{"aa":2,"zz":1}"#)
    );

    // Export bytes are stable for an unchanged database.
    let first = export.to_canonical_bytes();
    let second = snapshot_export(backend).await.to_canonical_bytes();
    assert_eq!(first, second);
}

#[tokio::test]
async fn concurrent_appends_form_one_gapless_chain() {
    let fresh = fresh_backend().await;
    let backend = Arc::new(fresh.backend.clone());

    let mut tasks = Vec::new();
    for index in 0..16 {
        let backend = Arc::clone(&backend);
        tasks.push(tokio::spawn(async move {
            let id = format!("concurrent-{index}");
            backend
                .begin_audit_intent(&id, "concurrent-actor", "POST /api/x", "/api/x", None)
                .await
                .expect("concurrent intent");
            backend
                .finalize_audit_intent(&id, "success", None)
                .await
                .expect("concurrent finalize");
        }));
    }
    for task in tasks {
        task.await.expect("writer task must not panic");
    }

    let export = snapshot_export(backend.as_ref()).await;
    assert_eq!(export.events.len(), 32, "16 intents + 16 terminals");
    // Global chain contract: 1..=32, each event linked to the immediately
    // preceding event, head equals the last event. Per-audit adjacency is not
    // required: concurrent writers may interleave other audit ids between a
    // pending and its terminal.
    assert_global_chain_integrity(&export);
    let seqs: Vec<u64> = export.events.iter().map(|event| event.seq).collect();
    assert_eq!(seqs, (1..=32).collect::<Vec<u64>>(), "exactly one chain");
    for index in 0..16 {
        assert_audit_pair(&export, &format!("concurrent-{index}"), PHASE_SUCCESS);
    }
    let verification = verify_export(&export);
    assert!(verification.ok, "{:?}", verification.findings);
    assert_eq!(
        export
            .events
            .iter()
            .filter(|event| event.phase == PHASE_SUCCESS)
            .count(),
        16,
        "each intent finalized once"
    );
}

#[tokio::test]
async fn unfinalized_intent_is_pending_and_finalize_is_idempotent() {
    let fresh = fresh_backend().await;
    let backend = &fresh.backend;

    backend
        .begin_audit_intent("crash-1", "crash-actor", "DELETE /x", "/x", None)
        .await
        .unwrap();
    let export = snapshot_export(backend).await;
    assert_eq!(export.events.len(), 1);
    assert_eq!(export.events[0].phase, PHASE_PENDING);
    assert!(export.events[0].metadata.is_none());
    let verification = verify_export(&export);
    assert!(
        verification.ok,
        "an un-finalized intent is a valid pending state, not tampering: {:?}",
        verification.findings
    );

    // First terminal write wins; repeat + conflicting finalizes are no-ops.
    backend
        .finalize_audit_intent("crash-1", "success", Some(r#"{"status":200}"#))
        .await
        .unwrap();
    backend
        .finalize_audit_intent("crash-1", "success", Some(r#"{"status":500}"#))
        .await
        .expect("a repeated finalize is idempotent");
    backend
        .finalize_audit_intent("crash-1", "failure", None)
        .await
        .expect("a conflicting finalize does not append");

    let export = snapshot_export(backend).await;
    assert_eq!(export.events.len(), 2, "one pending + one terminal event");
    assert_eq!(export.events[0].phase, PHASE_PENDING);
    assert_eq!(export.events[1].phase, PHASE_SUCCESS);
    let row = export
        .audit_logs
        .iter()
        .find(|row| row.id == "crash-1")
        .unwrap();
    assert_eq!(row.result, "success", "first terminal outcome wins");
    assert_eq!(
        row.metadata.as_ref().map(ToString::to_string).as_deref(),
        Some(r#"{"status":200}"#),
        "the first finalize's metadata is kept"
    );
    let verification = verify_export(&export);
    assert!(verification.ok, "{:?}", verification.findings);

    // A missing intent is still a broken-trail error.
    assert!(
        backend
            .finalize_audit_intent("no-such-intent", "success", None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn deleting_a_stored_audit_row_is_a_finding() {
    let fresh = fresh_backend().await;
    let backend = &fresh.backend;
    append_pair(backend, "row-1", "row-actor").await;

    sqlx::query("DELETE FROM audit_logs WHERE id = 'row-1'")
        .execute(backend.inner_pool())
        .await
        .unwrap();

    let verification = verify_export(&snapshot_export(backend).await);
    assert!(!verification.ok);
    assert!(
        codes(&verification).contains(&FINDING_MISSING_LOG_ROW),
        "{:?}",
        verification.findings
    );
}

#[tokio::test]
async fn mutating_a_stored_audit_row_is_a_finding() {
    let fresh = fresh_backend().await;
    let backend = &fresh.backend;
    append_pair(backend, "row-1", "row-actor").await;

    sqlx::query("UPDATE audit_logs SET target = '/api/edited' WHERE id = 'row-1'")
        .execute(backend.inner_pool())
        .await
        .unwrap();

    let verification = verify_export(&snapshot_export(backend).await);
    assert!(!verification.ok);
    assert!(
        codes(&verification).contains(&FINDING_LOG_MISMATCH),
        "{:?}",
        verification.findings
    );
}

#[tokio::test]
async fn mutating_a_chain_event_is_a_finding() {
    let fresh = fresh_backend().await;
    let backend = &fresh.backend;
    append_pair(backend, "chain-1", "chain-actor").await;

    sqlx::query("UPDATE audit_chain SET target = '/api/edited' WHERE seq = 2")
        .execute(backend.inner_pool())
        .await
        .unwrap();

    let verification = verify_export(&snapshot_export(backend).await);
    assert!(!verification.ok);
    assert!(
        codes(&verification).contains(&FINDING_HASH_MISMATCH),
        "{:?}",
        verification.findings
    );
}

#[tokio::test]
async fn deleting_a_chain_event_is_a_finding() {
    let fresh = fresh_backend().await;
    let backend = &fresh.backend;
    append_pair(backend, "chain-1", "chain-actor").await;

    sqlx::query("DELETE FROM audit_chain WHERE seq = 1")
        .execute(backend.inner_pool())
        .await
        .unwrap();

    let verification = verify_export(&snapshot_export(backend).await);
    assert!(!verification.ok);
    assert!(
        codes(&verification).contains(&FINDING_SEQ_MISMATCH),
        "{:?}",
        verification.findings
    );
}

#[tokio::test]
async fn a_deleted_head_row_is_a_fail_closed_storage_error() {
    let fresh = fresh_backend().await;
    let backend = &fresh.backend;
    append_pair(backend, "head-1", "head-actor").await;

    sqlx::query("DELETE FROM audit_chain_head WHERE id = 1")
        .execute(backend.inner_pool())
        .await
        .unwrap();

    // A structurally broken chain (missing head row) is deliberately NOT a
    // 200 finding: the snapshot cannot represent it, the web endpoints map the
    // error to 500 DB_ERROR, and every chain write also fails closed.
    assert!(backend.audit_chain_snapshot().await.is_err());
    assert!(
        backend
            .begin_audit_intent("head-2", "head-actor", "POST /api/x", "/api/x", None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn inserting_an_unchained_audit_row_is_a_finding() {
    let fresh = fresh_backend().await;
    let backend = &fresh.backend;
    append_pair(backend, "chain-1", "chain-actor").await;

    sqlx::query(
        "INSERT INTO audit_logs (id, user_id, action, target, result, metadata, timestamp) \
         VALUES ('forged', 'chain-actor', 'POST /api/users', '/api/users', 'success', NULL, ?)",
    )
    // A far-future timestamp is certainly newer than the chain start, so the
    // test cannot flake on a clock step between the two writes.
    .bind("2099-01-01T00:00:00.000000Z")
    .execute(backend.inner_pool())
    .await
    .unwrap();

    let verification = verify_export(&snapshot_export(backend).await);
    assert!(!verification.ok);
    assert!(
        codes(&verification).contains(&FINDING_UNCHAINED_LOG_ROW),
        "{:?}",
        verification.findings
    );
}

/// PostgreSQL parity arm: the same chain semantics through the other backend.
/// Gated on `OXO_PG_TEST_URL`; skips cleanly when unset.
///
/// ```text
/// OXO_PG_TEST_URL=postgres://postgres:postgres@127.0.0.1:5432/oxo_test \
///   cargo test -p oxo-flow-web --features postgres --test audit_chain
/// ```
#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_chain_semantics_match_sqlite() {
    use oxo_flow_web::infra::db::postgres::PostgresBackend;

    let url = match std::env::var("OXO_PG_TEST_URL") {
        Ok(url) if !url.trim().is_empty() => url.trim().to_string(),
        _ => {
            eprintln!("skipping PostgreSQL audit-chain arm: OXO_PG_TEST_URL is not set");
            return;
        }
    };
    let backend = PostgresBackend::new(&url)
        .await
        .expect("connect to live PostgreSQL");
    backend.init().await.expect("schema bootstrap");

    let actor = format!("pg-chain-actor-{}", Uuid::new_v4().simple());
    // Every audit id carries this run's nonce, so a rerun against the same
    // shared database cannot accumulate a second pending+terminal pair for a
    // previously used id (the chain is append-only and never cleared).
    let run = Uuid::new_v4().simple().to_string();
    let base = backend.audit_chain_snapshot().await.expect("baseline");

    // Concurrent appends serialize on the head row (FOR UPDATE).
    let mut tasks = Vec::new();
    for index in 0..8 {
        let backend = backend.clone();
        let actor = actor.clone();
        let run = run.clone();
        tasks.push(tokio::spawn(async move {
            let id = format!("pg-concurrent-{run}-{index}");
            backend
                .begin_audit_intent(&id, &actor, "POST /api/x", "/api/x", None)
                .await
                .expect("PG intent");
            backend
                .finalize_audit_intent(&id, "success", Some(r#"{"status":200}"#))
                .await
                .expect("PG finalize");
        }));
    }
    for task in tasks {
        task.await.expect("PG writer task");
    }

    // Idempotent / first-wins finalize.
    let id = format!("pg-idem-{}", Uuid::new_v4().simple());
    backend
        .begin_audit_intent(&id, &actor, "DELETE /x", "/x", None)
        .await
        .unwrap();
    backend
        .finalize_audit_intent(&id, "success", None)
        .await
        .unwrap();
    backend
        .finalize_audit_intent(&id, "failure", None)
        .await
        .expect("conflicting finalize is a no-op");

    let snapshot = backend.audit_chain_snapshot().await.expect("PG snapshot");
    let export = snapshot.into_export();
    assert!(
        export.head.seq >= base.head.seq + 18,
        "8 pairs (16) + 2 idempotent events must advance the head"
    );

    // The chain contract is GLOBAL: each event links to the immediately
    // preceding event, even though these eight writers race and their events
    // interleave. Per-audit adjacency is not required and is not asserted.
    assert_global_chain_integrity(&export);

    for index in 0..8 {
        assert_audit_pair(
            &export,
            &format!("pg-concurrent-{run}-{index}"),
            PHASE_SUCCESS,
        );
    }
    // The conflicting finalize appended nothing: still exactly one pending and
    // one success terminal.
    assert_audit_pair(&export, &id, PHASE_SUCCESS);

    let verification = verify_export(&export);
    assert!(verification.ok, "{:?}", verification.findings);
}
