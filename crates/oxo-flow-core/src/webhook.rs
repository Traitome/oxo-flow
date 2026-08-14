//! Webhook support for external notifications.
//!
//! Enables sending notifications to external services (Slack, Discord, custom endpoints)
//! when workflow execution events occur.

use crate::error::{OxoFlowError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Webhook configuration for external notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Webhook endpoint URL.
    pub url: String,

    /// HTTP method (POST, PUT, GET).
    #[serde(default = "default_method")]
    pub method: HttpMethod,

    /// Custom headers to include in the request.
    #[serde(default)]
    pub headers: HashMap<String, String>,

    /// Events to trigger webhook (workflow_completed, rule_completed, on_failure).
    #[serde(default = "default_events")]
    pub events: Vec<WebhookEvent>,

    /// Secret key for HMAC signature (optional, for verification).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,

    /// Signature scheme for the `X-OxoFlow-Signature` header.
    ///
    /// Defaults to RFC-2104 HMAC-SHA256 (`hmac-sha256=<hex>`). The legacy
    /// `sha256-keyed` scheme (`sha256=<hex(sha256(secret‖body))>`) predates
    /// the standard and is frozen for existing consumers (issue #67 §4);
    /// it will be removed in a future major version.
    #[serde(default)]
    pub signature_scheme: SignatureScheme,

    /// Timeout for webhook request in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    /// Maximum retries on failure.
    #[serde(default = "default_retries")]
    pub max_retries: u32,
}

fn default_method() -> HttpMethod {
    HttpMethod::Post
}

fn default_events() -> Vec<WebhookEvent> {
    vec![WebhookEvent::WorkflowCompleted]
}

fn default_timeout() -> u64 {
    30
}

fn default_retries() -> u32 {
    3
}

/// Signature scheme for the `X-OxoFlow-Signature` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SignatureScheme {
    /// RFC-2104 HMAC-SHA256 over the payload body (the default).
    #[default]
    HmacSha256,
    /// Legacy keyed SHA-256 (`sha256(secret‖body)`); frozen, deprecated.
    Sha256Keyed,
}

/// HTTP method for webhook requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HttpMethod {
    Post,
    Put,
    Get,
}

/// Events that can trigger webhook notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookEvent {
    /// Workflow started.
    WorkflowStarted,
    /// Workflow completed (success or failure).
    WorkflowCompleted,
    /// Workflow failed.
    WorkflowFailed,
    /// Rule completed.
    RuleCompleted,
    /// Rule failed.
    RuleFailed,
}

impl std::fmt::Display for WebhookEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WebhookEvent::WorkflowStarted => write!(f, "workflow_started"),
            WebhookEvent::WorkflowCompleted => write!(f, "workflow_completed"),
            WebhookEvent::WorkflowFailed => write!(f, "workflow_failed"),
            WebhookEvent::RuleCompleted => write!(f, "rule_completed"),
            WebhookEvent::RuleFailed => write!(f, "rule_failed"),
        }
    }
}

/// Payload sent to webhook endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookPayload {
    /// Event type.
    pub event: WebhookEvent,

    /// Workflow name.
    pub workflow_name: String,

    /// Timestamp (ISO 8601).
    pub timestamp: String,

    /// Event-specific data.
    pub data: WebhookData,

    /// oxo-flow version.
    #[serde(default = "default_version")]
    pub version: String,
}

