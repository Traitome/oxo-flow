//! Run-related handlers.
//!
//! Handles workflow run listing, detail retrieval, log access, and cancellation.

use axum::{extract::Json, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};

use crate::{ApiError, ErrorResponse, broadcast_event, db, extract_session, hpc, workspace};

/// Detailed run status response with log preview.
#[derive(Serialize, Deserialize)]
pub struct RunDetail {
    pub id: String,
    pub user_id: String,
    pub workflow_name: String,
    pub status: String,
    pub pid: Option<i64>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub log_tail: Option<String>,
    pub output_files: Vec<String>,
}

/// `GET /api/runs` — List all runs for the authenticated user.
pub async fn list_runs(headers: axum::http::HeaderMap) -> Result<Json<Vec<db::Run>>, ApiError> {
    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            code: "AUTH_REQUIRED".to_string(),
            message: "Authentication required".to_string(),
            detail: None,
            suggestion: None,
        },
    })?;

    let user = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found", None))?;

    let runs = sqlx::query_as::<_, db::Run>(
        "SELECT * FROM runs WHERE user_id = ? ORDER BY started_at DESC",
    )
    .bind(&user.id)
    .fetch_all(db::pool())
    .await
    .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

    Ok(Json(runs))
}

/// `GET /api/runs/{id}` — Get detailed run status with log preview.
pub async fn get_run_detail(
    headers: axum::http::HeaderMap,
    axum::extract::Path(run_id): axum::extract::Path<String>,
) -> Result<Json<RunDetail>, ApiError> {
    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            code: "AUTH_REQUIRED".to_string(),
            message: "Authentication required".to_string(),
            detail: None,
            suggestion: None,
        },
    })?;

    let user = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found", None))?;

    let run = sqlx::query_as::<_, db::Run>("SELECT * FROM runs WHERE id = ? AND user_id = ?")
        .bind(&run_id)
        .bind(&user.id)
        .fetch_optional(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError {
            status: StatusCode::NOT_FOUND,
            body: ErrorResponse {
                code: "NOT_FOUND".to_string(),
                message: "Run not found".to_string(),
                detail: None,
                suggestion: None,
            },
        })?;

    let run_dir = workspace::get_run_directory(&user.username, &run_id);
    let log_path = run_dir.join("execution.log");

    let log_tail = if log_path.exists()
        && let Ok(content) = std::fs::read_to_string(&log_path)
    {
        // Return last 50 lines for preview
        let lines: Vec<&str> = content.lines().collect();
        let start = lines.len().saturating_sub(50);
        Some(lines[start..].join("\n"))
    } else {
        None
    };

    let mut output_files = Vec::new();
    if run_dir.exists()
        && let Ok(entries) = std::fs::read_dir(&run_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path.file_name().and_then(|n| n.to_str()) != Some("workflow.oxoflow")
                && path.file_name().and_then(|n| n.to_str()) != Some("execution.log")
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                output_files.push(name.to_string());
            }
        }
    }

    Ok(Json(RunDetail {
        id: run.id,
        user_id: run.user_id,
        workflow_name: run.workflow_name,
        status: run.status,
        pid: run.pid,
        started_at: run.started_at.map(|t| t.to_rfc3339()),
        finished_at: run.finished_at.map(|t| t.to_rfc3339()),
        log_tail,
        output_files,
    }))
}

/// `GET /api/runs/{id}/logs` — Get full execution logs for a run.
pub async fn get_run_logs(
    headers: axum::http::HeaderMap,
    axum::extract::Path(run_id): axum::extract::Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            code: "AUTH_REQUIRED".to_string(),
            message: "Authentication required".to_string(),
            detail: None,
            suggestion: None,
        },
    })?;

    let user = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found", None))?;

    // Verify ownership
    let _run = sqlx::query_as::<_, db::Run>("SELECT * FROM runs WHERE id = ? AND user_id = ?")
        .bind(&run_id)
        .bind(&user.id)
        .fetch_optional(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError {
            status: StatusCode::NOT_FOUND,
            body: ErrorResponse {
                code: "NOT_FOUND".to_string(),
                message: "Run not found".to_string(),
                detail: None,
                suggestion: None,
            },
        })?;

    let run_dir = workspace::get_run_directory(&user.username, &run_id);
    let log_path = run_dir.join("execution.log");

    if !log_path.exists() {
        return Err(ApiError::bad_request("Log file not found", None));
    }

    let content = std::fs::read_to_string(log_path)
        .map_err(|e| ApiError::unprocessable("Failed to read log", Some(e.to_string())))?;

    Ok(content)
}

