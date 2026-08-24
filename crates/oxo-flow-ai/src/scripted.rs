//! Offline scripted provider (issue #73, Phase 2.4).
//!
//! Replays a serializable script of completions — tool calls, errors, and
//! artificial delays — through the real [`crate::provider::AiProvider`]
//! code path (request building, parsing, tool loops). CI can exercise
//! multi-turn tool-calling and error recovery without an API key or quota.
//!
//! Every call's transcript is recorded so tests can assert what the caller
//! actually sent (e.g. that an overflow retry used the compressed form).

use crate::error::AiError;
use crate::provider::AiProvider;
use crate::types::{AiResponse, Message, ToolCall, ToolDef, Usage};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;

/// One scripted completion replayed in order.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScriptedTurn {
    /// Replayed response content.
    #[serde(default)]
    pub content: Option<String>,
    /// Replayed tool calls.
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Error replayed instead of a response, as a tag:
    /// `context_overflow[:msg]`, `output_limit[:msg]`, `rate_limited`,
    /// `auth[:msg]`, `provider[:msg]`. Unknown tags replay as a provider
    /// error carrying the tag — fail loud, never silently.
    #[serde(default)]
    pub error: Option<String>,
    /// Artificial latency in milliseconds before the turn is replayed.
    #[serde(default)]
    pub delay_ms: u64,
}

impl ScriptedTurn {
    /// A turn that returns text content.
    pub fn content(content: impl Into<String>) -> Self {
        Self {
            content: Some(content.into()),
            ..Default::default()
        }
    }

    /// A turn that returns a tagged error.
    pub fn error(tag: impl Into<String>) -> Self {
        Self {
            error: Some(tag.into()),
            ..Default::default()
        }
    }
}

/// Replays [`ScriptedTurn`]s through the provider interface.
#[derive(Clone)]
pub struct ScriptedBackend {
    turns: Arc<Mutex<VecDeque<ScriptedTurn>>>,
    /// Transcripts received on each call, in order — for assertions.
    calls: Arc<Mutex<Vec<Vec<Message>>>>,
    model: String,
}

impl ScriptedBackend {
    /// Create a backend replaying `turns` in order.
    pub fn new(turns: Vec<ScriptedTurn>) -> Self {
        Self {
            turns: Arc::new(Mutex::new(turns.into())),
            calls: Arc::new(Mutex::new(Vec::new())),
            model: "scripted-test".into(),
        }
    }

    /// Deserialize a script from JSON (e.g. a test fixture file).
    pub fn from_json(spec: &str) -> Result<Self, AiError> {
        let turns: Vec<ScriptedTurn> = serde_json::from_str(spec).map_err(|e| AiError::Config {
            message: format!("invalid scripted-provider JSON: {e}"),
        })?;
        Ok(Self::new(turns))
    }

    /// Number of turns still queued.
    pub async fn remaining(&self) -> usize {
        self.turns.lock().await.len()
    }

    /// Model name reported by the provider interface.
    pub fn model_name(&self) -> String {
        self.model.clone()
    }

    /// Transcripts received so far, in call order.
    pub async fn observed_calls(&self) -> Vec<Vec<Message>> {
        self.calls.lock().await.clone()
    }

    pub(crate) async fn chat_with_tools(
        &self,
        messages: &[Message],
        _tools: &[ToolDef],
    ) -> Result<AiResponse, AiError> {
        self.calls.lock().await.push(messages.to_vec());
        let turn = self
            .turns
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| AiError::Provider {
                provider: "scripted".into(),
                message: "script exhausted — no turns left to replay".into(),
            })?;
        if turn.delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(turn.delay_ms)).await;
        }
        if let Some(tag) = turn.error {
            return Err(scripted_error(&tag));
        }
        let finish_reason = if turn.tool_calls.is_some() {
            "tool_calls"
        } else {
            "stop"
        };
        Ok(AiResponse {
            content: turn.content,
            reasoning_content: None,
            tool_calls: turn.tool_calls,
            usage: Usage::default(),
            finish_reason: finish_reason.to_string(),
        })
    }
}

/// Map a scripted error tag to an [`AiError`].
fn scripted_error(tag: &str) -> AiError {
    let (kind, message) = match tag.split_once(':') {
        Some((kind, rest)) => (kind, rest.to_string()),
        None => (tag, tag.to_string()),
    };
    let provider = "scripted".to_string();
    match kind {
        "context_overflow" => AiError::ContextOverflow { provider, message },
        "output_limit" => AiError::OutputLimit { provider, message },
        "rate_limited" => AiError::RateLimited {
            provider,
            retry_after: None,
        },
        "auth" => AiError::Auth { provider, message },
        _ => AiError::Provider {
            provider,
            message: format!("scripted error: {tag}"),
        },
    }
}

