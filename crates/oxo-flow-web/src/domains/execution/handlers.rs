//! HTTP handlers for execution domain.
//!
//! Thin adapters: parse HTTP request → call service → serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

use crate::domains::ai::agents::monitor_agent::{self, NodeExecutionStatus, ResourceUsage};
use crate::domains::ai::agents::report_agent;
use crate::domains::ai::agents::types::ReportFile;
use crate::domains::auth::current_user::{self, CurrentUser};
use crate::extract::ApiJson;
use axum::{Extension, Json, extract::Path, http::StatusCode};

use crate::extract::ApiQuery;

use super::checkpoint_status;
use super::service;
use super::types::*;
use crate::domains::workflow::handlers::{ApiError, can_read_pipeline, err, now_iso};
use crate::infra::db::models;

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

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

/// Run-mutating endpoints require at least the `user` role (#519): the
/// viewer role is documented read-only. Personal mode has no roles.
pub(crate) fn require_run_mutator(
    user: &current_user::CurrentUser,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    if crate::server::running_mode() != "personal" && user.role == "viewer" {
        return Err(err(
            StatusCode::FORBIDDEN,
            "ACCESS_DENIED",
            "Viewer role is read-only — the user role (or admin) is required to mutate runs".into(),
        ));
    }
    Ok(())
}

/// Pre-flight quota usage (threads, memory MB) for a pipeline TOML — the
/// single computation shared by create_run, retry, and resume-checkpoint
/// so every spawn path budgets identically (#519). Memory goes through
/// `effective_memory()` so both the deprecated `memory` field and the
/// modern `[rules.resources] memory` count — collecting only the
/// deprecated field let the preferred syntax bypass the quota entirely
/// (issue #738).
fn quota_usage_for(toml: &str) -> (u32, u64) {
    oxo_flow_core::config::WorkflowConfig::parse(toml)
        .map(|wf| {
            let threads: u32 = wf.rules.iter().map(|r| r.effective_threads().max(1)).sum();
            let memory_mb: u64 = wf
                .rules
                .iter()
                .filter_map(|r| {
                    r.effective_memory()
                        .and_then(oxo_flow_core::scheduler::parse_memory_mb)
                })
                .sum();
            (threads.max(1), memory_mb)
        })
        .unwrap_or((1, 0))
}

/// Light shape gate for `config_overrides` (full semantics live in the
/// CLI's `parse_cli_overrides`): each entry must be a non-empty KEY=VALUE
/// string. Mirrors the CLI's `--arg KEY=VALUE` surface, which the executor
/// appends to the spawned argv in equals form (`--arg=KEY=VALUE`).
fn validate_config_overrides(
    req: &CreateRunRequest,
) -> Result<Vec<String>, (StatusCode, Json<ApiError>)> {
    let overrides = req.config_overrides.clone().unwrap_or_default();
    for kv in &overrides {
        if kv.is_empty() {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "INVALID_CONFIG_OVERRIDE",
                "config_overrides entries must be non-empty KEY=VALUE strings".into(),
            ));
        }
        match kv.split_once('=') {
            Some((k, _)) if !k.trim_start_matches('-').is_empty() => {}
            _ => {
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    "INVALID_CONFIG_OVERRIDE",
                    format!("config_overrides entry '{kv}' must be KEY=VALUE"),
                ));
            }
        }
    }
    Ok(overrides)
}

#[cfg(test)]
mod quota_tests {
    use super::*;

    #[test]
    fn quota_counts_resources_memory_not_just_deprecated_field() {
        // #738: the modern [rules.resources] memory declaration must count
        // toward the quota — collecting only the deprecated `memory` field
        // let the preferred syntax bypass the cap entirely.
        let toml = "\
[workflow]
name = \"quota\"
version = \"1.0.0\"
description = \"quota fixture\"
author = \"tests\"

[[rules]]
name = \"modern\"
output = [\"out.txt\"]
shell = \"echo hi\"

[rules.resources]
memory = \"64G\"

[[rules]]
name = \"legacy\"
output = [\"out2.txt\"]
shell = \"echo hi\"
memory = \"1G\"
";
        let (threads, memory_mb) = quota_usage_for(toml);
        assert_eq!(memory_mb, 64 * 1024 + 1024, "resources.memory must count");
        assert_eq!(threads, 2);
    }

    #[test]
    fn quota_memory_field_precedence_matches_effective_memory() {
        // When both fields are set, the deprecated `memory` wins — the same
        // precedence `effective_memory()` (and the run estimator) applies,
        // so the collectors cannot disagree on field selection.
        let toml = "\
[workflow]
name = \"quota\"
version = \"1.0.0\"
description = \"quota fixture\"
author = \"tests\"

[[rules]]
name = \"both\"
output = [\"a.txt\"]
shell = \"echo a\"
memory = \"1G\"

[rules.resources]
memory = \"64G\"
";
        let (_threads, memory_mb) = quota_usage_for(toml);
        assert_eq!(memory_mb, 1024, "deprecated field takes precedence");

        let toml_resources_only = "\
[workflow]
name = \"quota\"
version = \"1.0.0\"
description = \"quota fixture\"
author = \"tests\"

[[rules]]
name = \"modern\"
output = [\"a.txt\"]
shell = \"echo a\"

[rules.resources]
memory = \"64G\"
";
        let (_threads, memory_mb) = quota_usage_for(toml_resources_only);
        assert_eq!(memory_mb, 64 * 1024, "resources-only rules must count");
    }
}

/// Apply the #213 run-creation limiter and the #82 quota pre-flight to a
/// spawn path (create_run, retry, resume-checkpoint — all of them spawn a
/// real run, so all of them must respect the same budgets, #519).
fn check_spawn_budget(
    user_id: &str,
    usage: (u32, u64),
) -> Result<(), (StatusCode, Json<ApiError>)> {
    if let Err(wait_secs) = run_rate_limit::check(user_id) {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(ApiError {
                code: "RUN_RATE_LIMITED".into(),
                message: format!("Too many runs created recently; retry in {wait_secs}s."),
                detail: Some(format!(
                    "Default allowance: {} runs per minute (override with \
                     OXO_FLOW_RUNS_RATE_LIMIT).",
                    run_rate_limit::DEFAULT_LIMIT
                )),
                suggestion: Some(
                    "Wait for the window to reset, or batch work via `oxo-flow batch` \
                     instead of many small runs."
                        .into(),
                ),
            }),
        ));
    }
    let quota = crate::infra::quota::global_quota_tracker().check(user_id, usage.0, usage.1);
    if !quota.allowed {
        return Err(err(
            StatusCode::TOO_MANY_REQUESTS,
            "QUOTA_EXCEEDED",
            format!(
                "Quota exceeded: {} — see /api/quota for limits",
                quota.violations.join("; ")
            ),
        ));
    }
    Ok(())
}

/// Per-identity sliding-window rate limit for run creation (issue #213).
/// Expensive spawns must not share the global request limiter's budget with
/// health checks and reads. Configurable via `OXO_FLOW_RUNS_RATE_LIMIT`
/// (allowed runs per minute, default 5; 0 disables the limiter entirely).
mod run_rate_limit {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};
    use std::time::{Duration, Instant};

    const WINDOW: Duration = Duration::from_secs(60);
    pub const DEFAULT_LIMIT: u32 = 5;

    fn limit() -> u32 {
        std::env::var("OXO_FLOW_RUNS_RATE_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_LIMIT)
    }

    static HITS: LazyLock<Mutex<HashMap<String, Vec<Instant>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    /// Returns `Err(seconds_until_next_slot)` when over the limit.
    pub fn check(identity: &str) -> Result<(), u64> {
        let max = limit();
        if max == 0 {
            return Ok(()); // disabled
        }
        let now = Instant::now();
        let mut map = HITS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let stamps = map.entry(identity.to_string()).or_default();
        stamps.retain(|t| now.duration_since(*t) < WINDOW);
        if stamps.len() >= max as usize {
            let oldest = stamps.first().copied().unwrap_or(now);
            let wait = WINDOW.saturating_sub(now.duration_since(oldest)).as_secs() + 1;
            return Err(wait);
        }
        stamps.push(now);
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn allows_up_to_limit_then_blocks_with_wait() {
            // Isolated identity so parallel tests don't interfere.
            let id = format!("rate-test-{}", std::process::id());
            for _ in 0..DEFAULT_LIMIT {
                assert!(check(&id).is_ok());
            }
            let wait = check(&id).expect_err("expected limit breach");
            assert!(wait > 0 && wait <= 61);
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/runs",
    tag = "runs",
    request_body = CreateRunRequest,
    responses(
        (status = 200, description = "Success", body = CreateRunResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/runs
pub async fn create_run(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(body): ApiJson<serde_json::Value>,
) -> ApiResult<CreateRunResponse> {
    // Parse through the typed contract (issue #324 F-4) so the type is the
    // single source of truth — but keep the stable MISSING error for a
    // missing `toml_content` instead of a bare serde message.
    //
    // Classification uses the serde error's field path, not message text:
    // `missing_field` errors carry the field name structurally, so a serde
    // wording change across versions cannot silently degrade the stable
    // MISSING code to INVALID_BODY (issue #335 finding 2).
    let req: CreateRunRequest = serde_json::from_value(body).map_err(|e| {
        // `missing field` is a Category::Data error whose Display embeds the
        // field path (`missing field \`toml_content\``); requiring Data rules
        // out Io/Eof/Syntax mismatches and the backtick-quoted path rules out
        // a coincidental mention in an unrelated message.
        let missing_toml = e.classify() == serde_json::error::Category::Data
            && e.to_string().contains("missing field `toml_content`");
        if missing_toml {
            err(
                StatusCode::BAD_REQUEST,
                "MISSING",
                "toml_content required".into(),
            )
        } else {
            err(
                StatusCode::BAD_REQUEST,
                "INVALID_BODY",
                format!("invalid create-run body: {e}"),
            )
        }
    })?;
    // Issue #213: dedicated budget for expensive run creation — separate
    // from the global per-minute request limiter so health checks / reads
    // cannot crowd it out (and vice versa).
    if let Err(wait_secs) = run_rate_limit::check(
        authenticated
            .as_ref()
            .map(|e| e.0.id.as_str())
            .unwrap_or("anonymous"),
    ) {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(ApiError {
                code: "RUN_RATE_LIMITED".into(),
                message: format!("Too many runs created recently; retry in {wait_secs}s."),
                detail: Some(format!(
                    "Default allowance: {} runs per minute (override with \
                     OXO_FLOW_RUNS_RATE_LIMIT).",
                    run_rate_limit::DEFAULT_LIMIT
                )),
                suggestion: Some(
                    "Wait for the window to reset, or batch work via `oxo-flow batch` \
                     instead of many small runs."
                        .into(),
                ),
            }),
        ));
    }

    let user = current_user::resolve(authenticated.as_ref());
    require_run_mutator(&user)?;
    let toml = req.toml_content.as_str();
    let max_jobs = req.max_jobs.unwrap_or(4);
    let config = RunConfig {
        max_jobs: Some(max_jobs),
        dry_run: req.dry_run,
        keep_going: req.keep_going,
        resource_budget: None,
    };

    // Optional remote execution: a configured SSH cluster connection makes
    // the run execute on that host (staged over tar-over-ssh, results
    // pulled back — domains/clusters/remote.rs).
    let cluster_id: Option<String> = req
        .cluster_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(String::from);

    // Optional saved-pipeline linkage (issue #69): runs targeting a saved
    // pipeline execute in the pipeline's persistent workdir, so the CLI's
    // checkpoint (config snapshot, rule fingerprints, input manifests)
    // survives across re-runs and delivers precise invalidation.
    let pipeline_id: Option<String> = req
        .pipeline_id
        .as_deref()
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

    // Quota enforcement (issue #82 P1-9): the tracker was fully
    // implemented but never consulted — every run must pre-flight its
    // resource reservation.
    let quota_usage = quota_usage_for(toml);
    let quota =
        crate::infra::quota::global_quota_tracker().check(&user.id, quota_usage.0, quota_usage.1);
    if !quota.allowed {
        return Err(err(
            StatusCode::TOO_MANY_REQUESTS,
            "QUOTA_EXCEEDED",
            format!(
                "Quota exceeded: {} — see /api/quota for limits",
                quota.violations.join("; ")
            ),
        ));
    }

    // Validate BEFORE anything is persisted: `validate_config_overrides`
    // and the two pure include-key gates below answer 400s, and a 400 must
    // never leave a ghost `queued` runs row behind (PR #930 NIT-3). IO
    // staging (mkdir/write) still happens after the row exists.
    let validated_overrides = validate_config_overrides(&req)?;
    let include_paths: Vec<(String, String)> = req
        .include_files
        .iter()
        .flat_map(|m| m.iter())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (rel, _) in &include_paths {
        if rel == "workflow.oxoflow" {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "INVALID_INCLUDE_PATH",
                "include_files key 'workflow.oxoflow' would overwrite the main workflow".into(),
            ));
        }
        if let Err(reason) = crate::workspace::check_relative_path(rel) {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "INVALID_INCLUDE_PATH",
                format!("include_files key '{rel}' is not a valid relative path: {reason}"),
            ));
        }
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
                // Read permission, not just existence (#519): executing a
                // private pipeline by id exposes its name/DAG/products
                // through the run's own status endpoints. Foreign and
                // unknown ids both answer PIPELINE_NOT_FOUND (no oracle).
                let row: Option<models::PipelineRow> =
                    sqlx::query_as("SELECT * FROM pipelines WHERE id = ?")
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
                if !row.is_some_and(|p| can_read_pipeline(&user, &p)) {
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
            dry_run: config.dry_run.unwrap_or(false),
            pid: None,
            workdir: Some(run_dir.to_string_lossy().to_string()),
            // Order-preserving JSON array (serde_json Vec serialization)
            // so retry can replay the exact argv the client sent.
            config_overrides: Some(serde_json::to_string(&validated_overrides).unwrap_or_default()),
            started_at: None,
            finished_at: None,
            created_at: now_iso(),
            workflow_name: Some(workflow_name),
        };
        if let Err(e) = sqlx::query(
            "INSERT OR IGNORE INTO runs (id, user_id, pipeline_id, pipeline_snapshot, status, phase, dry_run, pid, workdir, config_overrides, started_at, finished_at, created_at, workflow_name) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&run.id)
        .bind(&run.user_id)
        .bind(&run.pipeline_id)
        .bind(&run.pipeline_snapshot)
        .bind(&run.status)
        .bind(&run.phase)
        .bind(run.dry_run)
        .bind(run.pid)
        .bind(&run.workdir)
        .bind(&run.config_overrides)
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
        // subprocess) can read it. `tokio::fs` keeps the small write off
        // the async runtime's blocking threads (#268 item 4, web minor).
        let workflow_path = run_dir.join("workflow.oxoflow");
        if let Err(e) = tokio::fs::write(&workflow_path, toml).await {
            tracing::error!("Failed to write workflow to {:?}: {e}", workflow_path);
        } else {
            tracing::info!("Saved workflow to {:?}", workflow_path);
        }

        // Config-override passthrough: shape-validated above, before the
        // row was persisted (PR #930 NIT-3); full semantics stay inside
        // the CLI, which warns on unknown keys and hard-fails on invalid
        // values for declared keys.
        // Write [[include]] source files beside workflow.oxoflow so the
        // CLI's `from_file` re-parse resolves them via resolve_includes —
        // the same machinery local `oxo-flow run` uses. Keys are
        // workflow-relative workspace paths, gated above; the staging loop
        // below only performs the IO.
        for (rel, content) in &include_paths {
            let dest = run_dir.join(rel);
            if let Some(parent) = dest.parent()
                && let Err(e) = tokio::fs::create_dir_all(parent).await
            {
                tracing::error!("Failed to create include directory {:?}: {e}", parent);
                return Err(err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "RUN_ERROR",
                    "Failed to stage include files".to_string(),
                ));
            }
            if let Err(e) = tokio::fs::write(&dest, content).await {
                tracing::error!("Failed to write include {rel} to {:?}: {e}", dest);
                return Err(err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "RUN_ERROR",
                    format!("Failed to stage include file '{rel}'"),
                ));
            }
        }
        if !include_paths.is_empty() {
            tracing::info!(
                "Staged {} into {:?}",
                oxo_flow_core::plural(include_paths.len(), "include file", "include files"),
                run_dir
            );
        }

        // Stage the acting user's uploaded inputs into the run workspace
        // (issue #276 L1): `POST /api/files` writes to
        // `workspace/users/<u>/inputs/`, but the CLI executes in
        // `runs/<id>/` (or the pipeline's persistent dir) and could never
        // see them — every `metadata_file`/data-referencing workflow failed
        // pre-execution for web-only users. Mirroring the whole tree keeps
        // declared relative paths (`data/meta.tsv`) intact. Best-effort:
        // a user with no uploads (or an IO hiccup on a stale upload) must
        // not block the run; a genuinely needed missing file still fails
        // loudly inside the CLI with its normal diagnostics.
        let staged = crate::workspace::stage_user_inputs(&user.id, &run_dir);
        match staged {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                "Staged {} into {:?}",
                oxo_flow_core::plural(n, "uploaded input file", "uploaded input files"),
                run_dir
            ),
            Err(e) => tracing::warn!("Could not stage user inputs for run {}: {e}", resp.run_id),
        }

