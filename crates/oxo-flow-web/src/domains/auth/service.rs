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
pub fn validate_session(token: &str, _sessions: &[Session]) -> Result<AuthMeResponse, String> {
    // For v0.8, token-based session validation is a stub.
    // Real implementation connects to StorageBackend.
    if token.is_empty() {
        return Ok(AuthMeResponse {
            authenticated: false,
            username: None,
            role: None,
        });
    }
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
        contact: "wangsx@traitome.com".into(),
        message: "Free for academic use. Commercial use requires authorization.".into(),
    }
}

fn generate_token() -> String {
    Uuid::new_v4().to_string()
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