/// `DELETE /api/runs/{id}` — Cancel a running workflow.
pub async fn cancel_run(
    headers: axum::http::HeaderMap,
    axum::extract::Path(run_id): axum::extract::Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            code: "AUTH_REQUIRED".to_string(),
            message: "Authentication required".to_string(),
            detail: None,
            suggestion: None,
        },
    })?;

    let user = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found", None))?;

    // Verify ownership
    let run = sqlx::query_as::<_, db::Run>("SELECT * FROM runs WHERE id = ? AND user_id = ?")
        .bind(&run_id)
        .bind(&user.id)
        .fetch_optional(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError {
            status: StatusCode::NOT_FOUND,
            body: ErrorResponse {
                code: "NOT_FOUND".to_string(),
                message: "Run not found".to_string(),
                detail: None,
                suggestion: None,
            },
        })?;

    if run.status != "running" && run.status != "pending" {
        return Err(ApiError::bad_request(
            "Run is not in a cancellable state",
            Some(run.status),
        ));
    }

    // Cancel the run (kill the process if it exists)
    if let Some(pid) = run.pid {
        use sysinfo::System;
        let mut sys = System::new();
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        if let Some(process) = sys.process(sysinfo::Pid::from_u32(pid as u32)) {
            process.kill();
        }
    }

    sqlx::query("UPDATE runs SET status = 'cancelled', finished_at = ? WHERE id = ?")
        .bind(chrono::Utc::now())
        .bind(&run_id)
        .execute(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

    let _ = db::log_action(&user.id, "cancel_run", &run_id).await;

    // Broadcast cancellation event
    broadcast_event(
        "run_cancelled",
        &serde_json::json!({
            "run_id": run_id,
            "status": "cancelled"
        }),
    );

    Ok((
        axum::http::StatusCode::OK,
        Json(serde_json::json!({ "status": "cancelled" })),
    ))
}