        // Reserve the run's resources in the quota tracker; the executor
        // releases them on the terminal state.
        crate::infra::quota::global_quota_tracker().record_start(
            &user.id,
            quota_usage.0,
            quota_usage.1,
        );
        crate::infra::quota::reserve(&resp.run_id, &user.id, quota_usage.0, quota_usage.1);

        // Remote execution: the local workdir is the staging area; the
        // remote host runs the CLI and its results are pulled back.
        if let Some(cid) = cluster_id.as_deref() {
            let cluster_row: Option<models::ClusterRow> =
                sqlx::query_as("SELECT * FROM clusters WHERE id = ? AND enabled = 1")
                    .bind(cid)
                    .fetch_optional(pool)
                    .await
                    .map_err(|e| {
                        tracing::error!("DB error loading cluster {cid}: {e}");
                        err(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "DB_ERROR",
                            "Internal database error".to_string(),
                        )
                    })?;
            let cluster_row = cluster_row.ok_or_else(|| {
                err(
                    StatusCode::NOT_FOUND,
                    "CLUSTER_NOT_FOUND",
                    format!("Cluster {cid} not found or disabled"),
                )
            })?;
            crate::domains::clusters::remote::spawn_remote_run(
                resp.run_id.clone(),
                user.id.clone(),
                cluster_row,
                crate::domains::clusters::remote::RunStaging {
                    local_workdir: run_dir,
                    max_jobs: req.max_jobs,
                    config_overrides: validated_overrides,
                    allow_protected_drift: req.allow_protected_drift.unwrap_or(false),
                    provenance: req.provenance.unwrap_or(false),
                },
            );
            return Ok(Json(resp));
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
                dry_run: req.dry_run.unwrap_or(false),
                keep_going: req.keep_going.unwrap_or(false),
                max_jobs: req.max_jobs,
                samples: req.samples.unwrap_or_default(),
                targets: req.targets.unwrap_or_default(),
                config_overrides: validated_overrides,
                allow_protected_drift: req.allow_protected_drift.unwrap_or(false),
                provenance: req.provenance.unwrap_or(false),
            },
        );
    }

    Ok(Json(resp))
}

