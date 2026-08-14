//! Webhook notifications for run completion (issue #82 P1-12): the core
//! WebhookClient (HMAC-signed payloads) fires on terminal run states when
//! an admin has configured an endpoint. The web crate previously shipped
//! zero webhook references despite the feature being documented.

use oxo_flow_core::webhook::{
    HttpMethod, SignatureScheme, WebhookClient, WebhookConfig, WebhookEvent, WebhookPayload,
};

/// One config row (id=1) persisted by PUT /api/webhook.
pub struct WebhookSettings {
    pub enabled: bool,
    pub url: String,
    pub secret: Option<String>,
    pub events: Vec<String>,
}

pub async fn load_settings() -> Option<WebhookSettings> {
    let pool = crate::infra::db::sqlite::try_pool().ok()?;
    let row = sqlx::query_as::<_, (String, Option<String>, i64, String)>(
        "SELECT url, secret, enabled, events FROM webhook_config WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()?;
    let events: Vec<String> = serde_json::from_str(&row.3).unwrap_or_default();
    Some(WebhookSettings {
        url: row.0,
        secret: row.1,
        enabled: row.2 != 0,
        events,
    })
}

/// Fire the configured webhook for a terminal run state. Best-effort —
/// notification failures must never affect the run record itself.
pub async fn notify_terminal(run_id: &str, final_state: &str) {
    let Some(settings) = load_settings().await else {
        return;
    };
    if !settings.enabled || settings.url.trim().is_empty() {
        return;
    }
    // Both event spellings are accepted for compatibility with the CLI's
    // config vocabulary; either terminal state notifies.
    if !settings.events.is_empty()
        && !settings
            .events
            .iter()
            .any(|e| e == "workflow_completed" || e == "workflow_failed")
    {
        return;
    }
    let event = if final_state == "completed" {
        WebhookEvent::WorkflowCompleted
    } else {
        WebhookEvent::WorkflowFailed
    };

    let pool = match crate::infra::db::sqlite::try_pool() {
        Ok(p) => p,
        Err(_) => return,
    };
    let row: Option<(
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT workflow_name, started_at, finished_at, workdir FROM runs WHERE id = ?",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await
    .unwrap_or(None);
    let Some((workflow_name, started_at, finished_at, workdir)) = row else {
        return;
    };
    let workflow_name = workflow_name.unwrap_or_else(|| "pipeline".to_string());

    // Rule counts from the engine's checkpoint (single source of truth).
    let nodes = crate::domains::execution::checkpoint_status::load_node_statuses(
        std::path::Path::new(workdir.as_deref().unwrap_or("")),
        false,
    );
    let succeeded = nodes
        .iter()
        .filter(|n| n.status == crate::domains::execution::types::NodeStatus::Success)
        .count();
    let skipped = nodes
        .iter()
        .filter(|n| n.status == crate::domains::execution::types::NodeStatus::Skipped)
        .count();
    let failed = nodes
        .iter()
        .filter(|n| n.status == crate::domains::execution::types::NodeStatus::Failed)
        .count();
    let duration_ms = match (&started_at, &finished_at) {
        (Some(s), Some(f)) => {
            let start = chrono::DateTime::parse_from_rfc3339(s).ok();
            let end = chrono::DateTime::parse_from_rfc3339(f).ok();
            match (start, end) {
                (Some(a), Some(b)) => Some((b - a).num_milliseconds().max(0) as u64),
                _ => None,
            }
        }
        _ => None,
    };

    let payload = WebhookPayload {
        event,
        workflow_name,
        timestamp: chrono::Utc::now().to_rfc3339(),
        data: oxo_flow_core::webhook::WebhookData {
            total_rules: Some(nodes.len()),
            succeeded: Some(succeeded),
            failed: Some(failed),
            skipped: Some(skipped),
            duration_ms,
            rule: None,
            exit_code: None,
            error: if final_state == "failed" {
                Some(format!("Run {run_id} failed"))
            } else {
                None
            },
        },
        version: env!("CARGO_PKG_VERSION").to_string(),
    };

    let config = WebhookConfig {
        url: settings.url,
        method: HttpMethod::Post,
        headers: Default::default(),
        events: vec![event],
        secret: settings.secret,
        signature_scheme: SignatureScheme::HmacSha256,
        timeout_secs: 10,
        max_retries: 1,
    };
    let client = WebhookClient::new(config);
    if let Err(e) = client.send(&payload).await {
        tracing::warn!("webhook delivery failed for run {run_id}: {e}");
    }
}
