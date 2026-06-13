use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub token: String,
    pub user_id: String,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    pub username: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthMeResponse {
    pub authenticated: bool,
    pub username: Option<String>,
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserResponse {
    pub id: String,
    pub username: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_user: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub role: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseResponse {
    pub valid: bool,
    pub license_type: Option<String>,
    pub issued_to: Option<String>,
    pub commercial_use: String,
    pub contact: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_login_request_roundtrip() {
        let req = LoginRequest {
            username: "admin".into(),
            password: "secret".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: LoginRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.username, req.username);
    }

    #[test]
    fn test_login_response_roundtrip() {
        let resp = LoginResponse {
            token: "abc123".into(),
            username: "admin".into(),
            role: "admin".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: LoginResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.token, resp.token);
    }

    #[test]
    fn test_auth_me_response_roundtrip() {
        let resp = AuthMeResponse {
            authenticated: true,
            username: Some("admin".into()),
            role: Some("admin".into()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: AuthMeResponse = serde_json::from_str(&json).unwrap();
        assert!(back.authenticated);
    }

    #[test]
    fn test_user_response_roundtrip() {
        let resp = UserResponse {
            id: "u1".into(),
            username: "admin".into(),
            role: "admin".into(),
            auth_type: Some("password".into()),
            os_user: None,
            created_at: "2024-01-01".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: UserResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, resp.id);
    }

    #[test]
    fn test_create_user_request_roundtrip() {
        let req = CreateUserRequest {
            username: "newuser".into(),
            role: Some("viewer".into()),
            password: Some("pass".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: CreateUserRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.username, req.username);
    }

    #[test]
    fn test_license_response_roundtrip() {
        let resp = LicenseResponse {
            valid: true,
            license_type: Some("MIT".into()),
            issued_to: Some("user".into()),
            commercial_use: "yes".into(),
            contact: "admin@example.com".into(),
            message: "license valid".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: LicenseResponse = serde_json::from_str(&json).unwrap();
        assert!(back.valid);
    }
}