/// GET /api/runs — cursor-paginated run list (issue #82 P1-3/P1-13).
///
/// Query: `limit` (default 100, max 500), `cursor` (created_at of the last
/// row of the previous page), `status` filter, `q` search (workflow name
/// or run id prefix). Response envelope: `{items, next_cursor, total}` —
/// `next_cursor: null` means the last page.
///
/// Pagination is cursor-based by design: `created_at` is not unique, but a
/// cursor plus the `ORDER BY created_at DESC` scan is stable, and the
/// `idx_runs_created_at` index keeps it bounded. A `page` parameter is
/// therefore rejected with 400 rather than ignored.
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListRunsQuery {
    pub limit: Option<usize>,
    pub cursor: Option<String>,
    pub status: Option<String>,
    pub q: Option<String>,
    /// Rejected: this endpoint paginates with `cursor`, never by page number.
    pub page: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/runs",
    tag = "runs",
    security(("bearerAuth" = [])),
    params(ListRunsQuery),
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
    )
)]
pub async fn list_runs(
    authenticated: Option<Extension<CurrentUser>>,
    ApiQuery(params): ApiQuery<ListRunsQuery>,
) -> ApiResult<serde_json::Value> {
    // A `page` parameter means the caller expects offset pagination and would
    // silently read the FIRST page forever (the cursor is what actually
    // advances) — fail loudly instead.
    if params.page.is_some() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "PAGE_NOT_SUPPORTED",
            "This endpoint paginates with `cursor`, not `page`".into(),
        ));
    }
    let user = current_user::resolve(authenticated.as_ref());
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;
    let limit = params.limit.unwrap_or(100).clamp(1, 500) as i64;

    // Ownership scoping (issue #82 P0-4): non-admins see their own runs
    // only; admins see the whole fleet. Filters compose on top. Query
    // assembly lives in service.rs (#948 P2-6) so the pagination/filter
    // semantics are unit-testable without HTTP plumbing.
    let filters = service::RunListFilters {
        user_id: if user.is_admin() {
            None
        } else {
            Some(user.id.clone())
        },
        status: params.status.clone(),
        q: params.q.clone(),
        cursor: params.cursor.clone(),
        limit,
    };
    let assembled = service::assemble_run_list_query(&filters);

    // `page_where_sql` only ever contains static fragments pushed inside
    // `assemble_run_list_query` (column names and `?` placeholders) — all
    // user input rides in the binds. Fetch limit+1 to detect a next page
    // without a COUNT(*).
    let sql = format!(
        "SELECT * FROM runs{} ORDER BY created_at DESC LIMIT {}",
        assembled.page_where_sql,
        limit + 1
    );
    let mut query = sqlx::query_as::<_, models::RunRow>(sqlx::AssertSqlSafe(sql));
    for bind in &assembled.page_binds {
        query = query.bind(bind);
    }
    let rows: Vec<models::RunRow> = query.fetch_all(pool).await.map_err(|e| {
        tracing::error!("DB error listing runs: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    let has_more = rows.len() as i64 > limit;
    let rows = if has_more {
        &rows[..limit as usize]
    } else {
        &rows[..]
    };
    // `next_cursor: null` means "last page" (the documented contract, and
    // what the frontend Load More relies on) — only emit it when another
    // page actually exists (audit #651).
    let next_cursor = if has_more {
        rows.last().map(|r| r.created_at.clone())
    } else {
        None
    };

    // Total = number of rows matching the FILTERS (no LIMIT, no cursor).
    // Bounded by SQLite's rowid; fine for a per-deployment run log.
    let count_sql = format!("SELECT COUNT(*) FROM runs{}", assembled.filter_where_sql);
    let mut count_query = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(count_sql));
    for bind in &assembled.filter_binds {
        count_query = count_query.bind(bind);
    }
    let total: i64 = count_query.fetch_one(pool).await.unwrap_or(0);

    let items: Vec<serde_json::Value> = rows.iter().map(service::run_row_to_list_item).collect();

    Ok(Json(serde_json::json!({
        "items": items,
        "next_cursor": next_cursor,
        "total": total,
    })))
}

#[utoipa::path(
    get,
    path = "/api/runs/{id}",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 404, description = "Error", body = ApiError),
    )
)]
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
        "dry_run": run.dry_run,
        // No `pid` — same reason as the list endpoint: the host process id
        // stays engine-internal.
        "workdir": run.workdir,
        "started_at": run.started_at,
        "finished_at": run.finished_at,
        "created_at": run.created_at,
    })))
}

#[utoipa::path(
    get,
    path = "/api/runs/{id}/status",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = RunStatusResponse),
        (status = 404, description = "Error", body = ApiError),
    )
)]
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
    // Config expansion matches `run` (issue #707): {config.*}-routed
    // producer/consumer pairs only match after placeholder expansion.
    let dag = oxo_flow_core::WorkflowConfig::parse(&run.pipeline_snapshot)
        .ok()
        .and_then(|wf| {
            oxo_flow_core::dag::WorkflowDag::from_rules_with_config(
                &wf.rules,
                &wf.config_placeholder_values(),
            )
            .ok()
        });
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
    // empty timeline (never fabricated numbers). Bounded read: this
    // endpoint is polled, so only the newest window is returned.
    let workdir = std::path::Path::new(run.workdir.as_deref().unwrap_or(""));
    let metrics = checkpoint_status::load_metrics_bounded(workdir, 256 * 1024);
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

#[utoipa::path(
    get,
    path = "/api/runs/{id}/dag-status",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = DagStatusResponse),
        (status = 404, description = "Error", body = ApiError),
    )
)]
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

    // Parse the pipeline snapshot to build DAG (config-expanded, issue #707).
    let dag = oxo_flow_core::WorkflowConfig::parse(&run.pipeline_snapshot)
        .ok()
        .and_then(|wf| {
            oxo_flow_core::dag::WorkflowDag::from_rules_with_config(
                &wf.rules,
                &wf.config_placeholder_values(),
            )
            .ok()
        });

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
                stderr_tail: n.stderr_tail.clone(),
                stdout_tail: n.stdout_tail.clone(),
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

    // Compute metrics before moving dag_nodes into the response (no clone of
    // the full node vec). The counters partition `total_nodes`: every node
    // lands in exactly one bucket.
    let failed_nodes = dag_nodes.iter().filter(|n| n.status == "failed").count();
    let running_nodes = dag_nodes.iter().filter(|n| n.status == "running").count();
    let pending_nodes = dag_nodes.iter().filter(|n| n.status == "pending").count();
    let skipped_nodes = dag_nodes.iter().filter(|n| n.status == "skipped").count();

    // ETA based on SUCCESSFUL node durations only (issue #907 P1-2): failed
    // and skipped nodes must not inflate the average, and the remaining-work
    // count must be the nodes still to do (pending + running) — counting
    // failed/skipped as remaining pinned a positive ETA on terminal-failed
    // runs (review finding on #908).
    let completed_duration: u64 = dag_nodes
        .iter()
        .filter(|n| n.status == "success")
        .filter_map(|n| n.duration_ms)
        .sum();
    let completed_count = dag_nodes.iter().filter(|n| n.status == "success").count();
    let eta_ms = if completed_count > 0 {
        let avg_per_node = completed_duration / completed_count as u64;
        let remaining = (pending_nodes + running_nodes) as u64;
        Some(avg_per_node * remaining)
    } else {
        None
    };

    Ok(Json(DagStatusResponse {
        nodes: dag_nodes,
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
            total_nodes: failed_nodes
                + running_nodes
                + pending_nodes
                + skipped_nodes
                + completed_count,
            completed_nodes: completed_count,
            failed_nodes,
            running_nodes,
            pending_nodes,
            skipped_nodes,
            eta_ms,
        },
    }))
}

#[utoipa::path(
    get,
    path = "/api/runs/{id}/instances",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = Vec<serde_json::Value>),
        (status = 404, description = "Error", body = ApiError),
    )
)]
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
        .and_then(|wf| {
            oxo_flow_core::dag::WorkflowDag::from_rules_with_config(
                &wf.rules,
                &wf.config_placeholder_values(),
            )
            .ok()
        });
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
                "stderr_tail": n.stderr_tail,
                "stdout_tail": n.stdout_tail,
            })
        })
        .collect();

    Ok(Json(instances))
}

#[utoipa::path(
    get,
    path = "/api/runs/{id}/diagnostics",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = DiagnosticsResponse),
        (status = 404, description = "Error", body = ApiError),
    )
)]
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

    // Try to read log output from workdir — the newest 256 KiB only:
    // execution.log is the run's full unbounded log, and diagnosis keys
    // off trailing error context, so buffering the whole file per request
    // is pure memory amplification (issue #710).
    let log_output = match run.workdir.as_ref() {
        Some(wd) => checkpoint_status::read_tail_bounded(
            std::path::Path::new(&format!("{wd}/execution.log")),
            256 * 1024,
        ),
        None => String::new(),
    };

    let benchmarks = checkpoint_status::load_benchmarks(std::path::Path::new(
        run.workdir.as_deref().unwrap_or(""),
    ));

    let diagnostics = service::diagnose_run(&node_items, &log_output, &benchmarks);
    Ok(Json(diagnostics))
}

#[utoipa::path(
    get,
    path = "/api/runs/{id}/logs",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = String),
        (status = 404, description = "Error", body = ApiError),
    )
)]
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

    // Bounded tail (issue #734): the endpoint's contract is the log text,
    // but buffering a multi-GB log per request is a memory-amplification
    // surface — the newest 8 MiB covers any reasonable viewer, with an
    // explicit truncation marker so nothing is silently missing.
    const MAX_LOG_ENDPOINT_BYTES: u64 = 8 * 1024 * 1024;
    let log_content = match run.workdir.as_ref() {
        Some(wd) => {
            let log_path = format!("{wd}/execution.log");
            let path = std::path::Path::new(&log_path);
            match std::fs::metadata(path)
                .ok()
                .filter(|m| m.is_file())
                .map(|m| m.len())
            {
                Some(len) if len > MAX_LOG_ENDPOINT_BYTES => {
                    let tail = checkpoint_status::read_tail_bounded(path, MAX_LOG_ENDPOINT_BYTES);
                    format!(
                        "[log truncated — showing the newest {MAX_LOG_ENDPOINT_BYTES} of {len} bytes; read the full file from the run workdir]\n{tail}"
                    )
                }
                Some(_) => checkpoint_status::read_tail_bounded(path, MAX_LOG_ENDPOINT_BYTES),
                None => "No execution log available.".to_string(),
            }
        }
        None => "No execution log available.".to_string(),
    };

    Ok(Json(log_content))
}

#[utoipa::path(
    get,
    path = "/api/runs/{id}/results",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = Vec<serde_json::Value>),
        (status = 404, description = "Error", body = ApiError),
    )
)]
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
    // results/sample1/peaks.bed) are visible (issue #79 P1-08). The walk
    // is bounded (500 entries / depth 4) but still synchronous — run it
    // on the blocking pool (#268 item 4, web minor).
    let results: Vec<serde_json::Value> = match run.workdir.as_ref() {
        Some(wd) => {
            let wd = wd.clone();
            tokio::task::spawn_blocking(move || {
                service::list_files_recursive(std::path::Path::new(&wd))
            })
            .await
            .unwrap_or_default()
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
        }
        None => Vec::new(),
    };

    Ok(Json(results))
}

#[utoipa::path(
    post,
    path = "/api/runs/{id}/retry",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = RetryResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/runs/{id}/retry
