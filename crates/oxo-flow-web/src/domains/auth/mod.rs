//! Auth domain — authentication, sessions, and OAuth2.
//!
//! Supports username/password login, ORCID and GitHub OAuth2 flows,
//! session token management, and role-based access control (admin, user, viewer).
//! Invite-code authentication is available for air-gapped deployments.

pub mod handlers;
pub mod oauth;
pub mod service;
pub mod types;
