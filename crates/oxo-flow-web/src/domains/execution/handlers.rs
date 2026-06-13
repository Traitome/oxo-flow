//! HTTP handlers for execution domain.
//!
//! Thin adapters: parse HTTP request → call service → serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

use axum::{Json, extract::Path, http::StatusCode};

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

/// POST /api/runs
pub async fn create_run(Json(req): Json<serde_json::Value>) -> ApiResult<CreateRunResponse> {
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

    let resp = service::create_run(toml, &config, None)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "RUN_ERROR", e))?;

    // Persist run to database
    if let Ok(pool) = crate::infra::db::sqlite::try_pool() {
        let user_id = "default".to_string();
        let run = models::RunRow {
            id: resp.run_id.clone(),
            user_id,
            pipeline_id: String::new(),
            pipeline_snapshot: toml.to_string(),
            status: "queued".to_string(),
            phase: "parsing".to_string(),
            pid: None,
            workdir: None,
            started_at: None,
            finished_at: None,
            created_at: now_iso(),
        };
        let _ = sqlx::query(
            "INSERT OR IGNORE INTO runs (id, user_id, pipeline_id, pipeline_snapshot, status, phase, pid, workdir, started_at, finished_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .execute(pool)
        .await;
    }

    Ok(Json(resp))
}

/// GET /api/runs
pub async fn list_runs() -> ApiResult<Vec<serde_json::Value>> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let rows: Vec<models::RunRow> =
        sqlx::query_as("SELECT * FROM runs ORDER BY created_at DESC LIMIT 100")
            .fetch_all(pool)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let list: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "user_id": r.user_id,
                "pipeline_id": r.pipeline_id,
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
pub async fn get_run(Path(id): Path<String>) -> ApiResult<serde_json::Value> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let run: Option<models::RunRow> = sqlx::query_as("SELECT * FROM runs WHERE id = ?")
        .bind(&id)
        .fetch_optional(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    match run {
        Some(r) => Ok(Json(serde_json::json!({
            "id": r.id,
            "user_id": r.user_id,
            "pipeline_id": r.pipeline_id,
            "pipeline_snapshot": r.pipeline_snapshot,
            "status": r.status,
            "phase": r.phase,
            "pid": r.pid,
            "workdir": r.workdir,
            "started_at": r.started_at,
            "finished_at": r.finished_at,
            "created_at": r.created_at,
        }))),
        None => Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Run {id} not found"),
        )),
    }
}

/// GET /api/runs/{id}/status
pub async fn get_run_status(Path(id): Path<String>) -> ApiResult<RunStatusResponse> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let run: Option<models::RunRow> = sqlx::query_as("SELECT * FROM runs WHERE id = ?")
        .bind(&id)
        .fetch_optional(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let run = run.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Run {id} not found"),
        )
    })?;

    let nodes: Vec<models::RunNodeRow> =
        sqlx::query_as("SELECT * FROM run_nodes WHERE run_id = ? ORDER BY attempt ASC")
            .bind(&id)
            .fetch_all(pool)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let node_items: Vec<NodeStatusItem> = nodes
        .into_iter()
        .map(|n| {
            let status = match n.status.as_str() {
                "pending" => NodeStatus::Pending,
                "running" => NodeStatus::Running,
                "success" => NodeStatus::Success,
                "failed" => NodeStatus::Failed,
                "skipped" => NodeStatus::Skipped,
                _ => NodeStatus::Pending,
            };
            let started_at = n.started_at.clone();
            let finished_at = n.finished_at.clone();
            let duration_ms = finished_at.as_ref().and_then(|f| {
                started_at.as_ref().and_then(|s| {
                    let sf = chrono::NaiveDateTime::parse_from_str(s, "%+").ok()?;
                    let ff = chrono::NaiveDateTime::parse_from_str(f, "%+").ok()?;
                    Some((ff - sf).num_milliseconds().max(0) as u64)
                })
            });
            NodeStatusItem {
                rule: n.rule_name,
                status,
                started_at: n.started_at,
                duration_ms,
                exit_code: n.exit_code,
                progress_pct: None,
            }
        })
        .collect();

    let overall = service::compute_overall_status(&node_items);

    Ok(Json(RunStatusResponse {
        status: overall,
        phase: run.phase,
        nodes: node_items,
        timeline: vec![],
        resources: ResourceSnapshot::default(),
    }))
}