pub async fn retry_run(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<serde_json::Value>,
) -> ApiResult<RetryResponse> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let user = current_user::resolve(authenticated.as_ref());
    require_run_mutator(&user)?;
    let run = load_owned_run(pool, &user, &id).await?;

    // A retry spawns a REAL run — it pays the same limiter + quota
    // pre-flight as create_run (#519); looping retry on a cheap failed run
    // must not buy unbounded process spawns.
    let quota_usage = quota_usage_for(&run.pipeline_snapshot);
    check_spawn_budget(&user.id, quota_usage)?;

    // The retry re-executes in the SAME workdir; an active run would put
    // two CLIs on one checkpoint concurrently (corrupted state, duplicated
    // work). Only terminal runs can be retried.
    if matches!(run.status.as_str(), "running" | "queued" | "paused") {
        return Err(err(
            StatusCode::CONFLICT,
            "RUN_NOT_TERMINAL",
            format!(
                "Run {id} is still {} — wait for it to finish (or cancel it) before retrying",
                run.status
            ),
        ));
    }

    // `from_rule` is documented in the API reference, but the engine has no
    // "re-execute from rule X downstream" mode: the retry spawns the CLI with
    // --resume-failed --rerun, which re-runs the checkpoint's failed set. A
    // plan computed from an arbitrary origin would be a promise the spawned
    // run cannot keep, so the field is rejected instead of silently ignored.
    if req.get("from_rule").is_some_and(|v| !v.is_null()) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "UNSUPPORTED_FIELD",
            "from_rule is not supported: a retry re-executes the rules that failed in the \
             previous run (plus their downstream dependents). To re-run a specific rule, \
             remove its outputs or use `oxo-flow run --rerun`."
                .into(),
        ));
    }
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
        .and_then(|wf| {
            oxo_flow_core::dag::WorkflowDag::from_rules_with_config(
                &wf.rules,
                &wf.config_placeholder_values(),
            )
            .ok()
        })
        .ok_or_else(|| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DAG_ERROR",
                "Failed to reconstruct DAG from pipeline snapshot".into(),
            )
        })?;

    let plan = service::compute_retry_plan(&node_items, &dag, skip_succeeded)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "RETRY_ERROR", e))?;

    // Honesty note (issue #760): an empty plan on a FAILED run means the
    // previous run died before executing any rule (e.g. an expansion error
    // at spawn) — the retry re-attempts the same pipeline and re-fails
    // unless the configuration error is fixed. Saying so beats a silent
    // `will_rerun: [] / will_skip: []`.
    let mut plan = plan;
    if plan.will_rerun.is_empty() && plan.will_skip.is_empty() && run.status == "failed" {
        plan.note = Some(format!(
            "previous run failed before executing any rule (phase: {}); the retry \
             re-attempts the same pipeline — fix the reported error first or it will \
             fail identically",
            run.phase
        ));
    }

    // The retry is a REAL run (issue #82 P0-3 — previously the plan was
    // returned with a new_run_id that never existed). Re-executing in the
    // SAME workdir lets the CLI's checkpoint do the work: failed rules have
    // no success record and re-run, and the engine's cascade invalidation
    // marks their downstream dependents dirty too — exactly the computed
    // plan, verified against the same single source of truth.
    let new_run_id = plan.new_run_id.clone();
    let now = now_iso();
    // Replay the original run's config overrides (PR #930 NIT-8): a retry
    // that dropped them would re-execute with default configuration while
    // claiming to redo the same pipeline. Parse errors fall back to no
    // overrides (the value only exists if a pre-check passed at create
    // time, so this is a corrupted-row fallback, not a validation path).
    let retry_overrides: Vec<String> = run
        .config_overrides
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default();
    let new_run = models::RunRow {
        id: new_run_id.clone(),
        user_id: user.id.clone(),
        pipeline_id: run.pipeline_id.clone(),
        pipeline_snapshot: run.pipeline_snapshot.clone(),
        workflow_name: run.workflow_name.clone(),
        status: "queued".to_string(),
        phase: "parsing".to_string(),
        dry_run: run.dry_run,
        pid: None,
        workdir: run.workdir.clone(),
        config_overrides: run.config_overrides.clone(),
        started_at: None,
        finished_at: None,
        created_at: now.clone(),
    };
    sqlx::query(
        "INSERT OR IGNORE INTO runs (id, user_id, pipeline_id, pipeline_snapshot, workflow_name, status, phase, dry_run, pid, workdir, config_overrides, started_at, finished_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&new_run.id)
    .bind(&new_run.user_id)
    .bind(&new_run.pipeline_id)
    .bind(&new_run.pipeline_snapshot)
    .bind(&new_run.workflow_name)
    .bind(&new_run.status)
    .bind(&new_run.phase)
    .bind(new_run.dry_run)
    .bind(new_run.pid)
    .bind(&new_run.workdir)
    .bind(&new_run.config_overrides)
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

    crate::infra::quota::global_quota_tracker().record_start(
        &user.id,
        quota_usage.0,
        quota_usage.1,
    );
    crate::infra::quota::reserve(&new_run_id, &user.id, quota_usage.0, quota_usage.1);

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
            config_overrides: retry_overrides,
            // Retry keeps the protection gate (issue #935): only the
            // original create request may relax it, mirroring the CLI's
            // own resume which hardcodes false.
            allow_protected_drift: false,
            // Checksums are recorded by the original create request only;
            // resume inherits whatever the checkpoint carries.
            provenance: false,
        },
    );

    Ok(Json(plan))
}

#[utoipa::path(
    post,
    path = "/api/runs/{id}/cancel",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 404, description = "Error", body = ApiError),
    )
)]
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
    require_run_mutator(&user)?;
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
    // The quota reservation is released here — the executor's finalize path
    // early-returns for cancelled runs and never reaches its own release.
    if let Some((user_id, threads, memory_mb)) = crate::infra::quota::release(&id) {
        crate::infra::quota::global_quota_tracker().record_complete(&user_id, threads, memory_mb);
    }
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
    //
    // The pgid comes from the in-memory registry, falling back to the pid
    // recorded at spawn (the wrapper's pid doubles as its pgid — see
    // process_control). The registry alone is not enough: finalize_run
    // unregisters the moment the CLI wrapper is reaped, so a cancel landing
    // in that window would otherwise signal nothing and still return 200
    // while orphaned rule processes run on (issue #120). The DB fallback is
    // guarded against pid reuse by liveness + cmdline probes.
    //
    // The fallback must probe the process GROUP, not just the recorded pid
    // (issue #136 tier-2): in exactly the window it exists for — wrapper
    // leader reaped by the executor, registry already unregistered — the
    // wrapper pid probes dead (`probe_alive` answers ESRCH), while
    // `killpg(pgid, 0)` still answers because the orphaned CLI and rule
    // subprocesses keep running under the same pgid. When the leader is
    // gone, the identity guard is applied to the group's surviving members
    // instead, so a recycled pgid belonging to unrelated processes is never
    // signalled.
    let pgid = match crate::process_control::pgid(&id) {
        Some(pgid) => Some(pgid),
        None => {
            // fetch_one is safe — load_owned_run above proved the row exists;
            // the pid column itself is nullable (never spawned / cleaned up).
            let db_pid: Option<i64> = sqlx::query_scalar("SELECT pid FROM runs WHERE id = ?")
                .bind(&id)
                .fetch_one(pool)
                .await
                .unwrap_or(None);
            db_pid.and_then(|pid| {
                let pid = pid as i32;
                if pid <= 0 {
                    return None;
                }
                // Leader alive: probe + identity guard on the pid itself.
                if crate::process_control::probe_alive(pid)
                    && crate::process_control::looks_like_oxo_flow(pid)
                {
                    return Some(pid);
                }
                // Leader reaped but the GROUP outlived it: probe group
                // liveness (the wrapper was the group leader, so pid ==
                // pgid) and guard against pid reuse by scanning the
                // surviving members' command lines.
                if crate::process_control::group_alive(pid)
                    && crate::process_control::group_looks_like_oxo_flow(pid)
                {
                    return Some(pid);
                }
                None
            })
        }
    };

    if let Some(pgid) = pgid {
        use crate::process_control::{SIGCONT, SIGKILL, SIGTERM, group_alive, signal_group};
        if run.status == "paused"
            && let Err(e) = signal_group(pgid, SIGCONT)
        {
            tracing::warn!("SIGCONT before cancel failed for run {id} pgid {pgid}: {e}");
        }
        if let Err(e) = signal_group(pgid, SIGTERM) {
            tracing::error!("SIGTERM failed for run {id} pgid {pgid}: {e}");
        }
        // The grace window watches ACTUAL group liveness, not the registry:
        // the registry entry disappears when the CLI wrapper is reaped, which
        // can precede the death of orphaned rule subprocesses — gating the
        // SIGKILL escalation on it let survivors escape (issue #120).
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while group_alive(pgid) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        if group_alive(pgid) {
            if let Err(e) = signal_group(pgid, SIGKILL) {
                tracing::error!("SIGKILL failed for run {id} pgid {pgid}: {e}");
            }
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
            while group_alive(pgid) && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            if group_alive(pgid) {
                tracing::error!("run {id}: process group {pgid} still alive after SIGKILL");
            }
        }
        crate::process_control::unregister(&id);
    } else if let Some(cluster_id) = crate::domains::clusters::remote::remote_cluster_of(&id) {
        // Remote run: signal the remote wrapper script.
        let cluster_row: Option<models::ClusterRow> =
            sqlx::query_as("SELECT * FROM clusters WHERE id = ?")
                .bind(&cluster_id)
                .fetch_optional(pool)
                .await
                .unwrap_or(None);
        if let Some(cluster_row) = cluster_row
            && let Err(e) = crate::domains::clusters::remote::cancel_remote(&cluster_row, &id).await
        {
            tracing::warn!("remote cancel failed for run {id}: {e}");
        }
        crate::domains::clusters::remote::unregister_remote(&id);
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

#[utoipa::path(
    post,
    path = "/api/runs/{id}/pause",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/runs/{id}/pause
pub async fn pause_run(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    // The body carries no options today; `Option` exempts an empty body
    // from the JSON content-type demand (issue #324 F-6).
    _body: Option<Json<serde_json::Value>>,
) -> ApiResult<serde_json::Value> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let user = current_user::resolve(authenticated.as_ref());
    require_run_mutator(&user)?;
    let run = load_owned_run(pool, &user, &id).await?;
    // Terminal states are final — pausing a finished run would leave a
    // 'paused' row nothing ever finalizes again (ghost run).
    if matches!(run.status.as_str(), "completed" | "failed" | "cancelled") {
        return Err(err(
            StatusCode::CONFLICT,
            "RUN_NOT_ACTIVE",
            format!("Run {id} is already {} — cannot pause", run.status),
        ));
    }
    let reason = _body
        .as_ref()
        .and_then(|Json(v)| v.get("reason"))
        .and_then(|v| v.as_str())
        .unwrap_or("user_request");

    // Freeze the live process group (the CLI and every rule subprocess).
    // The status flip below is conditional on this succeeding: a queued run
    // has no process group yet, and marking it 'paused' would strand the row
    // (nothing to SIGCONT, and its quota reservation would never be
    // released) — a conflict is the honest answer.
    let Some(pgid) = crate::process_control::pgid(&id) else {
        return Err(err(
            StatusCode::CONFLICT,
            "NO_PROCESS_GROUP",
            format!(
                "Run {id} has no live process group to pause (queued or already detached) — \
                 cancel it instead"
            ),
        ));
    };
    if let Err(e) = crate::process_control::signal_group(pgid, crate::process_control::SIGSTOP) {
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
        Some(&run.user_id),
    );

    Ok(Json(serde_json::json!({
        "run_id": id,
        "status": "paused",
        "reason": reason,
        "paused_at": now,
    })))
}

#[utoipa::path(
    post,
    path = "/api/runs/{id}/resume",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/runs/{id}/resume