fn default_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Event-specific data payload.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebhookData {
    /// Total rules in workflow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_rules: Option<usize>,

    /// Succeeded rule count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub succeeded: Option<usize>,

    /// Failed rule count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<usize>,

    /// Skipped rule count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<usize>,

    /// Duration in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,

    /// Rule name (for rule events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,

    /// Exit code (for rule events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,

    /// Error message (for failure events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Webhook client for sending notifications.
pub struct WebhookClient {
    config: WebhookConfig,
}

impl WebhookClient {
    /// Create a new webhook client.
    pub fn new(config: WebhookConfig) -> Self {
        Self { config }
    }

    /// Send webhook notification.
    pub async fn send(&self, payload: &WebhookPayload) -> Result<()> {
        if !self.config.events.contains(&payload.event) {
            return Ok(()); // Event not configured, skip
        }

        let body = serde_json::to_string(payload).map_err(|e| OxoFlowError::Validation {
            message: format!("failed to serialize webhook payload: {}", e),
            rule: None,
            suggestion: None,
        })?;

        let mut retries = 0;
        while retries <= self.config.max_retries {
            let result = self.send_request(&body).await;
            match result {
                Ok(_) => return Ok(()),
                Err(e) if retries < self.config.max_retries => {
                    retries += 1;
                    tracing::warn!(
                        webhook_url = %self.config.url,
                        retry = retries,
                        error = %e,
                        "webhook request failed, retrying"
                    );
                    // Simple backoff: 1s, 2s, 4s...
                    tokio::time::sleep(tokio::time::Duration::from_secs(1 << retries)).await;
                }
                Err(e) => return Err(e),
            }
        }

        Ok(())
    }

    async fn send_request(&self, body: &str) -> Result<()> {
        use std::time::Duration;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.config.timeout_secs))
            .build()
            .map_err(|e| OxoFlowError::Validation {
                message: format!("failed to build HTTP client: {}", e),
                rule: None,
                suggestion: None,
            })?;

        let mut request = match self.config.method {
            HttpMethod::Post => client.post(&self.config.url),
            HttpMethod::Put => client.put(&self.config.url),
            HttpMethod::Get => client.get(&self.config.url),
        };

        // Add headers
        for (key, value) in &self.config.headers {
            request = request.header(key, value);
        }

        // Add signature if a secret is configured.
        if let Some(ref secret) = self.config.secret {
            let signature = self.compute_signature(body, secret);
            request = request.header("X-OxoFlow-Signature", signature);
        }

        // Add body for POST/PUT
        if self.config.method != HttpMethod::Get {
            request = request
                .header("Content-Type", "application/json")
                .body(body.to_string());
        }

        let response = request.send().await.map_err(|e| OxoFlowError::Validation {
            message: format!("webhook request failed: {}", e),
            rule: None,
            suggestion: Some("Check webhook URL and network connectivity".to_string()),
        })?;

        if !response.status().is_success() {
            return Err(OxoFlowError::Validation {
                message: format!("webhook returned non-success status: {}", response.status()),
                rule: None,
                suggestion: Some("Verify webhook endpoint accepts the payload format".to_string()),
            });
        }

        tracing::info!(
            webhook_url = %self.config.url,
            event = ?self.config.events,
            "webhook notification sent successfully"
        );

        Ok(())
    }

    fn compute_signature(&self, body: &str, secret: &str) -> String {
        match self.config.signature_scheme {
            SignatureScheme::HmacSha256 => {
                use hmac::{Hmac, Mac};
                use sha2::Sha256;
                let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
                    .expect("HMAC accepts keys of any size");
                mac.update(body.as_bytes());
                format!("hmac-sha256={}", hex::encode(mac.finalize().into_bytes()))
            }
            SignatureScheme::Sha256Keyed => {
                tracing::warn!(
                    "webhook signature scheme 'sha256-keyed' is deprecated — switch to 'hmac-sha256'"
                );
                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(secret.as_bytes());
                hasher.update(body.as_bytes());
                format!("sha256={}", hex::encode(hasher.finalize()))
            }
        }
    }
}

/// Slack-specific webhook payload format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackPayload {
    /// Message text.
    pub text: String,

    /// Optional blocks for rich formatting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<SlackBlock>>,
}

/// Slack block kit block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackBlock {
    /// Block type.
    pub type_: String,

    /// Block text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<SlackText>,
}

/// Slack text element.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackText {
    /// Text type (plain_text, mrkdwn).
    pub type_: String,

    /// Text content.
    pub text: String,
}

impl WebhookPayload {
    /// Convert to Slack-compatible payload.
    pub fn to_slack_payload(&self) -> SlackPayload {
        let status_emoji = match self.event {
            WebhookEvent::WorkflowStarted => "🚀",
            WebhookEvent::WorkflowCompleted => {
                if self.data.failed.unwrap_or(0) == 0 {
                    "✅"
                } else {
                    "⚠️"
                }
            }
            WebhookEvent::WorkflowFailed => "❌",
            WebhookEvent::RuleCompleted => "✓",
            WebhookEvent::RuleFailed => "✗",
        };

        let mut text_parts = vec![
            format!("{} **{}**", status_emoji, self.event),
            format!("Workflow: {}", self.workflow_name),
        ];

        if let Some(d) = self.data.duration_ms {
            text_parts.push(format!("Duration: {}ms", d));
        }
        if let Some(s) = self.data.succeeded {
            text_parts.push(format!("Succeeded: {}", s));
        }
        if let Some(f) = self.data.failed {
            text_parts.push(format!("Failed: {}", f));
        }

        SlackPayload {
            text: text_parts.join("\n"),
            blocks: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_config_defaults() {
        let config: WebhookConfig =
            serde_json::from_str("{\"url\": \"https://example.com/hook\"}").unwrap();
        assert_eq!(config.method, HttpMethod::Post);
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.events, vec![WebhookEvent::WorkflowCompleted]);
    }

    #[test]
    fn webhook_payload_serialization() {
        let payload = WebhookPayload {
            event: WebhookEvent::WorkflowCompleted,
            workflow_name: "test-workflow".to_string(),
            timestamp: "2026-05-17T12:00:00Z".to_string(),
            data: WebhookData {
                total_rules: Some(5),
                succeeded: Some(5),
                failed: Some(0),
                skipped: Some(0),
                duration_ms: Some(1000),
                rule: None,
                exit_code: None,
                error: None,
            },
            version: "0.6.0".to_string(),
        };

        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("workflow_completed"));
        assert!(json.contains("test-workflow"));
    }

