//! HTTP handlers for execution domain.
//!
//! Thin adapters: parse HTTP request → call service → serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

use crate::domains::ai::agents::monitor_agent::{self, NodeExecutionStatus, ResourceUsage};
use crate::domains::ai::agents::report_agent;
use crate::domains::ai::agents::types::ReportFile;
use crate::domains::auth::current_user::{self, CurrentUser};
use axum::{Extension, Json, extract::Path, http::StatusCode};

use super::checkpoint_status;
use super::service;
use super::types::*;
use crate::domains::workflow::handlers::ApiError;
use crate::infra::db::models;

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

fn err(s: StatusCode, c: &str, m: String) -> (StatusCode, Json<ApiError>) {
    (
        s,
        Json(ApiError {
            code: c.into(),
            message: m,
            detail: None,
            suggestion: None,
        }),
    )
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Fetch a run row and enforce ownership (issue #82 P0-4): admins may
/// address any run, everyone else only their own. Foreign and unknown runs
/// both 404 — the resource set itself is private, so existence must not
/// leak through a 403.
pub(crate) async fn load_owned_run(
    pool: &sqlx::SqlitePool,
    user: &CurrentUser,
    id: &str,
) -> Result<models::RunRow, (StatusCode, Json<ApiError>)> {
    let run: Option<models::RunRow> = sqlx::query_as("SELECT * FROM runs WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| {
            tracing::error!("DB error fetching run {id}: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?;

    let run = run.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Run {id} not found"),
        )
    })?;

    if !user.is_admin() && run.user_id != user.id {
        return Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Run {id} not found"),
        ));
    }
    Ok(run)
}

