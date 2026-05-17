# Webhook Notifications

oxo-flow includes a built-in webhook system that can send notifications about workflow and rule execution events to external services like Slack, Microsoft Teams, Discord, or any custom HTTP endpoint.

## Configuration

Webhooks are configured in your `.oxoflow` workflow file under the `[webhooks]` table. You can define multiple webhooks, each with its own target URL, events to subscribe to, and payload format.

### Example Configuration

```toml
[workflow]
name = "variant-calling"
version = "1.0.0"

[[webhooks]]
# Send all workflow-level events to a Slack channel
url = "https://hooks.slack.com/services/YOUR/WEBHOOK/URL"
events = ["WorkflowStarted", "WorkflowCompleted", "WorkflowFailed", "WorkflowCancelled"]
format = "slack"

[[webhooks]]
# Send only failed rule events to a custom monitoring endpoint
url = "https://api.my-monitoring.com/v1/alerts"
events = ["RuleFailed"]
format = "json"
secret = "my-shared-secret"  # Used for HMAC-SHA256 signature
```

## Supported Events

You can subscribe to the following events by listing them in the `events` array:

| Event Name | Description |
| --- | --- |
| `WorkflowStarted` | Fired when the workflow execution begins. |
| `WorkflowCompleted` | Fired when all rules in the workflow have completed successfully. |
| `WorkflowFailed` | Fired when the workflow fails (e.g., due to a failed rule with keep_going=false). |
| `WorkflowCancelled` | Fired when the workflow execution is manually cancelled. |
| `RuleStarted` | Fired when an individual rule begins execution. |
| `RuleCompleted` | Fired when a rule completes successfully. |
| `RuleFailed` | Fired when a rule fails after all retries are exhausted. |
| `RuleSkipped` | Fired when a rule is skipped (e.g., outputs are already up-to-date). |

If the `events` array is omitted, the webhook will receive **all** events by default.

## Payload Formats

The `format` field determines how the event data is serialized and sent to the endpoint.

### `json` (Default)

The `json` format sends a standard HTTP POST request with `Content-Type: application/json`. The payload includes detailed information about the event, the workflow, and (if applicable) the specific rule.

```json
{
  "event": "RuleFailed",
  "workflow": "variant-calling",
  "timestamp": "2026-05-18T12:00:00Z",
  "rule": "align_reads",
  "message": "rule 'align_reads' failed with exit code 1",
  "details": {
    "exit_code": 1,
    "stderr": "bwa: command not found"
  }
}
```

### `slack`

The `slack` format transforms the event into a Slack-compatible message payload (`{"text": "...", "blocks": [...]}`). This is directly compatible with Slack Incoming Webhooks. It uses color-coding (green for success, red for failure) and formatted blocks to display rule and workflow status clearly.

## Security (HMAC Signatures)

If you provide a `secret` field in your webhook configuration, oxo-flow will compute an HMAC-SHA256 signature of the payload and include it in the `X-Hub-Signature-256` HTTP header. 

Your receiving endpoint can use this signature to verify that the webhook request genuinely originated from your oxo-flow execution and that the payload was not tampered with in transit.

```http
POST /alerts HTTP/1.1
Host: api.my-monitoring.com
Content-Type: application/json
X-Hub-Signature-256: sha256=abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890

{"event": "RuleFailed", ...}
```
