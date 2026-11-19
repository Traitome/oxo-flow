#![cfg(feature = "postgres")]

//! Concurrency, crash-recovery, and horizontal-instance measurements for the
//! unified run-ledger storage abstraction on a LIVE PostgreSQL (issue #1251).
//!
//! The existing `infra::db::postgres` smoke tests and
//! `postgres_storage_contract.rs` pin schema bootstrap and CRUD wiring, and
//! its guard tests only exercise a 2-way race. This file is the measurement
//! named by the ROADMAP 12.1 exit criterion: N-way contention on the guarded
//! run insert, a writer killed mid-transaction, and two independent
//! `PostgresBackend` instances sharing one database.
//!
//! Gating follows the contract test: with `OXO_FLOW_TEST_POSTGRES_URL` unset
//! every test skips cleanly (stderr note, no failure). Point it at a
//! *disposable* database:
//!
//! ```text
//! OXO_FLOW_TEST_POSTGRES_URL=postgres://postgres:postgres@127.0.0.1:5432/oxo_test \
//!   cargo test -p oxo-flow-web --features postgres --test postgres_concurrency
//! ```
//!
//! `OXO_FLOW_TEST_POSTGRES_CONCURRENCY` overrides the N-way fan-out
//! (default 16, clamped to 2..=256). These tests print one
//! `measurement=…` line per run so `scripts/postgres-ledger-campaign.sh` can
//! collect raw numbers.

use oxo_flow_web::infra::db::models;
use oxo_flow_web::infra::db::postgres::PostgresBackend;
use oxo_flow_web::infra::db::{Pagination, StorageBackend};
use sqlx::{Connection, PgConnection, Row};
use std::collections::BTreeSet;
use std::sync::Arc;
use uuid::Uuid;

fn env_url() -> Option<String> {
    match std::env::var("OXO_FLOW_TEST_POSTGRES_URL") {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => None,
    }
}

