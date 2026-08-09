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
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(content: &str) -> Self {
        Self {
            role: MessageRole::User,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: &str) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant_with_tools(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: String::new(),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn tool(tool_call_id: &str, name: &str, content: &str) -> Self {
        Self {
            role: MessageRole::Tool,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.to_string()),
            name: Some(name.to_string()),
        }
    }
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
    /// Tool calls requested by the model (None when returning text).
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Token usage for this call.
    pub usage: Usage,
    /// Why the response ended: "stop", "tool_calls", "length", etc.
    pub finish_reason: String,
}

/// Token usage for a single AI call.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
