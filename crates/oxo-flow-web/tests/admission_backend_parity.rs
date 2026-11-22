//! Backend-parity harness for the run admission guard (audit RB-02, #1312).
//!
//! Run admission is one guarded
//! `INSERT ... SELECT ... WHERE NOT EXISTS ... ON CONFLICT DO NOTHING` built
//! by a shared SQL builder and backed by the partial unique indexes
//! `idx_runs_live_pipeline` / `idx_runs_live_workdir` on BOTH SQLite and
//! PostgreSQL. Before this harness the two backends had separate,
//! independently-written admission tests; nothing asserted that the SAME
//! scenario yields the SAME verdict on both, so a dialect drift at the guard
//! would be silent.
//!
//! This file drives the SAME scenarios through the SHARED `claim_run_row`
//! helper -- the exact function create/retry/resume call -- against every
//! available backend and asserts each backend's verdict set equals ONE shared
//! canonical contract. It does NOT retype SQL: the guard, the index and the
//! admission policy all stay where they live in the crate.
//!
//! - SQLite arm: unconditional. File-backed pool so the concurrent claims
//!   land on separate connections (an in-memory pool is a single connection
//!   and cannot genuinely race).
//! - PostgreSQL arm: gated on `OXO_PG_TEST_URL`, skips cleanly when unset.
//!
//! What this proves: backend-equivalent admission VERDICTS for the scenarios
//! below -- single process, single host. What it does NOT prove: multi-instance
//! deployment, pool exhaustion, network partitions, or crash recovery.

use oxo_flow_web::domains::execution::handlers::claim_run_row;
use oxo_flow_web::infra::db::StorageBackend;
use oxo_flow_web::infra::db::models;
use oxo_flow_web::infra::db::sqlite::SqliteBackend;
use uuid::Uuid;

#[cfg(feature = "postgres")]
use oxo_flow_web::infra::db::postgres::PostgresBackend;

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn run_row(id: &str, user_id: &str, pipeline_id: Option<&str>, workdir: &str) -> models::RunRow {
    models::RunRow {
        id: id.to_string(),
        user_id: user_id.to_string(),
        pipeline_id: pipeline_id.map(str::to_string),
        pipeline_snapshot: "[workflow]".to_string(),
        workflow_name: Some("parity.oxoflow".to_string()),
        status: "queued".to_string(),
        phase: "parsing".to_string(),
        dry_run: true,
        pid: None,
        workdir: Some(workdir.to_string()),
        config_overrides: None,
        artifacts_incomplete: None,
        cluster_id: None,
        started_at: None,
        finished_at: None,
        created_at: now(),
    }
}

fn pipeline_row(id: &str, user_id: &str) -> models::PipelineRow {
    models::PipelineRow {
        id: id.to_string(),
        user_id: user_id.to_string(),
        name: "parity".to_string(),
        version: "1.0.0".to_string(),
        toml_content: "[workflow]".to_string(),
        rules_count: 0,
        forked_from: None,
        visibility: "private".to_string(),
        created_at: now(),
        updated_at: now(),
        revision: 1,
    }
}

/// The canonical admission contract. EVERY backend arm must reproduce this
/// EXACT verdict vector, so backend-equivalent outcomes are asserted against
/// one shared source of truth rather than against each backend's own
/// expectation (which is how a silent dialect drift would slip through).
const EXPECTED: &[(&str, bool)] = &[
    ("workdir first", true),
    ("workdir second-same", false),
    ("workdir third-different", true),
    ("workdir after-terminal", true),
    ("pipeline first", true),
    ("pipeline second-same-diffwd", false),
    ("pipeline independent", true),
    ("pipeline after-terminal", true),
    ("concurrent exactly-one", true),
    ("rejection is Ok(false) not Err", true),
];

fn unique_username() -> String {
    format!("parity-u-{}", Uuid::new_v4().simple())
}

/// Drive one claim through the SHARED admission helper. A storage-layer error
/// (as opposed to a rejected claim) is a hard failure: the contract is that a
/// lost race is `Ok(false)`, never an error.
async fn claim(backend: &dyn StorageBackend, run: &models::RunRow) -> bool {
    claim_run_row(backend, run)
        .await
        .expect("claim_run_row must report a lost race as Ok(false), never Err")
}

fn wd(tag: &str) -> String {
    format!("/tmp/parity-{tag}-{}", Uuid::new_v4().simple())
}