/// GET /api/runs/{id}/dag-status
pub async fn get_dag_status(Path(id): Path<String>) -> ApiResult<DagStatusResponse> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let run: Option<models::RunRow> = sqlx::query_as("SELECT * FROM runs WHERE id = ?")
        .bind(&id)
        .fetch_optional(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let run = run.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Run {id} not found"),
        )
    })?;

    // Parse the pipeline snapshot to build DAG
    let dag = oxo_flow_core::WorkflowConfig::parse(&run.pipeline_snapshot)
        .ok()
        .and_then(|wf| oxo_flow_core::dag::WorkflowDag::from_rules(&wf.rules).ok());

    let nodes: Vec<models::RunNodeRow> =
        sqlx::query_as("SELECT * FROM run_nodes WHERE run_id = ? ORDER BY attempt ASC")
            .bind(&id)
            .fetch_all(pool)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let dag_nodes: Vec<DagNode> = nodes
        .iter()
        .map(|n| {
            let color = match n.status.as_str() {
                "success" => "green",
                "running" => "blue",
                "failed" => "red",
                "skipped" => "gray",
                _ => "lightgray",
            };
            let started_at = n.started_at.clone();
            let finished_at = n.finished_at.clone();
            let duration_ms = finished_at.as_ref().and_then(|f| {
                started_at.as_ref().and_then(|s| {
                    let sf = chrono::NaiveDateTime::parse_from_str(s, "%+").ok()?;
                    let ff = chrono::NaiveDateTime::parse_from_str(f, "%+").ok()?;
                    Some((ff - sf).num_milliseconds().max(0) as u64)
                })
            });
            DagNode {
                id: n.rule_name.clone(),
                label: n.rule_name.clone(),
                status: n.status.clone(),
                color: color.to_string(),
                duration_ms,
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

/// GET /api/runs/{id}/diagnostics
pub async fn get_diagnostics(Path(id): Path<String>) -> ApiResult<DiagnosticsResponse> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let run: Option<models::RunRow> = sqlx::query_as("SELECT * FROM runs WHERE id = ?")
        .bind(&id)
        .fetch_optional(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let run = run.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Run {id} not found"),
        )
    })?;

    let nodes: Vec<models::RunNodeRow> =
        sqlx::query_as("SELECT * FROM run_nodes WHERE run_id = ? ORDER BY attempt ASC")
            .bind(&id)
            .fetch_all(pool)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let node_items: Vec<NodeStatusItem> = nodes
        .iter()
        .map(|n| {
            let status = match n.status.as_str() {
                "success" => NodeStatus::Success,
                "failed" => NodeStatus::Failed,
                "running" => NodeStatus::Running,
                "skipped" => NodeStatus::Skipped,
                _ => NodeStatus::Pending,
            };
            NodeStatusItem {
                rule: n.rule_name.clone(),
                status,
                started_at: n.started_at.clone(),
                duration_ms: None,
                exit_code: n.exit_code,
                progress_pct: None,
            }
        })
        .collect();

    // Try to read log output from workdir
    let log_output = run
        .workdir
        .as_ref()
        .map(|wd| std::fs::read_to_string(format!("{wd}/execution.log")).unwrap_or_default())
        .unwrap_or_default();

    let diagnostics = service::diagnose_run(&node_items, &log_output);
    Ok(Json(diagnostics))
}

/// GET /api/runs/{id}/logs
pub async fn get_run_logs(Path(id): Path<String>) -> ApiResult<String> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let run: Option<models::RunRow> = sqlx::query_as("SELECT * FROM runs WHERE id = ?")
        .bind(&id)
        .fetch_optional(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let run = run.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Run {id} not found"),
        )
    })?;

    let log_content = run
        .workdir
        .as_ref()
        .and_then(|wd| std::fs::read_to_string(format!("{wd}/execution.log")).ok())
        .unwrap_or_else(|| "No execution log available.".to_string());

    Ok(Json(log_content))
}

/// GET /api/runs/{id}/results
pub async fn get_run_results(Path(id): Path<String>) -> ApiResult<Vec<serde_json::Value>> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let run: Option<models::RunRow> = sqlx::query_as("SELECT * FROM runs WHERE id = ?")
        .bind(&id)
        .fetch_optional(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let run = run.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Run {id} not found"),
        )
    })?;

    // List files in workdir if it exists
    let results: Vec<serde_json::Value> = run
        .workdir
        .as_ref()
        .and_then(|wd| {
            std::fs::read_dir(wd).ok().map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .map(|e| {
                        let path = e.path();
                        let meta = path.metadata().ok();
                        serde_json::json!({
                            "name": e.file_name().to_string_lossy(),
                            "path": path.to_string_lossy(),
                            "size_bytes": meta.as_ref().map(|m| m.len()).unwrap_or(0),
                            "is_dir": meta.map(|m| m.is_dir()).unwrap_or(false),
                        })
                    })
                    .collect()
            })
        })
        .unwrap_or_default();

    Ok(Json(results))
}

/// POST /api/runs/{id}/retry
pub async fn retry_run(
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

    let run: Option<models::RunRow> = sqlx::query_as("SELECT * FROM runs WHERE id = ?")
        .bind(&id)
        .fetch_optional(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let run = run.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Run {id} not found"),
        )
    })?;

    let from_rule = req.get("from_rule").and_then(|v| v.as_str());
    let skip_succeeded = req
        .get("skip_succeeded")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let nodes: Vec<models::RunNodeRow> =
        sqlx::query_as("SELECT * FROM run_nodes WHERE run_id = ? ORDER BY attempt ASC")
            .bind(&id)
            .fetch_all(pool)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let node_items: Vec<NodeStatusItem> = nodes
        .iter()
        .map(|n| {
            let status = match n.status.as_str() {
                "success" => NodeStatus::Success,
                "failed" => NodeStatus::Failed,
                "running" => NodeStatus::Running,
                "skipped" => NodeStatus::Skipped,
                _ => NodeStatus::Pending,
            };
            NodeStatusItem {
                rule: n.rule_name.clone(),
                status,
                started_at: n.started_at.clone(),
                duration_ms: None,
                exit_code: n.exit_code,
                progress_pct: None,
            }
        })
        .collect();

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

    service::compute_retry_plan(&node_items, &dag, from_rule, skip_succeeded)
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "RETRY_ERROR", e))
}

/// POST /api/runs/{id}/cancel
pub async fn cancel_run(Path(id): Path<String>) -> ApiResult<serde_json::Value> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let run: Option<models::RunRow> = sqlx::query_as("SELECT * FROM runs WHERE id = ?")
        .bind(&id)
        .fetch_optional(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    match run {
        Some(_r) => {
            let now = now_iso();
            sqlx::query("UPDATE runs SET status = 'cancelled', finished_at = ? WHERE id = ?")
                .bind(&now)
                .bind(&id)
                .execute(pool)
                .await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

            Ok(Json(serde_json::json!({
                "run_id": id,
                "status": "cancelled",
                "cancelled_at": now,
            })))
        }
        None => Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Run {id} not found"),
        )),
    }
}
