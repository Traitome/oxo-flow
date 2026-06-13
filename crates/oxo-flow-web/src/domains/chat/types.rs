use serde::{Deserialize, Serialize};

/// Request to send a message in a chat session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    /// Optional session ID for continuing an existing conversation.
    pub session_id: Option<String>,
    /// The user's message text.
    pub message: String,
    /// Optional context for the AI (data paths, intent hints, etc.).
    pub context: Option<ChatContext>,
}

/// Context provided with a chat message to help AI understand the user's setup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatContext {
    /// Paths to data files/directories for Data Agent to scan.
    pub data_paths: Option<Vec<String>>,
    /// Optional samplesheet CSV content (base64 or raw).
    pub samplesheet: Option<String>,
    /// Explicit intent hint (overrides AI inference).
    pub intent: Option<String>,
}

/// SSE event types emitted during chat processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ChatEvent {
    /// A chunk of text from the AI response.
    #[serde(rename = "text")]
    Text { chunk: String },
    /// An agent status update.
    #[serde(rename = "agent")]
    Agent {
        agent: String,
        status: String,
        progress: Option<f64>,
    },
    /// A structured action for the user to take.
    #[serde(rename = "action")]
    Action {
        action_type: String,
        data: serde_json::Value,
    },
    /// An error occurred.
    #[serde(rename = "error")]
    Error { code: String, message: String },
    /// The chat session is complete.
    #[serde(rename = "done")]
    Done {
        session_id: String,
        pipeline_id: Option<String>,
    },
}

/// A chat session summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSession {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A single message in a chat session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub metadata: Option<serde_json::Value>,
    pub created_at: String,
}
