//! Shared AI types — message structures, tool definitions, responses.
//!
//! These types align with the OpenAI/DeepSeek API format for maximum
//! compatibility across providers.

use serde::{Deserialize, Serialize};

// ── Message types ──────────────────────────────────────────────────────────

/// A chat message in the OpenAI-compatible format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
    /// DeepSeek reasoning models emit `reasoning_content` for assistant turns.
    /// It must be echoed back verbatim on subsequent API calls, so we keep it
    /// in the transcript (ignored by non-DeepSeek providers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Message {
    pub fn system(content: &str) -> Self {
        Self {
            role: MessageRole::System,
            content: content.to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(content: &str) -> Self {
        Self {
            role: MessageRole::User,
            content: content.to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: &str) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant_with_reasoning(content: &str, reasoning_content: &str) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.to_string(),
            reasoning_content: Some(reasoning_content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant_with_tools(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: String::new(),
            reasoning_content: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant_with_tools_and_reasoning(
        tool_calls: Vec<ToolCall>,
        reasoning_content: &str,
    ) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: String::new(),
            reasoning_content: Some(reasoning_content.to_string()),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
        }
    }

    /// Tool result message. Content is bounded to
    /// [`MAX_TOOL_RESULT_BYTES`] with a truncation marker (issue #73) —
    /// every transcript built through this constructor stays within
    /// predictable context limits.
    pub fn tool(tool_call_id: &str, name: &str, content: &str) -> Self {
        Self {
            role: MessageRole::Tool,
            content: bound_tool_result(content),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.to_string()),
            name: Some(name.to_string()),
        }
    }
}

/// Maximum bytes a single tool result may occupy in a transcript.
pub const MAX_TOOL_RESULT_BYTES: usize = 16 * 1024;

/// Bound a tool result to [`MAX_TOOL_RESULT_BYTES`], keeping the head and
/// tail (UTF-8 safe) and inserting a marker reporting the original size.
pub fn bound_tool_result(content: &str) -> String {
    if content.len() <= MAX_TOOL_RESULT_BYTES {
        return content.to_string();
    }
    let half = MAX_TOOL_RESULT_BYTES / 2;
    let head = truncate_utf8_from_start(content, half);
    let tail = truncate_utf8_from_end(content, half);
    format!(
        "{head}\n[... tool result truncated: {} bytes total; middle omitted ...]\n{tail}",
        content.len()
    )
}

/// Largest prefix of `s` with at most `max_bytes` bytes (char-boundary safe).
fn truncate_utf8_from_start(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Largest suffix of `s` with at most `max_bytes` bytes (char-boundary safe).
fn truncate_utf8_from_end(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut start = s.len() - max_bytes;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageRole {
    #[serde(rename = "system")]
    System,
    #[serde(rename = "user")]
    User,
    #[serde(rename = "assistant")]
    Assistant,
    #[serde(rename = "tool")]
    Tool,
}

// ── Tool definitions ───────────────────────────────────────────────────────

/// A tool definition exposed to the AI model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    /// Tool name (must match the registry key).
    pub name: String,
    /// Human-readable description — the model uses this to decide when to call.
    pub description: String,
    /// JSON Schema for the tool's parameters.
    pub parameters: serde_json::Value,
}

/// A tool call issued by the AI model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique call identifier (matches the response tool_call_id).
    pub id: String,
    /// Function name to invoke.
    pub name: String,
    /// JSON-encoded arguments string.
    pub arguments: String,
}

// ── AI response ────────────────────────────────────────────────────────────

/// Parsed response from an AI provider chat call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResponse {
    /// Text content (None when the model issues a tool call instead).
    pub content: Option<String>,
    /// Reasoning content emitted by DeepSeek-style reasoning models; must be
    /// echoed back in the next request when it is non-empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// Tool calls requested by the model (None when returning text).
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Token usage for this call.
    pub usage: Usage,
    /// Why the response ended: "stop", "tool_calls", "length", etc.
    pub finish_reason: String,
}

/// Token usage for a single AI call.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

impl Usage {
    /// Estimated cost in USD for DeepSeek models.
    /// Pricing as of 2026-08: deepseek-v4-pro $0.28/M in, $1.10/M out.
    pub fn cost_deepseek_v4_pro(&self) -> f64 {
        self.prompt_tokens as f64 * 0.28 / 1_000_000.0
            + self.completion_tokens as f64 * 1.10 / 1_000_000.0
    }