fn n_way() -> usize {
    std::env::var("OXO_FLOW_TEST_POSTGRES_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| (2..=256).contains(n))
        .unwrap_or(16)
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

async fn connect(url: &str) -> PostgresBackend {
    let backend = PostgresBackend::new(url)
        .await
        .expect("connect to disposable PostgreSQL database");
    backend.init().await.expect("initialize PostgreSQL schema");
    backend
}

async fn new_user(backend: &PostgresBackend, tag: &str) -> String {
    let name = format!("pg_{tag}_{}", Uuid::new_v4().simple());
    backend
        .create_user(&name, "admin", Some("hash"))
        .await
        .expect("create user")
        .id
}

fn sample_run(user_id: &str, workdir: &str) -> models::RunRow {
    models::RunRow {
        id: Uuid::new_v4().to_string(),
        user_id: user_id.to_string(),
        pipeline_id: None,
        pipeline_snapshot: "[workflow]\nname = \"conc\"\n".to_string(),
        workflow_name: Some("conc".to_string()),
        status: "queued".to_string(),
        phase: "parsing".to_string(),
        dry_run: false,
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

/// Count how many of the contested ids actually exist, reading them back
/// through the public API with a fresh query (not the insert's return value)
/// so a ghost row that the guard *wrote* but reported as rejected is caught.
async fn rows_present(backend: &PostgresBackend, ids: &[String]) -> usize {
    let mut present = 0;
    for id in ids {
        if backend.get_run(id).await.expect("get_run").is_some() {
            present += 1;
        }
    }
    present
}

/// Measurement 1 — concurrency. N guarded inserts on the SAME workdir key:
/// exactly one wins, the rest are rejected (never erroring), and exactly one
/// row exists on disk (no ghost/orphan). The pool is used again afterwards to
/// prove no connection was left wedged by a deadlock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_guarded_insert_admits_exactly_one() {
    let Some(url) = env_url() else {
        eprintln!("skipping PostgreSQL concurrency test: OXO_FLOW_TEST_POSTGRES_URL is unset");
        return;
    };
    let backend = Arc::new(connect(&url).await);
    let user_id = new_user(&backend, "conc").await;
    let n = n_way();
    let workdir = format!("/tmp/oxo-pg-conc-{}", Uuid::new_v4().simple());

    let runs: Vec<models::RunRow> = (0..n).map(|_| sample_run(&user_id, &workdir)).collect();
    let ids: Vec<String> = runs.iter().map(|r| r.id.clone()).collect();

    let mut handles = Vec::with_capacity(n);
    for run in runs {
        let backend = Arc::clone(&backend);
        handles.push(tokio::spawn(async move {
            backend.create_run_if_workdir_idle(&run).await
        }));
    }

    let mut admitted = 0usize;
    let mut errors = 0usize;
    for handle in handles {
        match handle.await.expect("join worker") {
            Ok(true) => admitted += 1,
            Ok(false) => {}
            Err(e) => {
                errors += 1;
                eprintln!("admission error under contention: {e}");
            }
        }
    }

    assert_eq!(
        errors, 0,
        "no insert may error under contention (a leaked unique violation is an unhandled race)"
    );
    assert_eq!(
        admitted, 1,
        "exactly one of {n} concurrent guarded inserts must win the workdir guard"
    );
    let live = rows_present(&backend, &ids).await;
    assert_eq!(
        live, 1,
        "exactly one row must exist after the race (no ghost/orphan rows)"
    );
    // Deadlock probe: the pool must still hand out connections afterwards.
    assert!(
        backend
            .list_runs(
                &user_id,
                Pagination {
                    page: 1,
                    per_page: 10
                }
            )
            .await
            .is_ok(),
        "the pool must remain usable after the race (no deadlock/hang)"
    );

    eprintln!(
        "measurement=concurrency n={n} admitted={admitted} live_rows={live} errors={errors} result=PASS"
    );
}

/// Non-vacuity control for measurement 1. With N DISTINCT guard keys the same
/// apparatus must observe N winners; if it could only ever print `1`, the
/// "exactly one" result above would be a constant, not a measurement. This
/// also pins the guard's *key* semantics: looseness here (a shared constant
/// key, say) would show up as `admitted < n`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn negative_control_distinct_keys_admit_all() {
    let Some(url) = env_url() else {
        eprintln!("skipping PostgreSQL negative-control test: OXO_FLOW_TEST_POSTGRES_URL is unset");
        return;
    };
    let backend = Arc::new(connect(&url).await);
    let user_id = new_user(&backend, "ctrl").await;
    let n = n_way();

    let runs: Vec<models::RunRow> = (0..n)
        .map(|i| {
            let workdir = format!("/tmp/oxo-pg-ctrl-{}-{i}", Uuid::new_v4().simple());
            sample_run(&user_id, &workdir)
        })
        .collect();
    let ids: Vec<String> = runs.iter().map(|r| r.id.clone()).collect();

    let mut handles = Vec::with_capacity(n);
    for run in runs {
        let backend = Arc::clone(&backend);
        handles.push(tokio::spawn(async move {
            backend.create_run_if_workdir_idle(&run).await
        }));
    }
    let mut admitted = 0usize;
    for handle in handles {
        if handle.await.expect("join worker").expect("admission") {
            admitted += 1;
        }
    }

    assert_eq!(
        admitted, n,
        "distinct guard keys must all be admitted — the apparatus can observe >1 winner"
    );
    assert_eq!(rows_present(&backend, &ids).await, n);

    eprintln!("measurement=negative_control n={n} admitted={admitted} result=PASS");
}

/// Measurement 2 — crash recovery. A writer opens a transaction, inserts a
/// queued run, and is then killed mid-transaction (`pg_terminate_backend`,
/// the in-process analog of a SIGKILL / dropped connection). The uncommitted
/// row must NOT survive, and a fresh independent connection must read
/// consistent state. Durable counterpart: a committed row IS visible to a
/// fresh connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_mid_transaction_leaves_no_ghost_row() {
    let Some(url) = env_url() else {
        eprintln!("skipping PostgreSQL crash-recovery test: OXO_FLOW_TEST_POSTGRES_URL is unset");
        return;
    };
    let backend = connect(&url).await;
    let user_id = new_user(&backend, "crash").await;

    let run_id = Uuid::new_v4().to_string();
    let workdir = format!("/tmp/oxo-pg-crash-{}", Uuid::new_v4().simple());
    let ts = now();

    // Writer connection: BEGIN + INSERT, flushed, deliberately NOT committed.
    // The INSERT is written out literally on purpose — this probe must derive
    // the row shape from the schema, not inherit it from the production
    // builders it is meant to check.
    let mut writer = PgConnection::connect(&url).await.expect("writer connect");
    let writer_pid: i32 = sqlx::query("SELECT pg_backend_pid()")
        .fetch_one(&mut writer)
        .await
        .expect("read writer backend pid")
        .get(0);
    let mut tx = writer.begin().await.expect("begin transaction");
    sqlx::query(
        "INSERT INTO runs \
         (id, user_id, pipeline_id, pipeline_snapshot, workflow_name, status, phase, \
          dry_run, pid, workdir, config_overrides, artifacts_incomplete, \
          started_at, finished_at, created_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)",
    )
    .bind(&run_id)
    .bind(&user_id)
    .bind(None::<String>)
    .bind("[workflow]\nname = \"crash\"\n")
    .bind("crash")
    .bind("queued")
    .bind("parsing")
    .bind(false)
    .bind(None::<i64>)
    .bind(&workdir)
    .bind(None::<String>)
    .bind(None::<String>)
    .bind(None::<String>)
    .bind(None::<String>)
    .bind(&ts)
    .execute(&mut *tx)
    .await
    .expect("in-transaction insert");

    // Kill the writer's backend from a separate control connection: the
    // transaction never commits, and the server aborts it.
    let mut control = PgConnection::connect(&url).await.expect("control connect");
    let killed: bool = sqlx::query("SELECT pg_terminate_backend($1)")
        .bind(writer_pid)
        .fetch_one(&mut control)
        .await
        .expect("terminate writer backend")
        .get(0);
    assert!(killed, "control connection must kill the writer's backend");
    drop(tx);
    let _ = writer.close().await; // socket is already dead; ignore the error

    // Fresh read: an uncommitted insert must not survive its writer's death.
    let fresh = connect(&url).await;
    assert!(
        fresh.get_run(&run_id).await.expect("fresh read").is_none(),
        "an uncommitted insert must not survive a mid-transaction crash (ghost row)"
    );

    // Durability counterpart: a committed admission IS visible to a new
    // independent connection.
    let committed = sample_run(
        &user_id,
        &format!("/tmp/oxo-pg-durable-{}", Uuid::new_v4().simple()),
    );
    assert!(
        fresh
            .create_run_if_workdir_idle(&committed)
            .await
            .expect("commit admission"),
        "the committed row must be admitted"
    );
    let fresh2 = connect(&url).await;
    assert!(
        fresh2
            .get_run(&committed.id)
            .await
            .expect("read committed")
            .is_some(),
        "a committed row must be visible to a fresh connection"
    );

    eprintln!(
        "measurement=crash_recovery killed_backend={killed} ghost_rows=0 committed_visible=1 result=PASS"
    );
}

/// Measurement 3 — horizontal instances. Two independent `PostgresBackend`
/// values (separate pools, same database) must not double-claim the guarded
/// run key, and must agree on pagination.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_instances_share_one_guard_and_agree_on_pagination() {
    let Some(url) = env_url() else {
        eprintln!(
            "skipping PostgreSQL horizontal-instance test: OXO_FLOW_TEST_POSTGRES_URL is unset"
        );
        return;
    };
    let instance_a = Arc::new(connect(&url).await);
    let instance_b = Arc::new(connect(&url).await);
    let user_id = new_user(&instance_a, "horiz").await;
    let n = n_way();
    let workdir = format!("/tmp/oxo-pg-horiz-{}", Uuid::new_v4().simple());

    // ── workdir guard, split across the two instances ──────────────────
    let runs: Vec<models::RunRow> = (0..n).map(|_| sample_run(&user_id, &workdir)).collect();
    let ids: Vec<String> = runs.iter().map(|r| r.id.clone()).collect();

    let mut handles = Vec::with_capacity(n);
    for (i, run) in runs.into_iter().enumerate() {
        let backend = if i % 2 == 0 {
            Arc::clone(&instance_a)
        } else {
            Arc::clone(&instance_b)
        };
        handles.push(tokio::spawn(async move {
            backend.create_run_if_workdir_idle(&run).await
        }));
    }
    let mut admitted = 0usize;
    for handle in handles {
        if handle.await.expect("join worker").expect("admission") {
            admitted += 1;
        }
    }
    assert_eq!(
        admitted, 1,
        "two instances must not both claim the same workdir key"
    );
    assert_eq!(rows_present(&instance_b, &ids).await, 1);

    // ── pipeline guard, one claim per instance ─────────────────────────
    let pipeline = models::PipelineRow {
        id: Uuid::new_v4().to_string(),
        user_id: user_id.clone(),
        name: "pg-horiz".to_string(),
        version: "1.0.0".to_string(),
        toml_content: "[workflow]\nname = \"horiz\"\n".to_string(),
        rules_count: 1,
        forked_from: None,
        visibility: "private".to_string(),
        created_at: now(),
        updated_at: now(),
        revision: 1,
    };
    let pipeline = instance_a
        .save_pipeline(&pipeline)
        .await
        .expect("save pipeline");
    let mut guarded = sample_run(
        &user_id,
        &format!("/tmp/oxo-pg-horiz-pipe-{}", Uuid::new_v4().simple()),
    );
    guarded.pipeline_id = Some(pipeline.id.clone());
    let mut challenger = guarded.clone();
    challenger.id = Uuid::new_v4().to_string();
    challenger.workdir = Some(format!(
        "/tmp/oxo-pg-horiz-pipe-b-{}",
        Uuid::new_v4().simple()
    ));

    let (a_claim, b_claim) = tokio::join!(
        instance_a.create_run_if_pipeline_idle(&guarded),
        instance_b.create_run_if_pipeline_idle(&challenger)
    );
    let a_claim = a_claim.expect("instance a claim");
    let b_claim = b_claim.expect("instance b claim");
    let claims = usize::from(a_claim) + usize::from(b_claim);
    assert_eq!(
        claims, 1,
        "the same pipeline must be claimed by exactly one instance"
    );
    let a_has = instance_a
        .get_run(&guarded.id)
        .await
        .expect("read guarded")
        .is_some();
    let loser = if a_has { &challenger.id } else { &guarded.id };
    assert!(
        instance_b
            .get_run(loser)
            .await
            .expect("read loser")
            .is_none(),
        "the losing instance must not have written a row"
    );

    // ── pagination consistency across instances ────────────────────────
    let mut seeded = BTreeSet::new();
    for i in 0..7 {
        let run = sample_run(
            &user_id,
            &format!("/tmp/oxo-pg-page-{i}-{}", Uuid::new_v4().simple()),
        );
        seeded.insert(run.id.clone());
        assert!(
            instance_a
                .create_run_if_workdir_idle(&run)
                .await
                .expect("seed run")
        );
    }
    let page_a = instance_a
        .list_runs(
            &user_id,
            Pagination {
                page: 1,
                per_page: 100,
            },
        )
        .await
        .expect("list on instance a");
    let page_b = instance_b
        .list_runs(
            &user_id,
            Pagination {
                page: 1,
                per_page: 100,
            },
        )
        .await
        .expect("list on instance b");
    assert_eq!(
        page_a.total_items, page_b.total_items,
        "instances must agree on the run total"
    );
    let ids_a: BTreeSet<String> = page_a.items.iter().map(|r| r.id.clone()).collect();
    let ids_b: BTreeSet<String> = page_b.items.iter().map(|r| r.id.clone()).collect();
    assert_eq!(ids_a, ids_b, "instances must return the same page contents");
    assert!(
        seeded.is_subset(&ids_a),
        "every seeded run must appear on the first page from either instance"
    );

    eprintln!(
        "measurement=horizontal_instances n={n} workdir_admitted={admitted} pipeline_claims={claims} page_total={} instances={} result=PASS",
        page_a.total_items, 2
    );
}