/// Convenience: an [`AiProvider::Scripted`] from turns.
pub fn scripted_provider(turns: Vec<ScriptedTurn>) -> AiProvider {
    AiProvider::Scripted(ScriptedBackend::new(turns))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{COMPRESS_KEEP_TAIL, compress_transcript};

    fn long_transcript() -> Vec<Message> {
        let mut messages = vec![Message::system("sys")];
        for i in 0..12 {
            messages.push(Message::user(&format!("user turn {i}")));
            messages.push(Message::assistant(&format!("assistant turn {i}")));
        }
        messages.push(Message::user("final question"));
        messages
    }

    #[tokio::test]
    async fn scripted_provider_replays_content_then_tool_calls() {
        let provider = scripted_provider(vec![
            ScriptedTurn::content("first answer"),
            ScriptedTurn {
                content: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call_1".into(),
                    name: "lookup_tool".into(),
                    arguments: "{}".into(),
                }]),
                ..Default::default()
            },
        ]);
        let first = provider
            .chat_with_tools(&[Message::user("hi")], &[])
            .await
            .unwrap();
        assert_eq!(first.content.as_deref(), Some("first answer"));
        let second = provider
            .chat_with_tools(&[Message::user("again")], &[])
            .await
            .unwrap();
        assert!(second.content.is_none());
        assert_eq!(second.tool_calls.unwrap()[0].name, "lookup_tool");

        // Exhausted scripts fail loudly.
        let err = provider
            .chat_with_tools(&[Message::user("third")], &[])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("exhausted"), "{err}");
    }

    #[tokio::test]
    async fn scripted_provider_replays_tagged_errors() {
        let provider = scripted_provider(vec![ScriptedTurn::error("context_overflow:too big")]);
        let err = provider
            .chat_with_tools(&[Message::user("hi")], &[])
            .await
            .unwrap_err();
        assert!(matches!(err, AiError::ContextOverflow { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn overflow_safe_compresses_and_retries_once() {
        let backend = ScriptedBackend::new(vec![
            ScriptedTurn::error("context_overflow:window full"),
            ScriptedTurn::content("ok after compression"),
        ]);
        let provider = AiProvider::Scripted(backend.clone());

        let response = provider
            .chat_with_tools_overflow_safe(&long_transcript(), &[])
            .await
            .unwrap();
        assert_eq!(response.content.as_deref(), Some("ok after compression"));

        // The retry must have used the compressed transcript.
        let calls = backend.observed_calls().await;
        assert_eq!(calls.len(), 2, "one retry, not more");
        assert_eq!(calls[0].len(), 26, "first call sends the full transcript");
        assert_eq!(
            calls[1].len(),
            2 + COMPRESS_KEEP_TAIL,
            "retry sends system + marker + tail"
        );
        assert!(calls[1][1].content.contains("omitted"));
    }

    #[tokio::test]
    async fn overflow_safe_second_overflow_is_readable() {
        let provider = scripted_provider(vec![
            ScriptedTurn::error("context_overflow:still full"),
            ScriptedTurn::error("context_overflow:still full"),
        ]);
        let err = provider
            .chat_with_tools_overflow_safe(&long_transcript(), &[])
            .await
            .unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("reduce"),
            "readable guidance expected: {text}"
        );
    }

    #[tokio::test]
    async fn overflow_safe_does_not_retry_other_errors() {
        let backend = ScriptedBackend::new(vec![ScriptedTurn::error("rate_limited")]);
        let provider = AiProvider::Scripted(backend.clone());
        let err = provider
            .chat_with_tools_overflow_safe(&long_transcript(), &[])
            .await
            .unwrap_err();
        assert!(matches!(err, AiError::RateLimited { .. }), "{err:?}");
        assert_eq!(backend.observed_calls().await.len(), 1);
    }

    #[tokio::test]
    async fn overflow_safe_bails_when_transcript_has_nothing_to_drop() {
        let provider = scripted_provider(vec![ScriptedTurn::error("context_overflow:full")]);
        let small = vec![Message::system("sys"), Message::user("final")];
        let err = provider
            .chat_with_tools_overflow_safe(&small, &[])
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("no removable turns"),
            "readable guidance expected: {err}"
        );
    }

    #[tokio::test]
    async fn scripted_provider_from_json_roundtrip() {
        let spec = r#"[{"content":"hello"},{"error":"rate_limited","delay_ms":5}]"#;
        let backend = ScriptedBackend::from_json(spec).unwrap();
        assert_eq!(backend.remaining().await, 2);
    }

    #[test]
    fn compress_transcript_is_importable_from_provider() {
        // compress_transcript lives in provider.rs; the overflow path uses it.
        let compressed = compress_transcript(&long_transcript()).unwrap();
        assert_eq!(compressed.len(), 2 + COMPRESS_KEEP_TAIL);
    }
}
