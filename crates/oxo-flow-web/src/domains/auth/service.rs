//! Pure authentication domain logic — zero HTTP dependency.
//!
//! Each function takes plain Rust types and returns `Result<T, String>`.
//! Suitable for reuse from handlers, CLI commands, or tests without
//! coupling to axum or any web framework.

use uuid::Uuid;

use super::types::*;

/// Authenticate user with username and password.
/// Uses env-var based credential checking (backward compat with existing system).
pub fn authenticate(username: &str, password: &str) -> Result<LoginResponse, String> {
    // Check admin
    if let Ok(admin_pw) = std::env::var("OXO_FLOW_ADMIN_PASSWORD")
        && username == "admin"
        && password == admin_pw.as_str()
    {
        return Ok(LoginResponse {
            token: generate_token(),
            username: "admin".into(),
            role: "admin".into(),
        });
    }
    // Check user
    if let Ok(user_pw) = std::env::var("OXO_FLOW_USER_PASSWORD")
        && password == user_pw
    {
        return Ok(LoginResponse {
            token: generate_token(),
            username: username.into(),
            role: "user".into(),
        });
    }
    // Check viewer
    if let Ok(viewer_pw) = std::env::var("OXO_FLOW_VIEWER_PASSWORD")
        && password == viewer_pw
    {
        return Ok(LoginResponse {
            token: generate_token(),
            username: username.into(),
            role: "viewer".into(),
        });
    }
    // Dev mode fallback: password equals username — ONLY when explicitly enabled.
    // In production this MUST be off, otherwise anyone can log in as any username.
    if std::env::var("OXO_FLOW_DEV_MODE").as_deref() == Ok("1")
        && password == username
        && !username.is_empty()
    {
        tracing::warn!(
            username = username,
            "DEV MODE: accepted password==username login for '{}'",
            username
        );
        return Ok(LoginResponse {
            token: generate_token(),
            username: username.into(),
            role: "user".into(),
        });
    }

    Err("Invalid credentials".into())
}

/// Validate session token. Returns user info if valid.
pub fn validate_session(token: &str, sessions: &[Session]) -> Result<AuthMeResponse, String> {
    if token.is_empty() {
        return Ok(AuthMeResponse {
            authenticated: false,
            username: None,
            role: None,
        });
    }

    // Check if token exists in the session list and is not expired
    let now = chrono::Utc::now().to_rfc3339();
    for session in sessions {
        if session.token == token {
            if session.expires_at > now {
                // Map user_id to username — in production this queries the DB
                let username = if session.user_id.is_empty() {
                    "user".to_string()
                } else {
                    session.user_id.clone()
                };
                return Ok(AuthMeResponse {
                    authenticated: true,
                    username: Some(username),
                    role: Some("user".into()),
                });
            }
            // Token expired
            return Ok(AuthMeResponse {
                authenticated: false,
                username: None,
                role: None,
            });
        }
    }

    // Token not found in sessions — authentication failed.
    // In production, every token MUST be in the sessions table.
    Ok(AuthMeResponse {
        authenticated: false,
        username: None,
        role: None,
    })
}

/// Check if user has required role.
pub fn check_role(role: &str, required: &str) -> bool {
    match required {
        "admin" => role == "admin",
        "user" => role == "admin" || role == "user",
        _ => true, // viewer can access anything viewer-level
    }
}

/// Get license status from existing OXO_FLOW_CONFIG.
pub fn license_status() -> LicenseResponse {
    LicenseResponse {
        valid: true,
        license_type: Some("academic".into()),
        issued_to: Some("Public Academic Test License (any academic user)".into()),
        commercial_use: "requires_authorization".into(),
        contact: "w_shixiang@163.com".into(),
        message: "Free for academic use. Commercial use requires authorization.".into(),
    }
}

fn generate_token() -> String {
    Uuid::new_v4().to_string()
}

// ---------------------------------------------------------------------------
// OAuth2 service functions
// ---------------------------------------------------------------------------

/// Build an OAuthConfig from environment variables.
///
/// Reads `ORCID_CLIENT_ID`/`ORCID_CLIENT_SECRET` or
/// `GITHUB_CLIENT_ID`/`GITHUB_CLIENT_SECRET` depending on the provider.
pub fn oauth_config_from_env(
    provider: &str,
    redirect_uri: &str,
) -> Result<super::oauth::OAuthConfig, String> {
    match provider.to_lowercase().as_str() {
        "orcid" => {
            let client_id = std::env::var("ORCID_CLIENT_ID")
                .map_err(|_| "ORCID_CLIENT_ID not set".to_string())?;
            let client_secret = std::env::var("ORCID_CLIENT_SECRET")
                .map_err(|_| "ORCID_CLIENT_SECRET not set".to_string())?;
            Ok(super::oauth::OAuthConfig::orcid(
                &client_id,
                &client_secret,
                redirect_uri,
            ))
        }
        "github" => {
            let client_id = std::env::var("GITHUB_CLIENT_ID")
                .map_err(|_| "GITHUB_CLIENT_ID not set".to_string())?;
            let client_secret = std::env::var("GITHUB_CLIENT_SECRET")
                .map_err(|_| "GITHUB_CLIENT_SECRET not set".to_string())?;
            Ok(super::oauth::OAuthConfig::github(
                &client_id,
                &client_secret,
                redirect_uri,
            ))
        }
        _ => Err(format!("Unsupported OAuth provider: {provider}")),
    }
}

