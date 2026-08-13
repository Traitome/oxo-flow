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
    /// Optional run id — scopes the read-only run-diagnosis tools.
    #[serde(default)]
    pub run_id: Option<String>,
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

/// A chat session summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSession {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}