    /// Estimated cost in USD for DeepSeek flash models.
    /// Pricing as of 2026-08: deepseek-v4-flash $0.14/M in, $0.55/M out.
    pub fn cost_deepseek_v4_flash(&self) -> f64 {
        self.prompt_tokens as f64 * 0.14 / 1_000_000.0
            + self.completion_tokens as f64 * 0.55 / 1_000_000.0
    }
}

// ── Serialization helpers ──────────────────────────────────────────────────

/// Convert tool calls to the native API format for OpenAI-compatible providers.
pub fn tool_calls_to_openai(tool_calls: &[ToolCall]) -> serde_json::Value {
    serde_json::Value::Array(
        tool_calls
            .iter()
            .map(|tc| {
                serde_json::json!({
                    "id": tc.id,
                    "type": "function",
                    "function": {
                        "name": tc.name,
                        "arguments": tc.arguments,
                    }
                })
            })
            .collect(),
    )
}

/// Parse tool calls from OpenAI-compatible API response.
pub fn tool_calls_from_openai(json: &serde_json::Value) -> Option<Vec<ToolCall>> {
    json.as_array().map(|arr| {
        arr.iter()
            .filter_map(|tc| {
                let func = tc.get("function")?;
                Some(ToolCall {
                    id: tc.get("id")?.as_str()?.to_string(),
                    name: func.get("name")?.as_str()?.to_string(),
                    arguments: func
                        .get("arguments")
                        .and_then(|a| a.as_str())
                        .unwrap_or("{}")
                        .to_string(),
                })
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_serialization_roundtrip() {
        let msg = Message::user("Hello");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"user\""));
        assert!(json.contains("Hello"));
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, MessageRole::User);
        assert_eq!(back.content, "Hello");
    }

    #[test]
    fn tool_call_serialization() {
        let calls = vec![ToolCall {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"/tmp/test.txt"}"#.into(),
        }];
        let json = tool_calls_to_openai(&calls);
        assert_eq!(json[0]["id"], "call_1");
        assert_eq!(json[0]["function"]["name"], "read_file");
    }

    #[test]
    fn tool_call_parsing() {
        let json = serde_json::json!([
            {
                "id": "call_abc",
                "type": "function",
                "function": {
                    "name": "lookup_tool",
                    "arguments": "{\"tool\": \"STAR\"}"
                }
            }
        ]);
        let parsed = tool_calls_from_openai(&json).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "lookup_tool");
        assert_eq!(parsed[0].arguments, r#"{"tool": "STAR"}"#);
    }

    #[test]
    fn bound_tool_result_keeps_short_content_unchanged() {
        let content = "short tool output";
        assert_eq!(bound_tool_result(content), content);
    }

    #[test]
    fn bound_tool_result_truncates_long_content_with_marker() {
        let content = "abcdef".repeat(10_000); // 60_000 bytes
        let bounded = bound_tool_result(&content);
        assert!(bounded.len() < content.len() + 128);
        assert!(
            bounded.contains("tool result truncated"),
            "marker required: {bounded}"
        );
        assert!(
            bounded.contains("60000 bytes total"),
            "marker must report the total size: {bounded}"
        );
        // Head and tail survive so the model keeps both the leading and
        // trailing context of the result.
        assert!(bounded.starts_with("abcdefabcdef"));
        assert!(bounded.ends_with("abcdefabcdef"));
        assert_eq!(bounded.matches("truncated").count(), 1);
    }

    #[test]
    fn bound_tool_result_is_utf8_safe_at_boundaries() {
        // Multi-byte characters straddling the byte budget must not panic
        // or produce replacement characters.
        let content = "测".repeat(60_000); // 3 bytes per char
        let bounded = bound_tool_result(&content);
        assert!(!bounded.contains('\u{FFFD}'), "no replacement chars");
        assert!(bounded.chars().all(|c| c == '测'
            || c.is_ascii()
            || c == '['
            || c == ']'
            || c == '.'
            || c == ' '
            || c == '\n'
            || c == ':'
            || c.is_ascii_digit()));
    }

    #[test]
    fn tool_message_bounds_content() {
        let content = "x".repeat(100_000);
        let msg = Message::tool("call_1", "lookup_tool", &content);
        assert!(msg.content.len() < content.len());
        assert!(msg.content.contains("tool result truncated"));
        assert_eq!(msg.tool_call_id.as_deref(), Some("call_1"));
    }

    #[test]
    fn usage_cost_is_positive() {
        let usage = Usage {
            prompt_tokens: 1000,
            completion_tokens: 500,
        };
        assert!(usage.cost_deepseek_v4_pro() > 0.0);
        assert!(usage.cost_deepseek_v4_flash() > 0.0);
        assert!(usage.cost_deepseek_v4_pro() > usage.cost_deepseek_v4_flash());
    }
}