/// POST /api/runs
pub async fn create_run(
    authenticated: Option<Extension<CurrentUser>>,
    Json(req): Json<serde_json::Value>,
) -> ApiResult<CreateRunResponse> {
    let user = current_user::resolve(authenticated.as_ref());
    let toml = req
        .get("toml_content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "MISSING",
                "toml_content required".into(),
            )
        })?;
    let max_jobs = req.get("max_jobs").and_then(|v| v.as_u64()).unwrap_or(4) as usize;
    let config = RunConfig {
        max_jobs: Some(max_jobs),
        dry_run: req.get("dry_run").and_then(|v| v.as_bool()),
        keep_going: req.get("keep_going").and_then(|v| v.as_bool()),
        resource_budget: None,
    };

    // Optional saved-pipeline linkage (issue #69): runs targeting a saved
    // pipeline execute in the pipeline's persistent workdir, so the CLI's
    // checkpoint (config snapshot, rule fingerprints, input manifests)
    // survives across re-runs and delivers precise invalidation.
    let pipeline_id: Option<String> = req
        .get("pipeline_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    // Boundary validation: pipeline_id becomes a path component and must be a
    // well-formed UUID (the save_pipeline id format).
    if let Some(pid) = pipeline_id.as_ref()
        && uuid::Uuid::parse_str(pid).is_err()
    {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "INVALID_PIPELINE_ID",
            format!("pipeline_id must be a UUID, got: {pid}"),
        ));
    }

    let resp = service::create_run(toml, &config, None)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "RUN_ERROR", e))?;

    // Persist run to database. Ad-hoc runs (no pipeline_id) keep a NULL
    // pipeline_id (the column is an FK only when a pipeline exists).
    if let Ok(pool) = crate::infra::db::sqlite::try_pool() {
        let workflow_name = oxo_flow_core::config::WorkflowConfig::parse(toml)
            .map(|c| c.workflow.name)
            .unwrap_or_else(|_| "unnamed".to_string());

        // Resolve the working directory the executor will run in.
        let run_dir = match &pipeline_id {
            Some(pid) => {
                let exists: Option<String> =
                    sqlx::query_scalar("SELECT id FROM pipelines WHERE id = ?")
                        .bind(pid)
                        .fetch_optional(pool)
                        .await
                        .map_err(|e| {
                            tracing::error!("DB error checking pipeline {pid}: {e}");
                            err(
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "DB_ERROR",
                                "Internal database error".into(),
                            )
                        })?;
                if exists.is_none() {
                    return Err(err(
                        StatusCode::NOT_FOUND,
                        "PIPELINE_NOT_FOUND",
                        format!("Pipeline {pid} not found"),
                    ));
                }
                crate::workspace::setup_pipeline_directory(&user.id, pid)
                    .map_err(|e| err(StatusCode::BAD_REQUEST, "RUN_ERROR", e.to_string()))?
            }
            None => crate::workspace::setup_run_directory(&user.id, &resp.run_id)
                .map_err(|e| err(StatusCode::BAD_REQUEST, "RUN_ERROR", e.to_string()))?,
        };

        let run = models::RunRow {
            id: resp.run_id.clone(),
            user_id: user.id.clone(),
            pipeline_id: pipeline_id.clone(),
            pipeline_snapshot: toml.to_string(),
            status: "queued".to_string(),
            phase: "parsing".to_string(),
            pid: None,
            workdir: Some(run_dir.to_string_lossy().to_string()),
            started_at: None,
            finished_at: None,
            created_at: now_iso(),
            workflow_name: Some(workflow_name),
        };
        if let Err(e) = sqlx::query(
            "INSERT OR IGNORE INTO runs (id, user_id, pipeline_id, pipeline_snapshot, status, phase, pid, workdir, started_at, finished_at, created_at, workflow_name) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&run.id)
        .bind(&run.user_id)
        .bind(&run.pipeline_id)
        .bind(&run.pipeline_snapshot)
        .bind(&run.status)
        .bind(&run.phase)
        .bind(run.pid)
        .bind(&run.workdir)
        .bind(&run.started_at)
        .bind(&run.finished_at)
        .bind(&run.created_at)
        .bind(&run.workflow_name)
        .execute(pool)
        .await
        {
            tracing::error!("Failed to persist run {} to database: {e}", run.id);
        }

        // Save pipeline TOML to the working directory so the executor (CLI
        // subprocess) can read it.
        let workflow_path = run_dir.join("workflow.oxoflow");
        if let Err(e) = std::fs::write(&workflow_path, toml) {
            tracing::error!("Failed to write workflow to {:?}: {e}", workflow_path);
        } else {
            tracing::info!("Saved workflow to {:?}", workflow_path);
        }

        crate::executor::spawn_background_run(
            resp.run_id.clone(),
            user.id.clone(),
            "none".to_string(),
            "local".to_string(),
            Some(run_dir),
            crate::executor::RunFlags {
                resume_failed: false,
                rerun: false,
                dry_run: req
                    .get("dry_run")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                keep_going: req
                    .get("keep_going")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                max_jobs: req
                    .get("max_jobs")
                    .and_then(|v| v.as_u64())
                    .map(|j| j as usize),
                samples: req
                    .get("samples")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str())
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default(),
                targets: req
                    .get("targets")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str())
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default(),
            },
        );
    }

    Ok(Json(resp))
}

/// GET /api/runs
pub async fn list_runs(
    authenticated: Option<Extension<CurrentUser>>,
) -> ApiResult<Vec<serde_json::Value>> {
    let user = current_user::resolve(authenticated.as_ref());
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    // Ownership scoping (issue #82 P0-4): non-admins see their own runs
    // only; admins see the whole fleet.
    let rows: Vec<models::RunRow> = if user.is_admin() {
        sqlx::query_as("SELECT * FROM runs ORDER BY created_at DESC LIMIT 100")
            .fetch_all(pool)
            .await
    } else {
        sqlx::query_as("SELECT * FROM runs WHERE user_id = ? ORDER BY created_at DESC LIMIT 100")
            .bind(&user.id)
            .fetch_all(pool)
            .await
    }
    .map_err(|e| {
        tracing::error!("DB error listing runs: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    let list: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "user_id": r.user_id,
                "pipeline_id": r.pipeline_id,
                "workflow_name": r.workflow_name,
                "status": r.status,
                "phase": r.phase,
                "pid": r.pid,
                "workdir": r.workdir,
                "started_at": r.started_at,
                "finished_at": r.finished_at,
                "created_at": r.created_at,
            })
        })
        .collect();

    Ok(Json(list))
}