pub async fn resume_run(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    // `Option` exempts an empty body from the JSON content-type demand
    // (issue #324 F-6); the only consumed key is the optional from_rule.
    body: Option<Json<serde_json::Value>>,
) -> ApiResult<serde_json::Value> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let user = current_user::resolve(authenticated.as_ref());
    require_run_mutator(&user)?;
    let run = load_owned_run(pool, &user, &id).await?;
    // Only a paused run can be resumed; anything else would fabricate a
    // 'running' row with no process behind it (ghost run).
    if run.status != "paused" {
        return Err(err(
            StatusCode::CONFLICT,
            "RUN_NOT_PAUSED",
            format!(
                "Run {id} is {0} — only paused runs can be resumed",
                run.status
            ),
        ));
    }
    // Resuming a paused process unfreezes it in place — there is no plan to
    // recompute, so a `from_rule` origin cannot be honoured (the engine has
    // no re-run-from-rule mode either; see retry_run). Reject it instead of
    // echoing back a field that changes nothing.
    if body
        .as_ref()
        .and_then(|Json(v)| v.get("from_rule"))
        .is_some_and(|v| !v.is_null())
    {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "UNSUPPORTED_FIELD",
            "from_rule is not supported on resume: a paused run continues in place. To \
             re-execute from a specific rule, cancel the run and retry it."
                .into(),
        ));
    }

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
        &serde_json::json!({"run_id": id}),
        Some(&run.user_id),
    );

    Ok(Json(serde_json::json!({
        "run_id": id,
        "status": "running",
    })))
}

#[utoipa::path(
    get,
    path = "/api/runs/{id}/preview",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 404, description = "Error", body = ApiError),
    )
)]
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
    // The workdir is rule-writable: gate the preview read on a regular
    // file (a planted FIFO would hang the open) and cap it (the preview is
    // server-written and KB-scale — anything larger was not written by
    // finalize_run) (issue #735).
    let preview_path = std::path::Path::new(workdir).join("dry-run-preview.json");
    let meta = tokio::fs::metadata(&preview_path).await.map_err(|_| {
        err(
            StatusCode::NOT_FOUND,
            "NO_PREVIEW",
            "No dry-run preview for this run".into(),
        )
    })?;
    if !meta.is_file() {
        return Err(err(
            StatusCode::NOT_FOUND,
            "NO_PREVIEW",
            "No dry-run preview for this run".into(),
        ));
    }
    const MAX_PREVIEW_BYTES: u64 = 16 * 1024 * 1024;
    if meta.len() > MAX_PREVIEW_BYTES {
        return Err(err(
            StatusCode::PAYLOAD_TOO_LARGE,
            "PREVIEW_TOO_LARGE",
            format!(
                "Preview file is {} bytes — larger than the {} byte server-written maximum",
                meta.len(),
                MAX_PREVIEW_BYTES
            ),
        ));
    }
    let content = tokio::fs::read_to_string(&preview_path)
        .await
        .map_err(|_| {
            err(
                StatusCode::NOT_FOUND,
                "NO_PREVIEW",
                "No dry-run preview for this run".into(),
            )
        })?;
    let value: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        tracing::error!("Failed to parse preview file: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "PREVIEW_PARSE_ERROR",
            "Preview file unreadable".to_string(),
        )
    })?;
    Ok(Json(value))
}

#[utoipa::path(
    get,
    path = "/api/runs/{id}/protected-drift",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = ProtectedDriftResponse),
        (status = 404, description = "Error", body = ApiError),
        (status = 503, description = "Error", body = ApiError),
    )
)]
/// GET /api/runs/{id}/protected-drift — read-only protected-tree drift
/// preview (issue #935 CLI→web parity slice 3).
///
/// Diffs every `[engine] protected_data_trees` entry of the run's workflow
/// snapshot against the baseline manifest recorded in the run's checkpoint
/// (`.oxo-flow/checkpoint.json`), reusing the exact core machinery the CLI
/// detector drives (`snapshot_protected_tree` → `manifests_match` /
/// `manifest_changes` → `ManifestChange::describe`). The response shape
/// mirrors the CLI dry-run's `protected_tree_drift` object.
///
/// Read-only by contract: baselines are never persisted here (the CLI's
/// `run` gate adopts them; dry-run previews don't), so repeated calls are
/// stable. Trees without a baseline yet are counted in `new_trees` ("would
/// be baselined"), matching the dry-run narration. Note the preview reads
/// the tree as it exists NOW — like dry-run, it does not reserve the
/// verdict: files can still change between this call and the actual run.
///
/// A corrupt/unreadable checkpoint is surfaced as a `warnings` entry (not a
/// silent all-clear) — absent checkpoint = no baselines yet, corrupt
/// checkpoint = baselines exist but cannot be compared; the two must not
/// look the same.
pub async fn get_run_protected_drift(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<ProtectedDriftResponse> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;
    let user = current_user::resolve(authenticated.as_ref());
    let run = load_owned_run(pool, &user, &id).await?;
    let workdir = run.workdir.as_deref().unwrap_or("").to_string();

    // The protected trees are declared in the workflow's [engine] section —
    // read them from the persisted snapshot, the same source the spawned
    // CLI parses. An unparseable snapshot means no declared trees (the run
    // would have failed before any tree could be protected).
    let config = oxo_flow_core::config::WorkflowConfig::parse(&run.pipeline_snapshot).ok();
    let trees: Vec<String> = config
        .as_ref()
        .and_then(|wf| wf.engine.as_ref())
        .map(|e| e.protected_data_trees().to_vec())
        .unwrap_or_default();
    if trees.is_empty() {
        return Ok(Json(ProtectedDriftResponse {
            drifted_files: Vec::new(),
            new_trees: 0,
            warnings: Vec::new(),
        }));
    }

    // Baselines live in the engine's checkpoint. Absent checkpoint (run
    // staged but not started, or a fresh workdir) = no baselines yet —
    // every existing tree then reports as new_trees, exactly the
    // before-first-run preview.
    let workdir_path = std::path::PathBuf::from(workdir);
    let ck_path = oxo_flow_core::executor::CheckpointState::default_path(&workdir_path);

    // Same loop contract as the CLI detector (run_preview.rs
    // `detect_protected_tree_drift`), minus the baseline recording — the
    // detector mutates `ck` on first observation, the web preview reports
    // the adoption instead (dry-run semantics: nothing persisted).
    // BTreeSet: deterministic order across calls regardless of config order.
    // The checkpoint read + full tree walk + hashing is bounded disk work on
    // a tree that can be large — run it on the blocking pool so a big tree
    // cannot stall a tokio worker (#268 item 4 pattern).
    let tree_set: std::collections::BTreeSet<String> = trees.into_iter().collect();
    let outcome = tokio::task::spawn_blocking(move || {
        let mut warnings: Vec<String> = Vec::new();
        let ck = if ck_path.exists() {
            match oxo_flow_core::executor::CheckpointState::load_from_file(&ck_path) {
                Ok(ck) => ck,
                Err(e) => {
                    // Corrupt/unreadable checkpoint: baselines exist but
                    // cannot be compared. Never present this as a clean
                    // result — surface it (review finding, PR #941).
                    tracing::warn!("checkpoint unreadable at {}: {e}", ck_path.display());
                    warnings.push(format!(
                        "checkpoint unreadable ({}), treating as no baselines: {e}",
                        ck_path.display()
                    ));
                    oxo_flow_core::executor::CheckpointState::new()
                }
            }
        } else {
            oxo_flow_core::executor::CheckpointState::new()
        };

        let mut drifted_files: Vec<String> = Vec::new();
        let mut new_trees = 0usize;
        for tree in &tree_set {
            let rel = tree
                .trim()
                .trim_start_matches("./")
                .trim_end_matches('/')
                .to_string();
            if rel.is_empty() {
                warnings.push("protected_data_trees: empty path value ignored".to_string());
                continue;
            }
            let abs = workdir_path.join(&rel);
            match oxo_flow_core::executor::checkpoint::snapshot_protected_tree(&abs, &workdir_path)
            {
                Ok(Some(current)) => match ck.protected_tree_baselines.get(&rel) {
                    Some(recorded)
                        if oxo_flow_core::executor::checkpoint::manifests_match(
                            recorded, &current,
                        ) =>
                    {
                        // Baseline still matches — nothing to report.
                    }
                    Some(recorded) => {
                        for change in oxo_flow_core::executor::checkpoint::manifest_changes(
                            recorded, &current,
                        ) {
                            drifted_files.push(change.describe());
                        }
                    }
                    None => {
                        // First observation: the run gate would adopt this
                        // baseline; the preview only reports it.
                        new_trees += 1;
                    }
                },
                Ok(None) => {
                    // Tree does not exist yet: adoption deferred, nothing to
                    // preview.
                }
                Err(e) => {
                    warnings.push(format!("protected tree {rel}/ not readable: {e}"));
                }
            }
        }
        ProtectedDriftResponse {
            drifted_files,
            new_trees,
            warnings,
        }
    })
    .await;

    match outcome {
        Ok(response) => Ok(Json(response)),
        Err(e) => {
            tracing::error!("protected-drift preview join failed: {e}");
            Err(err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DRIFT_PREVIEW_FAILED",
                "Protected-drift preview failed".to_string(),
            ))
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/runs/{id}/ai-status",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 404, description = "Error", body = ApiError),
    )
)]
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
        stderr_tail: n.stderr_tail.clone(),
        stdout_tail: n.stdout_tail.clone(),
    })
    .collect();

    // Real telemetry (issue #82 P1-2): memory/CPU of the run's process
    // tree, sampled by the executor; the trend timeline travels in the
    // response alongside the derived analysis. Bounded read: this endpoint
    // is polled every 5s while the monitor tab is visible.
    let workdir = std::path::Path::new(_run.workdir.as_deref().unwrap_or(""));
    let metrics = checkpoint_status::load_metrics_bounded(workdir, 256 * 1024);
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
async fn build_report_for_run(
    run: &models::RunRow,
) -> crate::domains::ai::agents::types::ReportData {
    let pipeline_name = oxo_flow_core::WorkflowConfig::parse(&run.pipeline_snapshot)
        .ok()
        .map(|c| c.workflow.name.clone())
        .unwrap_or_else(|| "pipeline".into());

    // Recursive listing: nested products must reach the report page and the
    // AI narrative (issue #79 P1-08 — the narrative claimed "4 output files"
    // while the real products lived in subdirectories). Synchronous but
    // bounded walk → blocking pool (#268 item 4, web minor).
    let files: Vec<ReportFile> = match run.workdir.as_ref() {
        Some(wd) => {
            let wd = wd.clone();
            let workdir = std::path::PathBuf::from(&wd);
            tokio::task::spawn_blocking(move || {
                service::list_files_recursive(std::path::Path::new(&wd))
            })
            .await
            .unwrap_or_default()
            .into_iter()
            // P1-6 (#907): the engine's bookkeeping files sit at the workdir
            // root next to real products — counting `execution.log` /
            // `workflow.oxoflow` as outputs inflated "N output files" in the
            // narrative. (The /results browser still lists them on purpose:
            // users may want to open the log from there.)
            .filter(move |f| {
                let p = std::path::Path::new(&f.path);
                p != workdir.join("execution.log") && p != workdir.join("workflow.oxoflow")
            })
            .map(|f| ReportFile {
                path: f.path,
                name: f.name,
                size_bytes: f.size_bytes,
                is_dir: f.is_dir,
            })
            .collect()
        }
        None => Vec::new(),
    };

    // Bounded tail (issue #710): the report agent summarizes trailing
    // error context, not the whole run log.
    let log_summary = match run.workdir.as_ref() {
        Some(wd) => checkpoint_status::read_tail_bounded(
            std::path::Path::new(&format!("{wd}/execution.log")),
            256 * 1024,
        ),
        None => String::new(),
    };

    // Failure context (issue #759): the report used to be status-blind —
    // a failed run's headline read "Pipeline completed" and Q&A answered
    // "No unexpected findings reported". Read the checkpoint directly
    // (not a hot path) for the structured failure data.
    let (run_status, failed_rules) = match run.workdir.as_deref() {
        Some(wd) => {
            let dir = std::path::Path::new(wd);
            let ck = oxo_flow_core::executor::CheckpointState::load_from_file(
                &oxo_flow_core::executor::CheckpointState::default_path(dir),
            )
            .ok();
            let failed = ck
                .as_ref()
                .map(|ck| {
                    ck.failed_rules
                        .iter()
                        // A stale failed entry whose when-verdict is false
                        // was gated off, not failed (#690) — not a failure
                        // to report.
                        .filter(|name| ck.when_verdicts.get(*name) != Some(&false))
                        .map(|name| {
                            let rec = ck.rule_runs.get(name);
                            crate::domains::ai::agents::types::FailedRuleInfo {
                                rule: name.clone(),
                                exit_code: rec.and_then(|r| r.exit_code),
                                stderr_tail: rec.and_then(|r| r.stderr_tail.clone()),
                                stdout_tail: rec.and_then(|r| r.stdout_tail.clone()),
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            (run.status.clone(), failed)
        }
        None => (run.status.clone(), Vec::new()),
    };

    // Wire the same diagnostics /api/runs/{id}/diagnostics computes
    // (issue #759): the OOM/137 finding branch in generate_report used to
    // be unreachable because this call site passed &[].
    let diagnostics: Vec<String> = match run.workdir.as_deref() {
        Some(wd) => {
            let dir = std::path::Path::new(wd);
            let node_items = checkpoint_status::load_node_statuses(dir, run.status == "running");
            let benchmarks = checkpoint_status::load_benchmarks(dir);
            let diag = service::diagnose_run(&node_items, &log_summary, &benchmarks);
            diag.failed_nodes
                .iter()
                .map(|n| {
                    format!(
                        "{}: {} — {}",
                        n.rule,
                        n.error_pattern
                            .clone()
                            .unwrap_or_else(|| "unknown_error".into()),
                        n.likely_cause
                    )
                })
                .chain(
                    diag.warnings
                        .iter()
                        .map(|w| format!("{}: {}", w.rule, w.suggestion)),
                )
                .collect()
        }
        None => Vec::new(),
    };

    report_agent::generate_report(
        &pipeline_name,
        &files,
        &log_summary,
        &diagnostics,
        &run_status,
        failed_rules,
    )
}

#[utoipa::path(
    get,
    path = "/api/runs/{id}/report",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 404, description = "Error", body = ApiError),
    )
)]
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

    let report = build_report_for_run(&run).await;

    Ok(Json(serde_json::json!(report)))
}

