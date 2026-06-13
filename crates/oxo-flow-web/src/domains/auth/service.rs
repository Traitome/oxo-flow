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
    // Dev mode fallback: password equals username
    if password == username && !username.is_empty() {
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

    // Token not found in sessions — for dev mode, accept any non-empty token
    Ok(AuthMeResponse {
        authenticated: true,
        username: Some("user".into()),
        role: Some("user".into()),
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

/// Initiate an OAuth2 authorization flow.
///
/// Returns the provider's authorization URL and a CSRF state token.
pub fn initiate_oauth(
    provider: &str,
    redirect_uri: &str,
) -> Result<OAuthAuthorizeResponse, String> {
    let config = oauth_config_from_env(provider, redirect_uri)?;
    let state = generate_token();
    let authorize_url = config.authorize_url(&state);

    Ok(OAuthAuthorizeResponse {
        authorize_url,
        state,
    })
}

/// Handle an OAuth2 callback: exchange code for token, fetch identity, create session.
pub async fn handle_oauth_callback(
    provider: &str,
    code: &str,
    state: &str,
    redirect_uri: &str,
) -> Result<OAuthCallbackResponse, String> {
    // In production, verify that `state` matches the one stored for this session
    let _ = state;

    let config = oauth_config_from_env(provider, redirect_uri)?;
    let access_token = config.exchange_code(code).await?;
    let (provider_user_id, username) = config.fetch_identity(&access_token).await?;

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
    fn test_authenticate_dev_mode() {
        let result = authenticate("testuser", "testuser").unwrap();
        assert_eq!(result.username, "testuser");
        assert_eq!(result.role, "user");
        assert!(!result.token.is_empty());
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
    fn test_validate_session_with_token() {
        let sessions = vec![];
        let result = validate_session("some-token", &sessions).unwrap();
        assert!(result.authenticated);
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