/// GET /api/runs/{id}
pub async fn get_run(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;
    let user = current_user::resolve(authenticated.as_ref());
    let run = load_owned_run(pool, &user, &id).await?;

    Ok(Json(serde_json::json!({
        "id": run.id,
        "user_id": run.user_id,
        "pipeline_id": run.pipeline_id,
        "pipeline_snapshot": run.pipeline_snapshot,
        "status": run.status,
        "phase": run.phase,
        "pid": run.pid,
        "workdir": run.workdir,
        "started_at": run.started_at,
        "finished_at": run.finished_at,
        "created_at": run.created_at,
    })))
}

/// GET /api/runs/{id}/status
pub async fn get_run_status(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<RunStatusResponse> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let user = current_user::resolve(authenticated.as_ref());
    let run = load_owned_run(pool, &user, &id).await?;

    // Node status comes from the engine's checkpoint state (single source of
    // truth), merged with the full rule list so unrun rules show as pending.
    let dag = oxo_flow_core::WorkflowConfig::parse(&run.pipeline_snapshot)
        .ok()
        .and_then(|wf| oxo_flow_core::dag::WorkflowDag::from_rules(&wf.rules).ok());
    let node_items = checkpoint_status::load_node_statuses(
        std::path::Path::new(run.workdir.as_deref().unwrap_or("")),
        run.status == "running",
    );
    let node_items = match &dag {
        Some(d) => {
            checkpoint_status::with_all_rules(node_items, &d.execution_order().unwrap_or_default())
        }
        None => node_items,
    };

    let overall = service::compute_overall_status(&node_items, Some(&run.status));

    // Real telemetry timeline (issue #82 P1-2): the executor's per-run
    // sampler appends to workdir/metrics.jsonl; an absent file yields an
    // empty timeline (never fabricated numbers).
    let workdir = std::path::Path::new(run.workdir.as_deref().unwrap_or(""));
    let metrics = checkpoint_status::load_metrics(workdir);
    let last = metrics.last();
    let resources = ResourceSnapshot {
        cpu_pct: last.and_then(|m| m["cpu_pct"].as_f64()).unwrap_or(0.0),
        memory_mb: last.and_then(|m| m["memory_mb"].as_f64()).unwrap_or(0.0) as u64,
        disk_mb: 0,
    };
    let timeline: Vec<TimelineEvent> = metrics
        .iter()
        .map(|m| TimelineEvent {
            timestamp: m["ts"].as_str().unwrap_or("").to_string(),
            event: "metrics".to_string(),
            node: None,
            message: Some(format!(
                "{:.0} MB RAM · {:.0}% CPU",
                m["memory_mb"].as_f64().unwrap_or(0.0),
                m["cpu_pct"].as_f64().unwrap_or(0.0)
            )),
        })
        .collect();

    Ok(Json(RunStatusResponse {
        status: overall,
        phase: run.phase,
        nodes: node_items,
        timeline,
        resources,
    }))
}

