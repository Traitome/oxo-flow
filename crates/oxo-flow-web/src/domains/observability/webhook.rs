//! Webhook notifications for run completion (issue #82 P1-12): the core
//! WebhookClient (HMAC-signed payloads) fires on terminal run states when
//! an admin has configured an endpoint. The web crate previously shipped
//! zero webhook references despite the feature being documented.

use oxo_flow_core::webhook::{
    HttpMethod, SignatureScheme, WebhookClient, WebhookConfig, WebhookEvent, WebhookPayload,
};

/// Secure default for newly configured web endpoints.
pub const DEFAULT_SIGNATURE_SCHEME: &str = "hmac-sha256";

/// Frozen compatibility scheme used by pre-existing webhook rows.
pub const LEGACY_SIGNATURE_SCHEME: &str = "sha256-keyed";

/// Parse a persisted/API signature scheme without silently coercing typos.
#[must_use]
pub fn signature_scheme(value: &str) -> Option<SignatureScheme> {
    match value.trim() {
        "hmac-sha256" => Some(SignatureScheme::HmacSha256),
        "sha256-keyed" => Some(SignatureScheme::Sha256Keyed),
        _ => None,
    }
}

/// Return the canonical stored spelling accepted by this API.
#[must_use]
pub fn canonical_signature_scheme(value: &str) -> Option<&'static str> {
    match signature_scheme(value)? {
        SignatureScheme::HmacSha256 => Some(DEFAULT_SIGNATURE_SCHEME),
        SignatureScheme::Sha256Keyed => Some(LEGACY_SIGNATURE_SCHEME),
    }
}

/// One config row (id=1) persisted by PUT /api/webhook.
pub struct WebhookSettings {
    pub enabled: bool,
    pub url: String,
    pub secret: Option<String>,
    pub events: Vec<String>,
    /// "hmac-sha256" or the frozen legacy "sha256-keyed".
    pub signature_scheme: String,
}

impl Default for WebhookSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            secret: None,
            events: Vec::new(),
            signature_scheme: DEFAULT_SIGNATURE_SCHEME.into(),
        }
    }
}

pub async fn load_settings() -> Option<WebhookSettings> {
    // The trait is the only read path — `global_backend()` itself falls back
    // to the SQLite pool, so the raw-SQL twin that lived below this comment
    // was unreachable duplicate semantics (the StorageBackend docs forbid
    // adding raw-SQL paths beside it).
    let backend = crate::infra::db::global_backend()?;
    let row = backend
        .get_webhook_config()
        .await
        .map_err(|e| tracing::warn!("webhook config read failed: {e}"))
        .ok()??;
    let events: Vec<String> = serde_json::from_str(&row.events).unwrap_or_default();
    Some(WebhookSettings {
        url: row.url,
        // `open` transparently passes legacy plaintext through (pre-sealing
        // rows, or deployments without OXO_FLOW_MASTER_KEY).
        secret: row.secret.map(|s| crate::infra::crypto::open(&s)),
        enabled: row.enabled != 0,
        events,
        signature_scheme: row.signature_scheme,
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

    let backend = match crate::infra::db::global_backend() {
        Some(b) => b,
        None => return,
    };
    let run = match backend.get_run(run_id).await {
        Ok(Some(r)) => r,
        _ => return,
    };
    let workflow_name = run.workflow_name;
    let started_at = run.started_at;
    let finished_at = run.finished_at;
    let workdir = run.workdir;
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

    let Some(scheme) = signature_scheme(&settings.signature_scheme) else {
        tracing::error!(
            "webhook delivery disabled for run {run_id}: unsupported signature scheme '{}'",
            settings.signature_scheme
        );
        return;
    };
    let config = WebhookConfig {
        url: settings.url,
        method: HttpMethod::Post,
        headers: Default::default(),
        events: vec![event],
        secret: settings.secret,
        signature_scheme: scheme,
        timeout_secs: 10,
        max_retries: 1,
    };
    let client = WebhookClient::new(config);
    if let Err(e) = client.send(&payload).await {
        tracing::warn!("webhook delivery failed for run {run_id}: {e}");
    }
}
