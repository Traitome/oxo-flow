//! Terminal hooks and --json run summary: run_terminal_hook, notify_webhook, RunCounts/RunJsonSummary + Drop emission (#948 P2-5).

use super::*;

/// Run the workflow-level terminal hook (`on_complete` after a fully
/// successful run, `on_error` after any failure) on every terminal path of
/// the run loop. Rendered with `{config.*}` plus the run counters and
/// `{workdir}`; best-effort by design — a failing hook warns, never
/// changes the run status (issue #227 item 1).
pub(crate) async fn run_terminal_hook(
    config: &WorkflowConfig,
    workdir: &std::path::Path,
    tmp_dir: Option<&std::path::Path>,
    succeeded: usize,
    failed: usize,
    skipped: usize,
) {
    let hook_cmd = if failed > 0 {
        config.workflow.on_error.as_deref()
    } else {
        config.workflow.on_complete.as_deref()
    };
    let Some(hook_cmd) = hook_cmd else {
        return;
    };
    let mut values = config.config_placeholder_values();
    values.insert("succeeded".to_string(), succeeded.to_string());
    values.insert("failed".to_string(), failed.to_string());
    values.insert("skipped".to_string(), skipped.to_string());
    values.insert("workdir".to_string(), workdir.display().to_string());
    let rendered = oxo_flow_core::executor::process::expand_placeholders(hook_cmd, &values);
    // Rule placeholders are not substituted here — only `[config]`, the
    // counters, and `{workdir}` (issue #1078 M6). A surviving engine token
    // would execute literally; warn without changing the run status.
    let scanned = oxo_flow_core::wildcard::strip_shell_comments(&rendered);
    let survivors = oxo_flow_core::executor::placeholder_gate::stale_placeholders(&scanned);
    if !survivors.is_empty() {
        let tokens = survivors
            .iter()
            .map(|stale| stale.token.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        tracing::warn!(
            hook = if failed > 0 { "on_error" } else { "on_complete" },
            placeholders = %tokens,
            "workflow-level hook still carries engine placeholder(s) — only [config] keys, the \
             run counters, and {{workdir}} are substituted; the shell receives the literal token"
        );
    }
    oxo_flow_core::executor::process::run_workflow_hook(
        &rendered,
        workdir,
        config.defaults.shell_prelude.as_deref(),
        tmp_dir,
    )
    .await;
}

/// Workflow-level webhook notification (issue #227 item 1): posts the
/// `workflow_started` / `workflow_completed` / `workflow_failed` event to
/// the `[webhook]` endpoint. Best-effort by design — a failing endpoint
/// logs a warning and never changes the run status. Awaited (not spawned)
/// so the process cannot exit before the request lands; the client's own
/// timeout bounds the wait.
pub(crate) async fn notify_webhook(
    config: &WorkflowConfig,
    event: oxo_flow_core::webhook::WebhookEvent,
    counts: Option<(usize, usize, usize)>,
) {
    let Some(ref webhook) = config.webhook else {
        return;
    };
    let (succeeded, failed, skipped) = counts.unwrap_or_default();
    let payload = oxo_flow_core::webhook::WebhookPayload {
        event,
        workflow_name: config.workflow.name.clone(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        data: oxo_flow_core::webhook::WebhookData {
            total_rules: None,
            succeeded: Some(succeeded),
            failed: Some(failed),
            skipped: Some(skipped),
            duration_ms: None,
            rule: None,
            exit_code: None,
            error: if failed > 0 {
                Some(oxo_flow_core::plural(failed, "rule failed", "rules failed"))
            } else {
                None
            },
        },
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let client = oxo_flow_core::webhook::WebhookClient::new(webhook.clone());
    if let Err(e) = client.send(&payload).await {
        tracing::warn!(error = %e, event = %event, "webhook notification failed (run continues)");
    }
}

/// Rule-level webhook notification: posts `rule_completed` / `rule_failed`
/// to the `[webhook]` endpoint as each scheduled rule reaches a terminal
/// state, carrying the rule name, exit code, and masked error text. Same
/// best-effort contract as [`notify_webhook`] — a failing endpoint logs a
/// warning and never changes the run status. Subscription filtering lives
/// in the client (`WebhookConfig.events`), so configs that only subscribe
/// to workflow-level events stay silent here with zero network traffic.
pub(crate) async fn notify_webhook_rule(
    webhook: Option<&oxo_flow_core::webhook::WebhookConfig>,
    workflow_name: &str,
    event: oxo_flow_core::webhook::WebhookEvent,
    rule: &str,
    exit_code: Option<i32>,
    error: Option<String>,
) {
    let Some(webhook) = webhook else {
        return;
    };
    let payload = oxo_flow_core::webhook::WebhookPayload {
        event,
        workflow_name: workflow_name.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        data: oxo_flow_core::webhook::WebhookData {
            rule: Some(rule.to_string()),
            exit_code,
            error,
            ..Default::default()
        },
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let client = oxo_flow_core::webhook::WebhookClient::new(webhook.clone());
    if let Err(e) = client.send(&payload).await {
        tracing::warn!(error = %e, event = %event, "webhook notification failed (run continues)");
    }
}

/// Terminal run counters shared by the `--json` summary and the workflow
/// terminal hooks. Grouped so the summary emitter stays under clippy's
/// argument-count lint.
#[derive(Default)]
pub(crate) struct RunCounts {
    pub(crate) succeeded: usize,
    pub(crate) skipped: usize,
    pub(crate) failed: usize,
    pub(crate) non_required_failed: usize,
    pub(crate) blocked: usize,
}

/// AI-repair loop outcome for the `--json` summary (issue #1164): set when
/// the failure handler applied a fix and attempted the verification rerun.
#[derive(Clone)]
pub(crate) struct RunRepairSummary {
    pub(crate) applied: bool,
    pub(crate) session_id: String,
    pub(crate) rerun_attempted: bool,
    pub(crate) rerun_succeeded: bool,
}

/// The `--json` summary for one `run`/`resume` invocation, emitted at most
/// once in the canonical `{"command":"run",...}` shape.
///
/// The document is documented as appearing on EVERY exit path
/// (docs/guide/src/commands/run.md): the explicit [`Self::emit`] calls carry
/// the terminal counts, and the `Drop` fallback covers the early aborts that
/// never reach one (workflow parse errors, preflight failures, repository
/// checkout errors) — those used to exit non-zero with zero bytes on stdout.
pub(crate) struct RunJsonSummary {
    json: bool,
    workflow: PathBuf,
    repair: Option<RunRepairSummary>,
    /// Enforced cost-budget detail (issue #1400), present only when the cost
    /// gate fired. Left unset on every other run so those `--json` documents
    /// stay byte-identical to a run without `[resource_budget] max_cost`.
    cost_budget: Option<serde_json::Value>,
    emitted: bool,
}

impl RunJsonSummary {
    pub(crate) fn new(json: bool, workflow: Option<&Path>) -> Self {
        Self {
            json,
            // Until the workflow is resolved the CLI argument is the best
            // name available; `set_workflow` replaces it with the resolved
            // path (a repository spec resolves to a checkout path).
            workflow: workflow.unwrap_or_else(|| Path::new("")).to_path_buf(),
            repair: None,
            cost_budget: None,
            emitted: false,
        }
    }

    /// Point the summary at the resolved workflow file.
    pub(crate) fn set_workflow(&mut self, workflow: &Path) {
        self.workflow = workflow.to_path_buf();
    }

    /// Attach the AI-repair loop outcome (issue #1164); emitted as the
    /// `repair` key of the summary document when present.
    pub(crate) fn set_repair(&mut self, repair: RunRepairSummary) {
        self.repair = Some(repair);
    }

    /// Attach the enforced cost-budget detail (issue #1400). Emitted as the
    /// `cost_budget` key only when set, so a run without `[resource_budget]`
    /// `max_cost` produces byte-identical JSON to a pre-#1400 build.
    pub(crate) fn set_cost_budget(&mut self, detail: serde_json::Value) {
        self.cost_budget = Some(detail);
    }

    pub(crate) fn emit(
        &mut self,
        status: &str,
        counts: &RunCounts,
        resources: Vec<serde_json::Value>,
    ) {
        if !self.json || self.emitted {
            return;
        }
        self.emitted = true;
        let mut output = serde_json::json!({
            "command": "run",
            "status": status,
            "workflow": self.workflow.to_string_lossy(),
            "results": serde_json::json!({
                "succeeded": counts.succeeded,
                "skipped": counts.skipped,
                "failed": counts.failed,
                "non_required_failed": counts.non_required_failed,
                "blocked": counts.blocked,
            }),
            "repair": self.repair.as_ref().map(|r| serde_json::json!({
                "applied": r.applied,
                "session_id": r.session_id,
                "rerun_attempted": r.rerun_attempted,
                "rerun_succeeded": r.rerun_succeeded,
            })),
            "resources": resources,
        });
        // The cost-budget detail is added ONLY when the gate fired, so a run
        // without `[resource_budget] max_cost` keeps its exact JSON bytes.
        if let Some(detail) = self.cost_budget.take()
            && let Some(obj) = output.as_object_mut()
        {
            obj.insert("cost_budget".to_string(), detail);
        }
        match serde_json::to_string_pretty(&output) {
            Ok(json) => println!("{}", json),
            Err(error) => {
                eprintln!("failed to pretty-print run JSON summary: {error}");
                println!("{}", output);
            }
        }
    }
}

impl Drop for RunJsonSummary {
    fn drop(&mut self) {
        // Only the failure paths return without an explicit emission, so an
        // abort that never reached one still reports `status: "failed"`.
        self.emit("failed", &RunCounts::default(), vec![]);
    }
}