#[utoipa::path(
    post,
    path = "/api/runs/{id}/report/ask",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = String),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/runs/{id}/report/ask
pub async fn ask_report_question(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<serde_json::Value>,
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
    let report = build_report_for_run(&run).await;
    let answer = report_agent::answer_question(&report, question);
    Ok(Json(answer))
}

#[utoipa::path(
    post,
    path = "/api/runs/{id}/report/visualize",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/runs/{id}/report/visualize
pub async fn visualize_report(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<serde_json::Value>,
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
    require_run_mutator(&user)?;
    let run = load_owned_run(pool, &user, &id).await?;

    // Real data sources: the run's file tree, or per-rule timings from the
    // engine's checkpoint. No canned rows.
    let report = build_report_for_run(&run).await;
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

// ---------------------------------------------------------------------------
// CLI command exposure (issue #81): clean and checkpoint-resume were CLI
// high-frequency commands with no web equivalent.
// ---------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/api/runs/{id}/clean",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 404, description = "Error", body = ApiError),
    )
)]
/// POST /api/runs/{id}/clean — run the CLI's `clean` against the run's
/// workdir (chunk files + stale state). Quick, synchronous, output echoed.
pub async fn clean_run(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let user = current_user::resolve(authenticated.as_ref());
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;
    let run = load_owned_run(pool, &user, &id).await?;
    let workdir = run.workdir.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NO_WORKDIR",
            "Run has no workdir".into(),
        )
    })?;

    let bin = crate::executor::find_oxo_flow_binary();
    let workflow = std::path::Path::new(&workdir).join("workflow.oxoflow");
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("clean")
        .arg(&workflow)
        .arg("--workdir")
        .arg(&workdir)
        .arg("--force");
    let output = run_bounded(cmd, CLEAN_TIMEOUT, CLEAN_OUTPUT_CAP_BYTES)
        .await
        .map_err(|e| {
            tracing::error!("clean subprocess failed for run {id}: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "CLEAN_ERROR",
                "Failed to run clean".to_string(),
            )
        })?;
    Ok(Json(serde_json::json!({
        "run_id": id,
        "exit_code": output.status,
        "stdout": output.stdout.lines().last().unwrap_or(""),
        "stderr": output.stderr.lines().last().unwrap_or(""),
    })))
}

/// Hard limit on the `clean` subprocess. It walks and deletes a run workdir;
/// a stuck CLI (network filesystem, huge tree) must not hold the request —
/// and its connection — open forever.
const CLEAN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
/// Per-stream capture cap. The response only surfaces the last line, but the
/// pipes were previously unbounded (a chatty CLI could buffer gigabytes).
const CLEAN_OUTPUT_CAP_BYTES: u64 = 64 * 1024;

/// A finished subprocess with its captured (capped) output.
#[derive(Debug)]
struct BoundedOutput {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Read at most `cap` bytes into `sink`, then discard the remainder so a
/// child writing past the cap can never block on a full pipe.
async fn capture_capped<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    cap: u64,
    sink: &mut Vec<u8>,
) -> std::io::Result<()> {
    use tokio::io::AsyncReadExt;

    {
        let mut limited = reader.take(cap);
        limited.read_to_end(sink).await?;
    }
    tokio::io::copy(reader, &mut tokio::io::sink()).await?;
    Ok(())
}

/// Run `cmd` with a hard timeout and a per-stream byte cap.
///
/// Output past the cap is drained and discarded (memory stays bounded), so
/// the child still exits normally and its status is real; on timeout the
/// child is killed (`kill_on_drop`) and the call reports it.
async fn run_bounded(
    mut cmd: tokio::process::Command,
    timeout: std::time::Duration,
    cap: u64,
) -> Result<BoundedOutput, String> {
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("failed to start: {e}"))?;
    let mut stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| "no stdout pipe".to_string())?;
    let mut stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| "no stderr pipe".to_string())?;

    let run = async move {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let (out_res, err_res, status) = tokio::join!(
            capture_capped(&mut stdout_pipe, cap, &mut out),
            capture_capped(&mut stderr_pipe, cap, &mut err),
            child.wait(),
        );
        out_res.map_err(|e| format!("stdout read failed: {e}"))?;
        err_res.map_err(|e| format!("stderr read failed: {e}"))?;
        let status = status.map_err(|e| format!("wait failed: {e}"))?;
        Ok::<_, String>(BoundedOutput {
            status: status.code(),
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
        })
    };

    match tokio::time::timeout(timeout, run).await {
        Ok(result) => result,
        Err(_) => Err(format!(
            "timed out after {}s (process killed)",
            timeout.as_secs()
        )),
    }
}

/// Hard limit on the `verify-data` subprocess. Unlike `clean`, the audit
/// streams an md5 over EVERY file in the tree — a multi-GiB delivery on
/// spinning storage plausibly takes minutes; 10 minutes keeps the bound
/// generous without ever wedging a connection forever.
const VERIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);
/// Per-stream capture cap for `verify-data --json`. The JSON document is
/// one entry per file; 4 MiB covers ~30k entries and anything beyond it
/// means the caller staged a sidecar far too large for a synchronous
/// request anyway.
const VERIFY_OUTPUT_CAP_BYTES: u64 = 4 * 1024 * 1024;