/// GET /api/runs/{id}/dag-status
pub async fn get_dag_status(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<DagStatusResponse> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let user = current_user::resolve(authenticated.as_ref());
    let run = load_owned_run(pool, &user, &id).await?;

    // Parse the pipeline snapshot to build DAG
    let dag = oxo_flow_core::WorkflowConfig::parse(&run.pipeline_snapshot)
        .ok()
        .and_then(|wf| oxo_flow_core::dag::WorkflowDag::from_rules(&wf.rules).ok());

    // Node status comes from the engine's checkpoint state, merged with the
    // full rule list so unrun rules show as pending.
    let node_items = checkpoint_status::load_node_statuses(
        std::path::Path::new(run.workdir.as_deref().unwrap_or("")),
        run.status == "running",
    );
    let node_items = match &dag {
        Some(d) => {
            checkpoint_status::with_all_rules(node_items, &d.execution_order().unwrap_or_default())
        }
        None => node_items,
    };
    let dag_nodes: Vec<DagNode> = node_items
        .iter()
        .map(|n| {
            let color = match n.status {
                NodeStatus::Success => "green",
                NodeStatus::Running => "blue",
                NodeStatus::Failed => "red",
                NodeStatus::Skipped => "gray",
                NodeStatus::Pending => "lightgray",
            };
            DagNode {
                id: n.rule.clone(),
                label: n.rule.clone(),
                status: n.status.to_string(),
                color: color.to_string(),
                duration_ms: n.duration_ms,
                exit_code: n.exit_code,
            }
        })
        .collect();

    let edges: Vec<DagEdge> = dag
        .as_ref()
        .map(|d| {
            let mut edge_list = Vec::new();
            for node_name in d.execution_order().unwrap_or_default() {
                if let Ok(deps) = d.dependencies(&node_name) {
                    for dep in deps {
                        edge_list.push(DagEdge {
                            source: dep,
                            target: node_name.clone(),
                        });
                    }
                }
            }
            edge_list
        })
        .unwrap_or_default();

    // Compute ETA based on completed node durations
    let completed_duration: u64 = dag_nodes.iter().filter_map(|n| n.duration_ms).sum();
    let completed_count = dag_nodes
        .iter()
        .filter(|n| n.status == "success" || n.status == "failed" || n.status == "skipped")
        .count();
    let total_count = dag_nodes.len();
    let eta_ms = if completed_count > 0 {
        let avg_per_node = completed_duration / completed_count as u64;
        let remaining = total_count.saturating_sub(completed_count) as u64;
        Some(avg_per_node * remaining)
    } else {
        None
    };

    Ok(Json(DagStatusResponse {
        nodes: dag_nodes.clone(),
        edges,
        parallel_groups: dag
            .as_ref()
            .and_then(|d| d.parallel_groups().ok())
            .unwrap_or_default(),
        critical_path: dag
            .as_ref()
            .and_then(|d| d.critical_path().ok())
            .unwrap_or_default(),
        metrics: DagMetrics {
            total_nodes: total_count,
            completed_nodes: completed_count,
            failed_nodes: dag_nodes.iter().filter(|n| n.status == "failed").count(),
            running_nodes: dag_nodes.iter().filter(|n| n.status == "running").count(),
            pending_nodes: dag_nodes.iter().filter(|n| n.status == "pending").count(),
            eta_ms,
        },
    }))
}

/// GET /api/runs/{id}/instances — the sample×rule instance table
/// (issue #82 P1-1): which concrete sample under which rule failed, ran,
/// or is still pending. The checkpoint stores EXPANDED instance names
/// (`qc_S1`); the base rule is recovered by longest-prefix matching
/// against the DAG, the same attribution `with_all_rules` uses.
pub async fn get_run_instances(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<Vec<serde_json::Value>> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;
    let user = current_user::resolve(authenticated.as_ref());
    let run = load_owned_run(pool, &user, &id).await?;

    let workdir = run.workdir.as_deref().unwrap_or("");
    let items = checkpoint_status::load_node_statuses(
        std::path::Path::new(workdir),
        run.status == "running",
    );
    let dag = oxo_flow_core::WorkflowConfig::parse(&run.pipeline_snapshot)
        .ok()
        .and_then(|wf| oxo_flow_core::dag::WorkflowDag::from_rules(&wf.rules).ok());
    let rules: Vec<String> = dag
        .as_ref()
        .and_then(|d| d.execution_order().ok())
        .unwrap_or_default();

    let instances: Vec<serde_json::Value> = items
        .iter()
        .map(|n| {
            let base = rules
                .iter()
                .filter(|r| n.rule == **r || n.rule.starts_with(&format!("{r}_")))
                .max_by_key(|r| r.len())
                .cloned()
                .unwrap_or_else(|| n.rule.clone());
            // Instance names embed the sample group:
            // `qc_auto-discovered_S1` → base `qc`, sample `S1`. The sample
            // is the LAST underscore-separated segment of the remainder
            // (group names may contain hyphens but samples are the trailing
            // identifier; anything before it is the group).
            let remainder = n.rule.strip_prefix(&format!("{base}_"));
            let (group, sample) = match remainder {
                Some(rest) => match rest.rsplit_once('_') {
                    Some((group_part, sample_part)) => {
                        (Some(group_part.to_string()), Some(sample_part.to_string()))
                    }
                    None => (None, Some(rest.to_string())),
                },
                None => (None, None),
            };
            serde_json::json!({
                "instance": n.rule,
                "rule": base,
                "sample": sample,
                "group": group,
                "status": n.status.to_string(),
                "duration_ms": n.duration_ms,
                "exit_code": n.exit_code,
            })
        })
        .collect();

    Ok(Json(instances))
}