    #[test]
    fn slack_payload_conversion() {
        let payload = WebhookPayload {
            event: WebhookEvent::WorkflowCompleted,
            workflow_name: "test".to_string(),
            timestamp: "2026-05-17T12:00:00Z".to_string(),
            data: WebhookData {
                succeeded: Some(5),
                failed: Some(0),
                duration_ms: Some(1000),
                ..Default::default()
            },
            version: "0.6.0".to_string(),
        };

        let slack = payload.to_slack_payload();
        assert!(slack.text.contains("✅"));
        assert!(slack.text.contains("Workflow: test"));
    }

    #[test]
    fn hmac_sha256_matches_rfc4231_case1() {
        // RFC 4231 test case 1: key = 0x0b × 20, data = "Hi There".
        let key: String = std::iter::repeat_n(0x0bu8, 20).map(char::from).collect();
        let config = WebhookConfig {
            url: "https://example.com".to_string(),
            method: HttpMethod::Post,
            headers: HashMap::new(),
            events: vec![WebhookEvent::WorkflowCompleted],
            secret: Some(key),
            signature_scheme: SignatureScheme::HmacSha256,
            timeout_secs: 30,
            max_retries: 0,
        };
        let client = WebhookClient::new(config);
        let sig = client.compute_signature("Hi There", &"\u{0b}".repeat(20));
        assert_eq!(
            sig,
            "hmac-sha256=b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn hmac_sha256_matches_rfc4231_case2() {
        // RFC 4231 test case 2: key = "Jefe", data = "what do ya want for nothing?".
        let config = WebhookConfig {
            url: "https://example.com".to_string(),
            method: HttpMethod::Post,
            headers: HashMap::new(),
            events: vec![WebhookEvent::WorkflowCompleted],
            secret: Some("Jefe".to_string()),
            signature_scheme: SignatureScheme::HmacSha256,
            timeout_secs: 30,
            max_retries: 0,
        };
        let client = WebhookClient::new(config);
        let sig = client.compute_signature("what do ya want for nothing?", "Jefe");
        assert_eq!(
            sig,
            "hmac-sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn legacy_sha256_keyed_still_available() {
        let config = WebhookConfig {
            url: "https://example.com".to_string(),
            method: HttpMethod::Post,
            headers: HashMap::new(),
            events: vec![WebhookEvent::WorkflowCompleted],
            secret: Some("test-secret".to_string()),
            signature_scheme: SignatureScheme::Sha256Keyed,
            timeout_secs: 30,
            max_retries: 0,
        };
        let client = WebhookClient::new(config);
        let sig = client.compute_signature("test-body", "test-secret");
        assert!(sig.starts_with("sha256="));
        assert_eq!(sig.len(), 71); // sha256= + 64 hex chars
    }

    #[test]
    fn default_scheme_is_hmac_sha256() {
        let config: WebhookConfig =
            serde_json::from_str(r#"{"url": "https://example.com", "secret": "k"}"#).unwrap();
        assert_eq!(config.signature_scheme, SignatureScheme::HmacSha256);

        let explicit: WebhookConfig = serde_json::from_str(
            r#"{"url": "https://example.com", "signature_scheme": "sha256-keyed"}"#,
        )
        .unwrap();
        assert_eq!(explicit.signature_scheme, SignatureScheme::Sha256Keyed);
    }

    // ── HTTP method variants ───────────────────────────────────────────

    #[test]
    fn http_method_post_default() {
        let config: WebhookConfig =
            serde_json::from_str(r#"{"url": "https://hooks.slack.com/hook"}"#).unwrap();
        assert_eq!(config.method, HttpMethod::Post);
    }

    #[test]
    fn http_method_explicit_get() {
        let config: WebhookConfig =
            serde_json::from_str(r#"{"url": "https://example.com", "method": "get"}"#).unwrap();
        assert_eq!(config.method, HttpMethod::Get);
    }

    #[test]
    fn http_method_explicit_put() {
        let config: WebhookConfig =
            serde_json::from_str(r#"{"url": "https://example.com", "method": "put"}"#).unwrap();
        assert_eq!(config.method, HttpMethod::Put);
    }

    // ── Event filtering ─────────────────────────────────────────────────

    #[test]
    fn event_not_configured_returns_ok() {
        let config = WebhookConfig {
            url: "https://example.com".to_string(),
            method: HttpMethod::Post,
            headers: std::collections::HashMap::new(),
            events: vec![WebhookEvent::WorkflowCompleted],
            secret: None,
            signature_scheme: SignatureScheme::HmacSha256,
            timeout_secs: 5,
            max_retries: 0,
        };
        let client = WebhookClient::new(config);
        let payload = WebhookPayload {
            event: WebhookEvent::WorkflowStarted,
            workflow_name: "test".to_string(),
            timestamp: "now".to_string(),
            data: WebhookData::default(),
            version: "1.0".to_string(),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert!(rt.block_on(client.send(&payload)).is_ok());
    }

    // ── HMAC edge cases ─────────────────────────────────────────────────

    #[test]
    fn hmac_empty_secret() {
        let client = WebhookClient::new(WebhookConfig {
            url: "https://example.com".to_string(),
            method: HttpMethod::Post,
            headers: std::collections::HashMap::new(),
            events: vec![WebhookEvent::WorkflowCompleted],
            secret: Some("".to_string()),
            signature_scheme: SignatureScheme::HmacSha256,
            timeout_secs: 5,
            max_retries: 0,
        });
        let sig = client.compute_signature("body", "");
        assert!(sig.starts_with("hmac-sha256="));
    }

    #[test]
    fn hmac_empty_body() {
        let client = WebhookClient::new(WebhookConfig {
            url: "https://example.com".to_string(),
            method: HttpMethod::Post,
            headers: std::collections::HashMap::new(),
            events: vec![WebhookEvent::WorkflowCompleted],
            secret: Some("key".to_string()),
            signature_scheme: SignatureScheme::HmacSha256,
            timeout_secs: 5,
            max_retries: 0,
        });
        let sig = client.compute_signature("", "key");
        assert!(sig.starts_with("hmac-sha256="));
    }

    // ── Slack payload conversion variants ────────────────────────────────

    #[test]
    fn slack_payload_started() {
        let payload = WebhookPayload {
            event: WebhookEvent::WorkflowStarted,
            workflow_name: "w".to_string(),
            timestamp: "t".to_string(),
            data: WebhookData::default(),
            version: "1".to_string(),
        };
        let slack = payload.to_slack_payload();
        assert!(slack.text.contains("🚀"));
    }

    #[test]
    fn slack_payload_failed() {
        let payload = WebhookPayload {
            event: WebhookEvent::WorkflowFailed,
            workflow_name: "w".to_string(),
            timestamp: "t".to_string(),
            data: WebhookData {
                error: Some("oops".into()),
                ..Default::default()
            },
            version: "1".to_string(),
        };
        let slack = payload.to_slack_payload();
        assert!(slack.text.contains("❌"));
    }

    // ── Event Display ───────────────────────────────────────────────────

    #[test]
    fn event_display_all() {
        assert_eq!(
            WebhookEvent::WorkflowStarted.to_string(),
            "workflow_started"
        );
        assert_eq!(
            WebhookEvent::WorkflowCompleted.to_string(),
            "workflow_completed"
        );
        assert_eq!(WebhookEvent::WorkflowFailed.to_string(), "workflow_failed");
        assert_eq!(WebhookEvent::RuleCompleted.to_string(), "rule_completed");
        assert_eq!(WebhookEvent::RuleFailed.to_string(), "rule_failed");
    }

    // ── WebhookData defaults ────────────────────────────────────────────

    #[test]
    fn webhook_data_all_none() {
        let data = WebhookData::default();
        assert!(data.total_rules.is_none());
        assert!(data.succeeded.is_none());
        assert!(data.error.is_none());
    }

    #[test]
    fn webhook_data_serialization_omits_none() {
        let data = WebhookData::default();
        let json = serde_json::to_string(&data).unwrap();
        assert_eq!(json, "{}");
    }
}