/// One scenario driver shared by every backend arm.
///
/// Every scenario goes through the SAME `claim_run_row` helper the production
/// create/retry/resume arms call, so the guard, the index and the admission
/// policy all stay where they live in the crate (no retyped SQL). The verdict
/// vector it returns is compared against the ONE canonical `EXPECTED` contract,
/// so a backend that diverges from the other is named exactly.
async fn collect_verdicts(backend: &dyn StorageBackend) -> Vec<(&'static str, bool)> {
    let user = backend
        .create_user(&unique_username(), "user", None)
        .await
        .expect("create_user seeds the run FK parent");
    let mut out: Vec<(&'static str, bool)> = Vec::new();

    // -- workdir-guarded claims (ad-hoc create / retry / resume) -------------
    let wd_a = wd("a");
    let wd_b = wd("b");
    let a1 = Uuid::new_v4().to_string();
    out.push((
        "workdir first",
        claim(backend, &run_row(&a1, &user.id, None, &wd_a)).await,
    ));
    out.push((
        "workdir second-same",
        claim(
            backend,
            &run_row(&Uuid::new_v4().to_string(), &user.id, None, &wd_a),
        )
        .await,
    ));
    out.push((
        "workdir third-different",
        claim(
            backend,
            &run_row(&Uuid::new_v4().to_string(), &user.id, None, &wd_b),
        )
        .await,
    ));
    backend
        .update_run_status(&a1, "completed", "done")
        .await
        .expect("release workdir");
    out.push((
        "workdir after-terminal",
        claim(
            backend,
            &run_row(&Uuid::new_v4().to_string(), &user.id, None, &wd_a),
        )
        .await,
    ));

    // -- pipeline-guarded claims (saved-pipeline create) --------------------
    let p1 = Uuid::new_v4().to_string();
    let p2 = Uuid::new_v4().to_string();
    backend
        .create_pipeline_with_revision(&pipeline_row(&p1, &user.id))
        .await
        .expect("seed pipeline A");
    backend
        .create_pipeline_with_revision(&pipeline_row(&p2, &user.id))
        .await
        .expect("seed pipeline B");
    let r1 = Uuid::new_v4().to_string();
    out.push((
        "pipeline first",
        claim(backend, &run_row(&r1, &user.id, Some(&p1), &wd("p1a"))).await,
    ));
    out.push((
        "pipeline second-same-diffwd",
        claim(
            backend,
            &run_row(&Uuid::new_v4().to_string(), &user.id, Some(&p1), &wd("p1b")),
        )
        .await,
    ));
    out.push((
        "pipeline independent",
        claim(
            backend,
            &run_row(&Uuid::new_v4().to_string(), &user.id, Some(&p2), &wd("p2a")),
        )
        .await,
    ));
    backend
        .update_run_status(&r1, "completed", "done")
        .await
        .expect("release pipeline");
    out.push((
        "pipeline after-terminal",
        claim(
            backend,
            &run_row(&Uuid::new_v4().to_string(), &user.id, Some(&p1), &wd("p1c")),
        )
        .await,
    ));

    // -- concurrent double-claim: exactly one winner, no error --------------
    let cd = wd("concurrent");
    let c1 = run_row(&Uuid::new_v4().to_string(), &user.id, None, &cd);
    let c2 = run_row(&Uuid::new_v4().to_string(), &user.id, None, &cd);
    let (x, y) = tokio::join!(claim_run_row(backend, &c1), claim_run_row(backend, &c2));
    let winners = [
        x.expect("no error on a lost race"),
        y.expect("no error on a lost race"),
    ]
    .iter()
    .filter(|admitted| **admitted)
    .count();
    out.push(("concurrent exactly-one", winners == 1));

    // -- rejection is a verdict (Ok(false)), never a storage error ----------
    let r = claim_run_row(
        backend,
        &run_row(&Uuid::new_v4().to_string(), &user.id, None, &cd),
    )
    .await;
    out.push(("rejection is Ok(false) not Err", matches!(r, Ok(false))));

    out
}

/// SQLite arm: unconditional. A file-backed pool (max 5 connections) so the
/// concurrent claims genuinely land on separate connections instead of the
/// single-connection in-memory fixture.
#[tokio::test]
async fn admission_verdicts_match_contract_on_sqlite() {
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("sqlite:{}?mode=rwc", dir.path().join("parity.db").display());
    let backend = SqliteBackend::new(&url)
        .await
        .expect("file-backed sqlite backend");
    backend.init().await.expect("schema init");

    let got = collect_verdicts(&backend).await;
    assert_eq!(
        got,
        EXPECTED.to_vec(),
        "SQLite admission verdicts diverged from the shared contract"
    );
}

/// PostgreSQL arm: same scenarios, same shared contract. Gated on
/// `OXO_PG_TEST_URL`; skips cleanly when unset (local `make ci` has no
/// PostgreSQL). Run with:
///
/// ```text
/// OXO_PG_TEST_URL=postgres://postgres:postgres@127.0.0.1:5432/oxo_test \
///   cargo test -p oxo-flow-web --features postgres --test admission_backend_parity
/// ```
#[cfg(feature = "postgres")]
#[tokio::test]
async fn admission_verdicts_match_contract_on_postgres() {
    let url = match std::env::var("OXO_PG_TEST_URL") {
        Ok(u) if !u.trim().is_empty() => u.trim().to_string(),
        _ => {
            eprintln!("skipping PostgreSQL admission-parity arm: OXO_PG_TEST_URL is not set");
            return;
        }
    };
    let backend = PostgresBackend::new(&url)
        .await
        .expect("connect to live PostgreSQL");
    backend.init().await.expect("schema bootstrap");

    let got = collect_verdicts(&backend).await;
    assert_eq!(
        got,
        EXPECTED.to_vec(),
        "PostgreSQL admission verdicts diverged from the shared contract"
    );
}