/// GET /api/runs/{id}/diagnostics
pub async fn get_diagnostics(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<DiagnosticsResponse> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let user = current_user::resolve(authenticated.as_ref());
    let run = load_owned_run(pool, &user, &id).await?;

    let node_items = checkpoint_status::load_node_statuses(
        std::path::Path::new(run.workdir.as_deref().unwrap_or("")),
        run.status == "running",
    );

    // Try to read log output from workdir
    let log_output = run
        .workdir
        .as_ref()
        .map(|wd| std::fs::read_to_string(format!("{wd}/execution.log")).unwrap_or_default())
        .unwrap_or_default();

    let benchmarks = checkpoint_status::load_benchmarks(std::path::Path::new(
        run.workdir.as_deref().unwrap_or(""),
    ));

    let diagnostics = service::diagnose_run(&node_items, &log_output, &benchmarks);
    Ok(Json(diagnostics))
}

/// GET /api/runs/{id}/logs
pub async fn get_run_logs(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<String> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let user = current_user::resolve(authenticated.as_ref());
    let run = load_owned_run(pool, &user, &id).await?;

    let log_content = run
        .workdir
        .as_ref()
        .and_then(|wd| std::fs::read_to_string(format!("{wd}/execution.log")).ok())
        .unwrap_or_else(|| "No execution log available.".to_string());

    Ok(Json(log_content))
}