#[utoipa::path(
    post,
    path = "/api/data/verify",
    tag = "data",
    request_body = VerifyDataRequest,
    responses(
        (status = 200, description = "Audit finished — findings are in the body", body = VerifyDataResponse),
        (status = 400, description = "Error", body = ApiError),
        (status = 500, description = "Error", body = ApiError),
    )
)]
/// POST /api/data/verify — audit a workspace-relative data tree against a
/// vendor md5 sidecar (issue #935 P2, web slice of the CLI's
/// `verify-data` command).
///
/// The tree is sandboxed to the acting user's workspace (#521 vocabulary —
/// same containment as `/api/data/analyze`); the sidecar arrives inline and
/// is staged to a temp file for the CLI subprocess. HTTP 200 carries the
/// audit findings (anomalies are data, not errors); 4xx/5xx is reserved for
/// bad requests and infrastructure failures.
pub async fn verify_data(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(req): ApiJson<VerifyDataRequest>,
) -> ApiResult<VerifyDataResponse> {
    let user = current_user::resolve(authenticated.as_ref());

    // Containment: the tree must resolve inside the acting user's
    // workspace root (absolute paths, `..`, trailing slash → 400).
    let tree_abs = crate::workspace::resolve_user_scoped_path(&user.id, &req.tree)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "INVALID_TREE", e))?;
    if !tree_abs.is_dir() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "INVALID_TREE",
            format!("tree '{}' does not exist or is not a directory", req.tree),
        ));
    }
    if req.md5_content.trim().is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "INVALID_SIDECAR",
            "md5_content is empty — provide the vendor sidecar text".into(),
        ));
    }

    // Stage the inline sidecar for the CLI (`--from` takes a path, not
    // stdin). The temp file lives OUTSIDE the tree, so the CLI's own
    // sidecar-self exclusion never even triggers.
    let sidecar_dir = tempfile::tempdir().map_err(|e| {
        tracing::error!("failed to stage verify-data sidecar tempdir: {e}");
        internal_verify_error("failed to stage sidecar".to_string())
    })?;
    let sidecar_path = sidecar_dir.path().join("md5.txt");
    tokio::fs::write(&sidecar_path, &req.md5_content)
        .await
        .map_err(|e| {
            tracing::error!("failed to write verify-data sidecar file: {e}");
            internal_verify_error("failed to stage sidecar".to_string())
        })?;

    let bin = crate::executor::find_oxo_flow_binary();
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("verify-data")
        .arg(&tree_abs)
        .arg("--from")
        .arg(&sidecar_path)
        .arg("--json");
    let output = run_bounded(cmd, VERIFY_TIMEOUT, VERIFY_OUTPUT_CAP_BYTES)
        .await
        .map_err(|e| {
            tracing::error!("verify-data subprocess failed: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "VERIFY_ERROR",
                "Failed to run verify-data".to_string(),
            )
        })?;

    // Exit policy mirrors the CLI: 0 = clean (or empty sidecar), 1 =
    // anomalies found (that is the audit's product, not a failure). Any
    // other exit is an infrastructure error with stderr context. The CLI
    // also exits 1 for hard input errors (e.g. a malformed sidecar line),
    // which it reports on stderr with an EMPTY stdout — distinguish that
    // from a findings document so a client-input error is not masked as a
    // 500 that invites a retry which can never succeed.
    let doc: serde_json::Value = match output.status {
        Some(1) if output.stdout.trim().is_empty() => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "INVALID_SIDECAR",
                format!(
                    "verify-data rejected the input: {}",
                    output.stderr.lines().last().unwrap_or("unknown error"),
                ),
            ));
        }
        Some(0) | Some(1) => serde_json::from_str(&output.stdout).map_err(|e| {
            tracing::error!(
                "verify-data produced unparseable JSON (exit {:?}): {e} — stderr: {}",
                output.status,
                output.stderr.lines().last().unwrap_or(""),
            );
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "VERIFY_ERROR",
                "verify-data produced unparseable JSON".to_string(),
            )
        })?,
        other => {
            tracing::error!(
                "verify-data exited unexpectedly ({other:?}): {}",
                output.stderr.lines().last().unwrap_or(""),
            );
            return Err(err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "VERIFY_ERROR",
                "verify-data exited unexpectedly".to_string(),
            ));
        }
    };

    // Shape-check the CLI document (args were ours, but the binary can be
    // a stale PATH fallback — degrade loudly, never lie to the caller).
    let entries: Vec<VerifyDataEntry> = doc
        .get("entries")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .ok_or_else(|| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "VERIFY_ERROR",
                "verify-data output missing 'entries' — is OXO_FLOW_BIN pointing at an up-to-date binary?"
                    .to_string(),
            )
        })?;
    let count = |key: &str| -> usize {
        doc.get(key)
            .and_then(|v| v.as_u64())
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(0)
    };
    let (matched, mismatched, missing, extra) = (
        count("matched"),
        count("mismatched"),
        count("missing"),
        count("extra"),
    );
    // The CLI guarantees every entry lands in exactly one bucket; a document
    // whose counts do not add up is a stale/incompatible binary, not an
    // audit result. Same degrade-loudly policy as the entries check above.
    if matched + mismatched + missing + extra != entries.len() {
        return Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "VERIFY_ERROR",
            format!(
                "verify-data counts (matched {matched} + mismatched {mismatched} + missing {missing} + extra {extra}) do not sum to entries.len() ({}) — is OXO_FLOW_BIN pointing at an up-to-date binary?",
                entries.len(),
            ),
        ));
    }
    let resp = VerifyDataResponse {
        tree: req.tree,
        matched,
        mismatched,
        missing,
        extra,
        entries,
        note: doc.get("note").and_then(|v| v.as_str()).map(String::from),
    };
    // Temp sidecar is dropped with `sidecar_dir` (it lives outside the
    // tree, so no cleanup inside it is needed).
    drop(sidecar_dir);
    Ok(Json(resp))
}

/// Internal-server-error helper for the verify-data handler (the two
/// error branches above share code/message shape).
fn internal_verify_error(message: String) -> (StatusCode, Json<ApiError>) {
    err(StatusCode::INTERNAL_SERVER_ERROR, "VERIFY_ERROR", message)
}

#[utoipa::path(
    get,
    path = "/api/runs/{id}/provenance-verify",
    tag = "runs",
    responses(
        (status = 200, description = "Verification finished — drift findings are in the body", body = ProvenanceVerifyResponse),
        (status = 400, description = "Error", body = ApiError),
        (status = 404, description = "Error", body = ApiError),
        (status = 500, description = "Error", body = ApiError),
        (status = 503, description = "Error", body = ApiError),
    )
)]
/// GET /api/runs/{id}/provenance-verify — re-hash a run's recorded output
/// checksums via the CLI's `provenance verify` (issue #276, web surface).
///
/// Read-only: the CLI subprocess is spawned with the run's checkpoint in
/// read mode only — it never mutates the checkpoint. The response mirrors
/// the CLI's `--json` document with entries flattened to the top level
/// (no `verify` wrapper); `ok` is
/// false exactly when the CLI would exit 1 (mismatched or missing > 0).
/// HTTP 200 carries drift findings — like verify-data, drift is data, not
/// an error; HTTP errors are reserved for a missing run, infrastructure
/// failures, and unparseable CLI output.
///
/// A run spawned without `provenance: true` has no recorded checksums — the
/// CLI reports that as a `note` (with `matched` = 0), which this endpoint
/// forwards verbatim.
pub async fn verify_run_provenance(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<ProvenanceVerifyResponse> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;
    let user = current_user::resolve(authenticated.as_ref());
    let run = load_owned_run(pool, &user, &id).await?;

    let workdir = run.workdir.clone().ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NO_WORKDIR",
            "Run has no workdir".into(),
        )
    })?;
    // Same default checkpoint path the CLI resolve logic uses when the
    // checkpoint argument is omitted (`CheckpointState::default_path`).
    let checkpoint = std::path::Path::new(&workdir)
        .join(".oxo-flow")
        .join("checkpoint.json");
    if !checkpoint.exists() {
        return Err(err(
            StatusCode::NOT_FOUND,
            "NO_CHECKPOINT",
            format!("No checkpoint at {}", checkpoint.display()),
        ));
    }

    // Top-level `--json` (a global flag before the subcommand) drives the
    // machine-readable document on stdout; the human report stays on
    // stderr. Checksums are re-hashed over every recorded output — the
    // same bounded-disk-work shape as verify-data, so reuse its timeout and
    // capture cap.
    let bin = crate::executor::find_oxo_flow_binary();
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("--json")
        .arg("provenance")
        .arg("verify")
        .arg(&checkpoint);
    let output = run_bounded(cmd, VERIFY_TIMEOUT, VERIFY_OUTPUT_CAP_BYTES)
        .await
        .map_err(|e| {
            tracing::error!("provenance verify subprocess failed: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "PROVENANCE_VERIFY_ERROR",
                "Failed to run provenance verify".to_string(),
            )
        })?;

    // Exit policy mirrors the CLI: 0 = all matched (or the empty-checksum
    // fallbacks, which print a note document and exit 0), 1 = drift found
    // (mismatched or missing > 0) — that is the finding, not a failure. The
    // CLI also exits 1 for unreadable/unparseable checkpoints with an EMPTY
    // stdout — map that to a 400 so a caller-input problem is not masked as
    // a retryable 500.
    let doc: serde_json::Value = match output.status {
        Some(1) if output.stdout.trim().is_empty() => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "INVALID_CHECKPOINT",
                format!(
                    "provenance verify rejected the checkpoint: {}",
                    output.stderr.lines().last().unwrap_or("unknown error"),
                ),
            ));
        }
        Some(0) | Some(1) => serde_json::from_str(&output.stdout).map_err(|e| {
            tracing::error!("provenance verify produced unparseable JSON: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "PROVENANCE_VERIFY_ERROR",
                "provenance verify produced unparseable JSON".to_string(),
            )
        })?,
        other => {
            tracing::error!(
                "provenance verify exited unexpectedly ({other:?}): {}",
                output.stderr.lines().last().unwrap_or(""),
            );
            return Err(err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "PROVENANCE_VERIFY_ERROR",
                "provenance verify exited unexpectedly".to_string(),
            ));
        }
    };

    // Shape-check the CLI document — entries are NESTED under "verify"
    // (unlike verify-data's flat document). Args were ours, but the binary
    // can be a stale PATH fallback — degrade loudly, never lie.
    let verify = doc.get("verify").ok_or_else(|| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "PROVENANCE_VERIFY_ERROR",
            "provenance verify output missing 'verify' section — is OXO_FLOW_BIN pointing at an up-to-date binary?"
                .to_string(),
        )
    })?;
    let entries: Vec<ProvenanceVerifyEntry> = verify
        .get("entries")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .ok_or_else(|| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "PROVENANCE_VERIFY_ERROR",
                "provenance verify output missing 'verify.entries' — is OXO_FLOW_BIN pointing at an up-to-date binary?"
                    .to_string(),
            )
        })?;
    let count = |key: &str| -> usize {
        verify
            .get(key)
            .and_then(|v| v.as_u64())
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(0)
    };
    let (matched, mismatched, missing, cleaned) = (
        count("matched"),
        count("mismatched"),
        count("missing"),
        count("cleaned"),
    );
    // The CLI guarantees every entry lands in exactly one bucket; a document
    // whose counts do not add up is a stale/incompatible binary, not an
    // audit result. Same degrade-loudly policy as the entries check above.
    if matched + mismatched + missing + cleaned != entries.len() {
        return Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "PROVENANCE_VERIFY_ERROR",
            format!(
                "provenance verify counts (matched {matched} + mismatched {mismatched} + missing {missing} + cleaned {cleaned}) do not sum to entries.len() ({}) — is OXO_FLOW_BIN pointing at an up-to-date binary?",
                entries.len(),
            ),
        ));
    }
    let note = verify
        .get("note")
        .and_then(|v| v.as_str())
        .map(String::from);
    Ok(Json(ProvenanceVerifyResponse {
        checkpoint: checkpoint.display().to_string(),
        matched,
        mismatched,
        missing,
        cleaned,
        entries,
        note,
        // Mirror the CLI exit policy: drift found = not ok.
        ok: mismatched == 0 && missing == 0,
    }))
}

#[utoipa::path(
    post,
    path = "/api/runs/{id}/resume-checkpoint",
    tag = "runs",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/runs/{id}/resume-checkpoint — resume an unfinished run from
