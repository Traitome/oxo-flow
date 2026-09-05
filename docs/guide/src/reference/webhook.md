# Webhook Notifications

oxo-flow includes a webhook client (`oxo_flow_core::webhook`, behind the `webhook` cargo feature — enabled by default) that can send JSON notifications about workflow and rule execution events to external services like Slack, Microsoft Teams, Discord, or any custom HTTP endpoint.

> **Status note:** the webhook module provides the configuration model, the HTTP client (with retries and HMAC signing), and the payload types. It is not yet wired into the executor — the engine does not currently dispatch webhook notifications during workflow execution.

## Configuration

A webhook is described by the `WebhookConfig` struct:

| Field | Type | Default | Description |
|---|---|---|---|
| `url` | String | — (required) | Webhook endpoint URL. |
| `method` | String | `"post"` | HTTP method: `post`, `put`, or `get`. |
| `headers` | Map | `{}` | Custom HTTP headers to include in the request. |
| `events` | Array of String | `["workflow_completed"]` | Events to subscribe to (see below). If omitted, **only** `workflow_completed` is used. |
| `secret` | String | none | Secret key for HMAC-SHA256 signature. |
| `timeout_secs` | Integer | `30` | Request timeout in seconds. |
| `max_retries` | Integer | `3` | Maximum retries on failure (with exponential backoff: 1s, 2s, 4s...). |

### Example Configuration

```toml
url = "https://hooks.slack.com/services/YOUR/WEBHOOK/URL"
events = ["workflow_started", "workflow_completed", "workflow_failed"]
method = "post"
headers = { "X-Custom" = "value" }
secret = "my-shared-secret"  # Used for HMAC-SHA256 signature
timeout_secs = 30
max_retries = 3
```

## Supported Events

Events are serialized in snake_case. You can subscribe by listing them in the `events` array:

| Event Name | Description |
| --- | --- |
| `workflow_started` | Fired when the workflow execution begins. |
| `workflow_completed` | Fired when the workflow finishes (success or failure). |
| `workflow_failed` | Fired when the workflow fails. |
| `rule_completed` | Fired when an individual rule completes. |
| `rule_failed` | Fired when an individual rule fails. |

There is no `RuleStarted` or `RuleSkipped` event in the webhook module. (The
`ExecutionEvent` log stream in the core executor likewise has no
`workflow_cancelled` variant — an interruption surfaces as `rule_failed`
records with an `interrupted by <signal>` skip reason.)

If the `events` array is omitted, the default subscription is `["workflow_completed"]` only.

## Payload

The `json` payload is a standard HTTP POST request with `Content-Type: application/json`. The body is a `WebhookPayload`:

```json
{
  "event": "rule_failed",
  "workflow_name": "variant-calling",
  "timestamp": "2026-05-18T12:00:00Z",
  "data": {
    "total_rules": 12,
    "succeeded": 10,
    "failed": 1,
    "skipped": 1,
    "duration_ms": 45210,
    "rule": "align_reads",
    "exit_code": 1,
    "error": "bwa: command not found"
  },
  "version": "0.17.2"
}
```

Field details:

- `event` — the event name in snake_case.
- `workflow_name` — the workflow name.
- `timestamp` — ISO 8601 timestamp.
- `data` — event-specific fields; only the fields relevant to the event are present (`total_rules`, `succeeded`, `failed`, `skipped`, `duration_ms`, `rule`, `exit_code`, `error`).
- `version` — the oxo-flow version that sent the notification.

### Slack format

For Slack-compatible endpoints, `WebhookPayload::to_slack_payload()` converts the payload to a `{"text": "...", "blocks": [...]}` message with per-event status emoji (green success, red failure), directly compatible with Slack Incoming Webhooks.

## Security (HMAC Signatures)

If you set the `secret` field, oxo-flow computes an RFC-2104 HMAC-SHA256
signature over the payload body and includes it in the
`X-OxoFlow-Signature` HTTP header (verified against the published RFC 4231
test vectors):

```http
POST /alerts HTTP/1.1
Host: api.my-monitoring.com
Content-Type: application/json
X-OxoFlow-Signature: hmac-sha256=abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890

{"event": "rule_failed", ...}
```

Your receiving endpoint can use this signature to verify that the webhook request genuinely originated from your oxo-flow execution and that the payload was not tampered with in transit.

**Legacy scheme.** The original implementation signed with a non-standard
keyed SHA-256 (`sha256=hex(sha256(secret‖body))`), which is *not*
HMAC-SHA256 despite the header name. It remains available via
`signature_scheme = "sha256-keyed"` for existing consumers and is frozen:
emit a warning, switch your verifier to `hmac-sha256`, and it will be
removed in a future major version. The default is `"hmac-sha256"`.