/// Persist a pending OAuth CSRF state so the callback can verify it.
pub async fn store_pending_state(state: &str) -> Result<(), String> {
    let pool =
        crate::infra::db::sqlite::try_pool().map_err(|_| "Database unavailable".to_string())?;
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query("INSERT INTO oauth_states (state, created_at) VALUES (?, ?)")
        .bind(state)
        .bind(now)
        .execute(pool)
        .await
        .map_err(|e| format!("Failed to store OAuth state: {e}"))?;
    Ok(())
}

/// Verify a callback's CSRF state was issued by this server and consume it
/// (single use).
pub async fn verify_and_consume_state(state: &str) -> Result<(), String> {
    let pool =
        crate::infra::db::sqlite::try_pool().map_err(|_| "Database unavailable".to_string())?;
    let deleted = sqlx::query("DELETE FROM oauth_states WHERE state = ?")
        .bind(state)
        .execute(pool)
        .await
        .map_err(|e| format!("State verification failed: {e}"))?
        .rows_affected();
    if deleted == 0 {
        return Err("Invalid or expired CSRF state".into());
    }
    Ok(())
}

/// Initiate an OAuth2 authorization flow.
///
/// Returns the provider's authorization URL and a CSRF state token.
pub async fn initiate_oauth(
    provider: &str,
    redirect_uri: &str,
) -> Result<OAuthAuthorizeResponse, String> {
    let config = oauth_config_from_env(provider, redirect_uri)?;
    let state = generate_token();
    let authorize_url = config.authorize_url(&state);

    // Persist the pending state so the callback can verify it (CSRF defense).
    store_pending_state(&state).await?;

    Ok(OAuthAuthorizeResponse {
        authorize_url,
        state,
    })
}

/// Handle an OAuth2 callback: exchange code for token, fetch identity, create session.
///
/// Callers must verify + consume `state` via [`verify_and_consume_state`]
/// before calling this — the handler performs the gate at the HTTP boundary.
pub async fn handle_oauth_callback(
    provider: &str,
    code: &str,
    state: &str,
    redirect_uri: &str,
) -> Result<OAuthCallbackResponse, String> {
    let _ = state;

    let config = oauth_config_from_env(provider, redirect_uri)?;
    let (access_token, orcid_id) = config.exchange_code(code).await?;
    let (provider_user_id, username) = config
        .fetch_identity(&access_token, orcid_id.as_deref())
        .await?;

    let session_token = generate_token();

    Ok(OAuthCallbackResponse {
        token: session_token,
        provider_user_id,
        username,
        role: "user".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_authenticate_password_equals_username_rejected_in_production() {
        // Without OX_FLOW_DEV_MODE=1, password==username MUST be rejected.
        // This test must pass in CI (where the env var is not set).
        if std::env::var("OXO_FLOW_DEV_MODE").as_deref() == Ok("1") {
            // Dev mode is enabled — skip this assertion
            return;
        }
        let result = authenticate("testuser", "testuser");
        assert!(
            result.is_err(),
            "password==username should be rejected without OX_FLOW_DEV_MODE=1"
        );
    }

    #[test]
    fn test_authenticate_invalid() {
        let result = authenticate("nobody", "wrongpassword");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_session_empty() {
        let sessions = vec![];
        let result = validate_session("", &sessions).unwrap();
        assert!(!result.authenticated);
    }

    #[test]
    fn test_validate_session_unknown_token_rejected() {
        // Unknown tokens are rejected — no more dev-mode fallback
        let sessions = vec![];
        let result = validate_session("some-token", &sessions).unwrap();
        assert!(!result.authenticated);
    }

    #[test]
    fn test_validate_session_valid_token() {
        let sessions = vec![Session {
            token: "valid-token".into(),
            user_id: "admin".into(),
            created_at: "2024-01-01T00:00:00Z".into(),
            expires_at: "2099-12-31T23:59:59Z".into(),
        }];
        let result = validate_session("valid-token", &sessions).unwrap();
        assert!(result.authenticated);
        assert_eq!(result.username, Some("admin".into()));
    }

    #[test]
    fn test_validate_session_expired_token() {
        let sessions = vec![Session {
            token: "expired-token".into(),
            user_id: "admin".into(),
            created_at: "2020-01-01T00:00:00Z".into(),
            expires_at: "2020-01-02T00:00:00Z".into(),
        }];
        let result = validate_session("expired-token", &sessions).unwrap();
        assert!(!result.authenticated);
    }

    #[test]
    fn test_check_role_admin() {
        assert!(check_role("admin", "admin"));
        assert!(check_role("admin", "user"));
        assert!(check_role("admin", "viewer"));
        assert!(!check_role("user", "admin"));
        assert!(check_role("user", "user"));
        assert!(check_role("user", "viewer"));
        assert!(check_role("viewer", "viewer"));
    }

    #[test]
    fn test_license_status() {
        let status = license_status();
        assert!(status.valid);
        assert_eq!(status.license_type, Some("academic".into()));
    }
}
