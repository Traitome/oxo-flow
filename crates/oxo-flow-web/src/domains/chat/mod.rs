//! Chat domain — conversational AI pipeline creation.
//!
//! Provides SSE-streaming chat with multi-agent orchestration:
//! Orchestrator → (Data Agent, Tool Expert, Validator) → Response.
//! All agent calls go through deterministic core APIs — zero write access.

pub mod handlers;
pub mod service;
pub mod types;