/// `GET /api/runs/{id}/results` — Get structured output records for a run.
pub async fn get_run_results(
    headers: axum::http::HeaderMap,
    axum::extract::Path(run_id): axum::extract::Path<String>,
) -> Result<Json<Vec<oxo_flow_core::result::OutputRecord>>, ApiError> {
    let session = crate::extract_session(&headers)
        .await
        .ok_or_else(|| ApiError {
            status: StatusCode::UNAUTHORIZED,
            body: ErrorResponse {
                code: "AUTH_REQUIRED".to_string(),
                message: "Authentication required".to_string(),
                detail: None,
                suggestion: None,
            },
        })?;

    let user = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found", None))?;

    // Verify the run belongs to the user
    sqlx::query_as::<_, db::Run>("SELECT * FROM runs WHERE id = ? AND user_id = ?")
        .bind(&run_id)
        .bind(&user.id)
        .fetch_optional(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError {
            status: StatusCode::NOT_FOUND,
            body: ErrorResponse {
                code: "NOT_FOUND".to_string(),
                message: "Run not found or not owned by user".to_string(),
                detail: None,
                suggestion: None,
            },
        })?;

    let rows = db::get_output_records(&run_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

    let records: Vec<oxo_flow_core::result::OutputRecord> =
        rows.iter().map(|r| r.to_core()).collect();

    Ok(Json(records))
}
/// Request body for HPC cluster submission.
#[derive(Debug, Serialize, Deserialize)]
pub struct HpcSubmitRequest {
    pub scheduler: String,
    #[serde(default)]
    pub partition: Option<String>,
    #[serde(default)]
    pub cpus: Option<u32>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub walltime: Option<String>,
}

/// `POST /api/runs/{id}/hpc-submit` — Submit a run to an HPC cluster.
pub async fn hpc_submit_run(
    headers: axum::http::HeaderMap,
    axum::extract::Path(run_id): axum::extract::Path<String>,
    Json(req): Json<HpcSubmitRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            code: "AUTH_REQUIRED".to_string(),
            message: "Authentication required".to_string(),
            detail: None,
            suggestion: None,
        },
    })?;

    let user = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found", None))?;

    // Verify run exists and user owns it
    let run = sqlx::query_as::<_, db::Run>("SELECT * FROM runs WHERE id = ? AND user_id = ?")
        .bind(&run_id)
        .bind(&user.id)
        .fetch_optional(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError {
            status: StatusCode::NOT_FOUND,
            body: ErrorResponse {
                code: "NOT_FOUND".to_string(),
                message: "Run not found".to_string(),
                detail: None,
                suggestion: None,
            },
        })?;

    // Only submit if run is running or pending
    if run.status != "running" && run.status != "pending" && run.status != "success" {
        return Err(ApiError::bad_request(
            "Run is not in a submittable state",
            Some(run.status),
        ));
    }

    // Try to submit to the HPC scheduler
    let (hpc_job_id, job_status) = match req.scheduler.as_str() {
        "slurm" => {
            let cpus = req.cpus.unwrap_or(4);
            // Write a minimal sbatch script from the workflow
            let script = format!(
                "#!/bin/bash\n#SBATCH --job-name={}\n#SBATCH --cpus-per-task={}\n{}\n{}\noxo-flow run workflow.oxoflow --workdir \"{}\"",
                run.workflow_name,
                cpus,
                req.partition
                    .as_ref()
                    .map(|p| format!("#SBATCH --partition={}", p))
                    .unwrap_or_default(),
                req.memory
                    .as_ref()
                    .map(|m| format!("#SBATCH --mem={}", m))
                    .unwrap_or_default(),
                crate::workspace::get_run_directory(&user.username, &run_id).display(),
            );
            let script_path =
                workspace::get_run_directory(&user.username, &run_id).join("sbatch.sh");
            std::fs::write(&script_path, &script).map_err(|e| {
                ApiError::unprocessable("Failed to write sbatch script", Some(e.to_string()))
            })?;

            hpc::submit_slurm_job(&script_path.to_string_lossy(), &run.workflow_name, cpus)
                .map(|jid| (jid, "submitted"))
                .unwrap_or_else(|_e| (String::new(), "failed"))
        }
        "pbs" => {
            // PBS submission would go here
            return Err(ApiError::bad_request(
                "PBS submission not yet implemented",
                None,
            ));
        }
        _ => {
            return Err(ApiError::bad_request(
                "Unsupported scheduler",
                Some("Use 'slurm' or 'pbs'".to_string()),
            ));
        }
    };

    if job_status == "failed" {
        return Err(ApiError::unprocessable("HPC submission failed", None));
    }

    // Record the HPC job
    let hpc_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now();
    sqlx::query(
        "INSERT INTO hpc_jobs (id, run_id, user_id, scheduler, job_id, partition_name, status, submitted_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&hpc_id)
    .bind(&run_id)
    .bind(&user.id)
    .bind(&req.scheduler)
    .bind(&hpc_job_id)
    .bind(&req.partition)
    .bind(job_status)
    .bind(now)
    .execute(db::pool())
    .await
    .map_err(|e| ApiError::bad_request("Failed to record HPC job", Some(e.to_string())))?;

    let _ = db::log_action(&user.id, "hpc_submit", &run_id).await;

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "hpc_job_id": hpc_job_id,
            "scheduler": req.scheduler,
            "run_id": run_id,
            "status": "submitted"
        })),
    ))
}