/// its checkpoint via the CLI's `resume` command, recorded as a NEW run
/// (the same pattern as retry).
pub async fn resume_checkpoint(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    let user = current_user::resolve(authenticated.as_ref());
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;
    require_run_mutator(&user)?;
    let run = load_owned_run(pool, &user, &id).await?;

    // A checkpoint resume spawns a REAL run — same budgets as create_run
    // and retry (#519).
    let quota_usage = quota_usage_for(&run.pipeline_snapshot);
    check_spawn_budget(&user.id, quota_usage)?;

    // The resume spawns a second CLI into the SAME workdir; an active run
    // would put two CLIs on one checkpoint concurrently — the exact hazard
    // retry_run guards against. Only terminal runs can be resumed
    // (audit #661).
    if matches!(run.status.as_str(), "running" | "queued" | "paused") {
        return Err(err(
            StatusCode::CONFLICT,
            "RUN_NOT_TERMINAL",
            format!(
                "Run {id} is still {} — wait for it to finish (or cancel it) before resuming from its checkpoint",
                run.status
            ),
        ));
    }

    let workdir = run.workdir.clone().ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NO_WORKDIR",
            "Run has no workdir".into(),
        )
    })?;

    let checkpoint = std::path::Path::new(&workdir)
        .join(".oxo-flow")
        .join("checkpoint.json");
    if !checkpoint.exists() {
        return Err(err(
            StatusCode::NOT_FOUND,
            "NO_CHECKPOINT",
            format!("No checkpoint at {}", checkpoint.display()),
        ));
    }
    let jobs = req.get("max_jobs").and_then(|v| v.as_u64()).unwrap_or(1) as usize;

    // New run row; same workdir so the resume continues in place.
    let new_run_id = uuid::Uuid::new_v4().to_string();
    let now = now_iso();
    let new_run = models::RunRow {
        id: new_run_id.clone(),
        user_id: user.id.clone(),
        pipeline_id: run.pipeline_id.clone(),
        pipeline_snapshot: run.pipeline_snapshot.clone(),
        workflow_name: run.workflow_name.clone(),
        status: "queued".to_string(),
        phase: "resuming".to_string(),
        dry_run: run.dry_run,
        pid: None,
        workdir: Some(workdir.clone()),
        // The resume subcommand has no --arg passthrough surface in
        // spawn_background_run_with_args; the checkpointed workflow in the
        // workdir already carries the original run's baked configuration.
        config_overrides: None,
        started_at: None,
        finished_at: None,
        created_at: now.clone(),
    };
    sqlx::query(
        "INSERT OR IGNORE INTO runs (id, user_id, pipeline_id, pipeline_snapshot, workflow_name, status, phase, dry_run, pid, workdir, config_overrides, started_at, finished_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&new_run.id)
    .bind(&new_run.user_id)
    .bind(&new_run.pipeline_id)
    .bind(&new_run.pipeline_snapshot)
    .bind(&new_run.workflow_name)
    .bind(&new_run.status)
    .bind(&new_run.phase)
    .bind(new_run.dry_run)
    .bind(new_run.pid)
    .bind(&new_run.workdir)
    .bind(&new_run.config_overrides)
    .bind(&new_run.started_at)
    .bind(&new_run.finished_at)
    .bind(&new_run.created_at)
    .execute(pool)
    .await
    .map_err(|e| {
        tracing::error!("DB error inserting resume run: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".to_string(),        )
    })?;

    let args: Vec<std::ffi::OsString> = vec![
        "resume".into(),
        checkpoint.into_os_string(),
        "--workdir".into(),
        workdir.clone().into(),
        "-j".into(),
        jobs.to_string().into(),
    ];
    crate::infra::quota::global_quota_tracker().record_start(
        &user.id,
        quota_usage.0,
        quota_usage.1,
    );
    crate::infra::quota::reserve(&new_run_id, &user.id, quota_usage.0, quota_usage.1);
    crate::executor::spawn_background_run_with_args(
        new_run_id.clone(),
        user.id.clone(),
        "none".to_string(),
        "local".to_string(),
        Some(std::path::PathBuf::from(workdir)),
        args,
    );

    Ok(Json(serde_json::json!({
        "run_id": new_run_id,
        "resumed_from": id,
        "max_jobs": jobs,
    })))
}

#[cfg(test)]
mod bounded_subprocess_tests {
    use super::*;

    fn sh(script: &str) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(script);
        cmd
    }

    /// Output past the cap must not deadlock the wait: the pipes are closed
    /// once the cap is hit, so the writer gets EPIPE and the status arrives.
    #[tokio::test]
    async fn output_is_capped_without_hanging() {
        let out = run_bounded(
            sh("head -c 200000 /dev/zero | tr '\\0' 'x'"),
            std::time::Duration::from_secs(30),
            1024,
        )
        .await
        .expect("bounded run succeeds");
        assert_eq!(out.status, Some(0));
        assert_eq!(
            out.stdout.len(),
            1024,
            "stdout capture must stop at the cap"
        );
    }

    /// A hung subprocess is killed and reported instead of holding the
    /// request open forever.
    #[tokio::test]
    async fn hung_subprocess_is_killed_and_reported() {
        let err = run_bounded(sh("sleep 30"), std::time::Duration::from_millis(200), 1024)
            .await
            .expect_err("timeout must be an error");
        assert!(err.contains("timed out"), "unexpected error: {err}");
    }

    /// Wire-level argv test for `POST /api/data/verify` (issue #935 P2):
    /// the exact CLI invocation the handler must produce, exercised
    /// against a real tree + staged sidecar. Mirrors the handler's
    /// construction: `verify-data <tree> --from <sidecar> --json`.
    #[tokio::test]
    async fn verify_data_argv_produces_json_document() {
        let tmp = tempfile::tempdir().unwrap();
        let tree = tmp.path().join("delivery");
        std::fs::create_dir_all(tree.join("raw")).unwrap();
        std::fs::write(tree.join("raw/S1.fq"), b"AAAA").unwrap();
        std::fs::write(tree.join("raw/STRAY.txt"), b"stray").unwrap();

        // Stage the sidecar OUTSIDE the tree, exactly like the handler.
        let sidecar_dir = tempfile::tempdir().unwrap();
        let sidecar = sidecar_dir.path().join("md5.txt");
        let digest = {
            use md5::{Digest, Md5};
            let mut h = Md5::new();
            h.update(b"AAAA");
            hex::encode(h.finalize())
        };
        std::fs::write(&sidecar, format!("{digest}  raw/S1.fq\n")).unwrap();

        let mut cmd = tokio::process::Command::new(crate::executor::find_oxo_flow_binary());
        cmd.arg("verify-data")
            .arg(&tree)
            .arg("--from")
            .arg(&sidecar)
            .arg("--json");
        let out = run_bounded(cmd, std::time::Duration::from_secs(120), 1024 * 1024)
            .await
            .expect("verify-data runs");

        // 1 matched + 1 extra → exit 1 (findings, not failure).
        assert_eq!(out.status, Some(1), "stderr: {}", out.stderr);
        let doc: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(doc["matched"], 1);
        assert_eq!(doc["extra"], 1);
        assert_eq!(doc["entries"].as_array().unwrap().len(), 2);
        assert_eq!(doc["entries"][0]["file"], "raw/S1.fq");
        assert_eq!(doc["entries"][0]["status"], "matched");
        assert_eq!(doc["entries"][1]["status"], "extra");
    }

    /// The handler maps a hard CLI rejection (exit 1, empty stdout, error on
    /// stderr — e.g. a malformed sidecar line) to 400 INVALID_SIDECAR, not a
    /// 500: this test pins the stdout-empty signal the handler keys on.
    #[tokio::test]
    async fn verify_data_malformed_sidecar_exits_1_with_empty_stdout() {
        let tmp = tempfile::tempdir().unwrap();
        let tree = tmp.path().join("delivery");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join("S1.fq"), b"AAAA").unwrap();

        let sidecar_dir = tempfile::tempdir().unwrap();
        let sidecar = sidecar_dir.path().join("md5.txt");
        std::fs::write(&sidecar, "not-a-digest  S1.fq\n").unwrap();

        let mut cmd = tokio::process::Command::new(crate::executor::find_oxo_flow_binary());
        cmd.arg("verify-data")
            .arg(&tree)
            .arg("--from")
            .arg(&sidecar)
            .arg("--json");
        let out = run_bounded(cmd, std::time::Duration::from_secs(120), 1024 * 1024)
            .await
            .expect("verify-data runs");

        assert_eq!(out.status, Some(1), "stderr: {}", out.stderr);
        assert!(
            out.stdout.trim().is_empty(),
            "hard input errors print nothing on stdout, got: {}",
            out.stdout
        );
        assert!(
            out.stderr.contains("malformed"),
            "stderr must name the sidecar problem: {}",
            out.stderr
        );
    }

    /// Wire-level test for `GET /api/runs/{id}/protected-drift` (issue #935
    /// slice 3): the core diff machinery the handler drives, exercised
    /// against a real tree + checkpoint, asserting the CLI-mirroring
    /// contract — first observation counts as new_trees (not drift), later
    /// mutations surface as one `describe()` line each, and NOTHING is
    /// persisted (read-only like dry-run).
    #[test]
    fn protected_drift_preview_is_read_only_and_mirrors_cli() {
        use oxo_flow_core::executor::CheckpointState;
        use oxo_flow_core::executor::checkpoint as ckpt;

        let tmp = tempfile::tempdir().unwrap();
        let workdir = tmp.path().join("workdir");
        std::fs::create_dir_all(workdir.join("raw")).unwrap();
        std::fs::write(workdir.join("raw/S1.fq"), b"AAAA").unwrap();
        std::fs::write(workdir.join("raw/S2.fq"), b"BBBB").unwrap();

        // No checkpoint on disk → baselines map empty: first observation is
        // reported as new_trees, never as drift.
        let ck = CheckpointState::load_from_file(&CheckpointState::default_path(&workdir))
            .unwrap_or_else(|_| CheckpointState::new());
        assert!(
            ck.protected_tree_baselines.is_empty(),
            "absent checkpoint = no baselines"
        );

        let current = ckpt::snapshot_protected_tree(&workdir.join("raw"), &workdir)
            .expect("tree readable")
            .expect("tree exists");
        assert!(
            !ck.protected_tree_baselines.contains_key("raw"),
            "no baseline yet → preview reports new_trees, not drift"
        );
        assert!(ckpt::manifests_match(&current, &current));

        // Seed a baseline (what a prior run would have persisted) and
        // confirm the diff path surfaces truncation + removal.
        let mut ck2 = CheckpointState::new();
        ck2.record_protected_tree_baseline("raw", current);
        std::fs::write(workdir.join("raw/S1.fq"), b"").unwrap();
        std::fs::remove_file(workdir.join("raw/S2.fq")).unwrap();

        let recorded = ck2.protected_tree_baselines.get("raw").unwrap();
        assert!(!ckpt::manifests_match(
            recorded,
            &ckpt::snapshot_protected_tree(&workdir.join("raw"), &workdir)
                .unwrap()
                .unwrap()
        ));
        let lines: Vec<String> = ckpt::manifest_changes(
            recorded,
            &ckpt::snapshot_protected_tree(&workdir.join("raw"), &workdir)
                .unwrap()
                .unwrap(),
        )
        .iter()
        .map(|c| c.describe())
        .collect();
        assert_eq!(lines.len(), 2, "one line per drifted file: {lines:?}");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("S1.fq") && l.contains("(TRUNCATED)"))
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("S2.fq") && l.contains("removed"))
        );

        // The read-only contract: no checkpoint file was created by any of
        // the calls above (the run gate, not the preview, persists).
        assert!(
            !CheckpointState::default_path(&workdir).exists(),
            "preview must not persist baselines"
        );
    }
}