/// GET /api/runs/{id}/results
pub async fn get_run_results(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<Vec<serde_json::Value>> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let user = current_user::resolve(authenticated.as_ref());
    let run = load_owned_run(pool, &user, &id).await?;

    // List files in the workdir recursively so nested products (e.g.
    // results/sample1/peaks.bed) are visible (issue #79 P1-08).
    let results: Vec<serde_json::Value> = run
        .workdir
        .as_ref()
        .map(|wd| {
            service::list_files_recursive(std::path::Path::new(wd))
                .into_iter()
                .map(|f| {
                    serde_json::json!({
                        "name": f.name,
                        "path": f.path,
                        "size_bytes": f.size_bytes,
                        "is_dir": f.is_dir,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Json(results))
}

/// POST /api/runs/{id}/retry
pub async fn retry_run(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> ApiResult<RetryResponse> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let user = current_user::resolve(authenticated.as_ref());
    let run = load_owned_run(pool, &user, &id).await?;

    let from_rule = req.get("from_rule").and_then(|v| v.as_str());
    let skip_succeeded = req
        .get("skip_succeeded")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let node_items = checkpoint_status::load_node_statuses(
        std::path::Path::new(run.workdir.as_deref().unwrap_or("")),
        run.status == "running",
    );

    let dag = oxo_flow_core::WorkflowConfig::parse(&run.pipeline_snapshot)
        .ok()
        .and_then(|wf| oxo_flow_core::dag::WorkflowDag::from_rules(&wf.rules).ok())
        .ok_or_else(|| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DAG_ERROR",
                "Failed to reconstruct DAG from pipeline snapshot".into(),
            )
        })?;

    let plan = service::compute_retry_plan(&node_items, &dag, from_rule, skip_succeeded)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "RETRY_ERROR", e))?;

    // The retry is a REAL run (issue #82 P0-3 — previously the plan was
    // returned with a new_run_id that never existed). Re-executing in the
    // SAME workdir lets the CLI's checkpoint do the work: failed rules have
    // no success record and re-run, and the engine's cascade invalidation
    // marks their downstream dependents dirty too — exactly the computed
    // plan, verified against the same single source of truth.
    let new_run_id = plan.new_run_id.clone();
    let now = now_iso();
    let new_run = models::RunRow {
        id: new_run_id.clone(),
        user_id: user.id.clone(),
        pipeline_id: run.pipeline_id.clone(),
        pipeline_snapshot: run.pipeline_snapshot.clone(),
        workflow_name: run.workflow_name.clone(),
        status: "queued".to_string(),
        phase: "parsing".to_string(),
        pid: None,
        workdir: run.workdir.clone(),
        started_at: None,
        finished_at: None,
        created_at: now.clone(),
    };
    sqlx::query(
        "INSERT OR IGNORE INTO runs (id, user_id, pipeline_id, pipeline_snapshot, workflow_name, status, phase, pid, workdir, started_at, finished_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&new_run.id)
    .bind(&new_run.user_id)
    .bind(&new_run.pipeline_id)
    .bind(&new_run.pipeline_snapshot)
    .bind(&new_run.workflow_name)
    .bind(&new_run.status)
    .bind(&new_run.phase)
    .bind(new_run.pid)
    .bind(&new_run.workdir)
    .bind(&new_run.started_at)
    .bind(&new_run.finished_at)
    .bind(&new_run.created_at)
    .execute(pool)
    .await
    .map_err(|e| {
        tracing::error!("DB error inserting retry run {new_run_id}: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    // Same workdir → checkpoint-driven partial re-execution.
    crate::executor::spawn_background_run(
        new_run_id.clone(),
        user.id.clone(),
        "none".to_string(),
        "local".to_string(),
        run.workdir.clone().map(std::path::PathBuf::from),
        crate::executor::RunFlags {
            dry_run: false,
            keep_going: false,
            max_jobs: None,
            samples: vec![],
            targets: vec![],
            resume_failed: true,
            rerun: true,
        },
    );

    Ok(Json(plan))
}

/// POST /api/runs/{id}/cancel
pub async fn cancel_run(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let user = current_user::resolve(authenticated.as_ref());
    let run = load_owned_run(pool, &user, &id).await?;

    // Terminal states are final — cancelling a finished run would silently
    // rewrite its result.
    let is_terminal = matches!(run.status.as_str(), "completed" | "failed" | "cancelled");
    if is_terminal {
        return Err(err(
            StatusCode::CONFLICT,
            "RUN_NOT_ACTIVE",
            format!("Run {id} is already {0} — cannot cancel", run.status),
        ));
    }

    // Persist the cancellation BEFORE signaling: the executor's exit path
    // checks for a 'cancelled' row and skips its own terminal write, so the
    // kill fallout can never flip the status back to completed/failed.
    let now = now_iso();
    sqlx::query(
        "UPDATE runs SET status = 'cancelled', phase = 'cancelled', finished_at = ? WHERE id = ?",
    )
    .bind(&now)
    .bind(&id)
    .execute(pool)
    .await
    .map_err(|e| {
        tracing::error!("DB error cancelling run {id}: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    // Signal the live process group: paused groups need SIGCONT first so the
    // SIGTERM can be delivered, then a bounded grace window before SIGKILL.
    if let Some(pgid) = crate::process_control::pgid(&id) {
        use crate::process_control::{SIGCONT, SIGKILL, SIGTERM, signal_group};
        if run.status == "paused"
            && let Err(e) = signal_group(pgid, SIGCONT)
        {
            tracing::warn!("SIGCONT before cancel failed for run {id} pgid {pgid}: {e}");
        }
        if let Err(e) = signal_group(pgid, SIGTERM) {
            tracing::warn!("SIGTERM failed for run {id} pgid {pgid}: {e}");
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while crate::process_control::pgid(&id).is_some() && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        if crate::process_control::pgid(&id).is_some()
            && let Err(e) = signal_group(pgid, SIGKILL)
        {
            tracing::warn!("SIGKILL failed for run {id} pgid {pgid}: {e}");
        }
        crate::process_control::unregister(&id);
    } else {
        tracing::warn!(
            "cancel for run {id}: no live process group registered (already finished or server restarted)"
        );
    }

    crate::broadcast_event_for(
        "run_cancelled",
        &serde_json::json!({"run_id": id, "cancelled_at": now}),
        Some(&run.user_id),
    );

    Ok(Json(serde_json::json!({
        "run_id": id,
        "status": "cancelled",
        "cancelled_at": now,
    })))
}

/// POST /api/runs/{id}/pause
pub async fn pause_run(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let user = current_user::resolve(authenticated.as_ref());
    let _run = load_owned_run(pool, &user, &id).await?;
    let reason = req
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("user_request");

    // Freeze the live process group (the CLI and every rule subprocess).
    if let Some(pgid) = crate::process_control::pgid(&id)
        && let Err(e) = crate::process_control::signal_group(pgid, crate::process_control::SIGSTOP)
    {
        return Err(err(
            StatusCode::CONFLICT,
            "PAUSE_ERROR",
            format!("Failed to pause run {id}: {e}"),
        ));
    }

    let now = now_iso();
    sqlx::query("UPDATE runs SET status = 'paused', phase = ? WHERE id = ?")
        .bind(format!("paused: {reason}"))
        .bind(&id)
        .execute(pool)
        .await
        .map_err(|e| {
            tracing::error!("DB error pausing run {id}: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?;

    crate::broadcast_event_for(
        "run_paused",
        &serde_json::json!({"run_id": id, "reason": reason}),
        Some(&_run.user_id),
    );

    Ok(Json(serde_json::json!({
        "run_id": id,
        "status": "paused",
        "reason": reason,
        "paused_at": now,
    })))
}

/// POST /api/runs/{id}/resume
pub async fn resume_run(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let user = current_user::resolve(authenticated.as_ref());
    let _run = load_owned_run(pool, &user, &id).await?;
    let from_rule = req.get("from_rule").and_then(|v| v.as_str());

    // Unfreeze the live process group.
    if let Some(pgid) = crate::process_control::pgid(&id)
        && let Err(e) = crate::process_control::signal_group(pgid, crate::process_control::SIGCONT)
    {
        return Err(err(
            StatusCode::CONFLICT,
            "RESUME_ERROR",
            format!("Failed to resume run {id}: {e}"),
        ));
    }

    let _now = now_iso();
    sqlx::query("UPDATE runs SET status = 'running', phase = 'executing' WHERE id = ?")
        .bind(&id)
        .execute(pool)
        .await
        .map_err(|e| {
            tracing::error!("DB error resuming run {id}: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?;

    crate::broadcast_event_for(
        "run_resumed",
        &serde_json::json!({"run_id": id, "from_rule": from_rule}),
        Some(&_run.user_id),
    );

    Ok(Json(serde_json::json!({
        "run_id": id,
        "status": "running",
        "from_rule": from_rule,
    })))
}

/// GET /api/runs/{id}/preview — the instance-level dry-run plan
/// (checkpoint_preview + execution_order), persisted by the executor when
/// a dry-run completes (issue #79 P2: the web preview previously showed
/// unexpanded rules and dropped samples/targets).
pub async fn get_run_preview(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;
    let user = current_user::resolve(authenticated.as_ref());
    let run = load_owned_run(pool, &user, &id).await?;
    let workdir = run.workdir.as_deref().unwrap_or("");
    let content =
        std::fs::read_to_string(std::path::Path::new(workdir).join("dry-run-preview.json"))
            .map_err(|_| {
                err(
                    StatusCode::NOT_FOUND,
                    "NO_PREVIEW",
                    "No dry-run preview for this run".into(),
                )
            })?;
    let value: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "PREVIEW_PARSE_ERROR",
            format!("Preview file unreadable: {e}"),
        )
    })?;
    Ok(Json(value))
}

/// GET /api/runs/{id}/ai-status
pub async fn get_ai_status(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let user = current_user::resolve(authenticated.as_ref());
    let _run = load_owned_run(pool, &user, &id).await?;

    let exec_nodes: Vec<NodeExecutionStatus> = checkpoint_status::load_node_statuses(
        std::path::Path::new(_run.workdir.as_deref().unwrap_or("")),
        _run.status == "running",
    )
    .iter()
    .map(|n| NodeExecutionStatus {
        rule: n.rule.clone(),
        status: n.status.to_string(),
        duration_ms: n.duration_ms.map(|d| d as i64),
        exit_code: n.exit_code,
        started_at: n.started_at.clone(),
    })
    .collect();

    // Real telemetry (issue #82 P1-2): memory/CPU of the run's process
    // tree, sampled by the executor; the trend timeline travels in the
    // response alongside the derived analysis.
    let workdir = std::path::Path::new(_run.workdir.as_deref().unwrap_or(""));
    let metrics = checkpoint_status::load_metrics(workdir);
    let last = metrics.last();
    let host = crate::sys::get_host_resources();
    let resources = ResourceUsage {
        cpu_pct: last.and_then(|m| m["cpu_pct"].as_f64()).unwrap_or(0.0),
        memory_mb: last.and_then(|m| m["memory_mb"].as_f64()).unwrap_or(0.0),
        memory_pct: last
            .and_then(|m| m["memory_mb"].as_f64())
            .map(|mb| mb * 100.0 / host.total_memory_mb as f64)
            .unwrap_or(0.0),
        memory_total_mb: host.total_memory_mb as f64,
        disk_pct: 0.0,
        disk_mb: 0.0,
    };
    let status = monitor_agent::analyze_run_status(&exec_nodes, &resources);
    let mut payload = serde_json::json!(status);
    payload["timeline"] = serde_json::json!(metrics);

    Ok(Json(payload))
}

/// GET /api/runs/{id}/report
/// Build the deterministic report data from a run row — shared by the report
/// page, the Q&A, and the visualization endpoints so they all answer from
/// the SAME real data.
fn build_report_for_run(run: &models::RunRow) -> crate::domains::ai::agents::types::ReportData {
    let pipeline_name = oxo_flow_core::WorkflowConfig::parse(&run.pipeline_snapshot)
        .ok()
        .map(|c| c.workflow.name.clone())
        .unwrap_or_else(|| "pipeline".into());

    // Recursive listing: nested products must reach the report page and the
    // AI narrative (issue #79 P1-08 — the narrative claimed "4 output files"
    // while the real products lived in subdirectories).
    let files: Vec<ReportFile> = run
        .workdir
        .as_ref()
        .map(|wd| {
            service::list_files_recursive(std::path::Path::new(wd))
                .into_iter()
                .map(|f| ReportFile {
                    path: f.path,
                    name: f.name,
                    size_bytes: f.size_bytes,
                    is_dir: f.is_dir,
                })
                .collect()
        })
        .unwrap_or_default();

    let log_summary = run
        .workdir
        .as_ref()
        .and_then(|wd| std::fs::read_to_string(format!("{wd}/execution.log")).ok())
        .unwrap_or_default();

    report_agent::generate_report(&pipeline_name, &files, &log_summary, &[])
}

pub async fn get_run_report(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let user = current_user::resolve(authenticated.as_ref());
    let run = load_owned_run(pool, &user, &id).await?;

    let report = build_report_for_run(&run);

    Ok(Json(serde_json::json!(report)))
}

/// POST /api/runs/{id}/report/ask
pub async fn ask_report_question(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> ApiResult<String> {
    let question = req
        .get("question")
        .and_then(|v| v.as_str())
        .unwrap_or("what are the results");
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let user = current_user::resolve(authenticated.as_ref());
    let run = load_owned_run(pool, &user, &id).await?;

    // Answer from the same deterministic report the report page shows.
    let report = build_report_for_run(&run);
    let answer = report_agent::answer_question(&report, question);
    Ok(Json(answer))
}

/// POST /api/runs/{id}/report/visualize
pub async fn visualize_report(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    let chart_type = req
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("volcano");
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let user = current_user::resolve(authenticated.as_ref());
    let run = load_owned_run(pool, &user, &id).await?;

    // Real data sources: the run's file tree, or per-rule timings from the
    // engine's checkpoint. No canned rows.
    let report = build_report_for_run(&run);
    let data: Vec<serde_json::Value> = if chart_type == "files" {
        report
            .file_tree
            .iter()
            .map(|f| serde_json::json!({"name": f.name, "size_bytes": f.size_bytes}))
            .collect()
    } else {
        checkpoint_status::load_node_statuses(
            std::path::Path::new(run.workdir.as_deref().unwrap_or("")),
            false,
        )
        .into_iter()
        .map(|n| {
            serde_json::json!({
                "rule": n.rule,
                "duration_secs": n.duration_ms.map(|d| d as f64 / 1000.0).unwrap_or(0.0),
            })
        })
        .collect()
    };

    let plot_json = serde_json::json!({
        "chart_type": chart_type,
        "title": format!("{chart_type} chart"),
        "spec": if chart_type == "files" {
            serde_json::json!({
                "mark": "bar",
                "encoding": {
                    "x": {"field": "name", "type": "nominal", "title": "File"},
                    "y": {"field": "size_bytes", "type": "quantitative", "title": "Bytes"},
                }
            })
        } else {
            serde_json::json!({
                "mark": "bar",
                "encoding": {
                    "x": {"field": "rule", "type": "nominal", "title": "Rule"},
                    "y": {"field": "duration_secs", "type": "quantitative", "title": "Seconds"},
                }
            })
        },
        "data": data,
    });

    Ok(Json(plot_json))
}
