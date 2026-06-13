//! Authentication handlers.
//!
//! Handles login, session validation, and license status checking.

use axum::{extract::Json, http::StatusCode, response::IntoResponse};
use serde::Deserialize;

use crate::{
    ApiError, AuthMeResponse, ErrorResponse, LicenseStatus, LoginRequest, LoginResponse,
    OXO_FLOW_CONFIG, check_credentials_db, check_license, db, extract_session,
    generate_session_token,
};

/// `POST /api/auth/login` — Authenticate and obtain a session token.
pub async fn login(Json(req): Json<LoginRequest>) -> Result<impl IntoResponse, ApiError> {
    let user = check_credentials_db(&req.username, &req.password)
        .await
        .ok_or_else(|| ApiError {
            status: StatusCode::UNAUTHORIZED,
            body: ErrorResponse {
                code: "AUTH_REQUIRED".to_string(),
                message: "Invalid credentials".to_string(),
                detail: Some("The username or password provided is incorrect.".to_string()),
                suggestion: None,
            },
        })?;

    let role = user.role.clone();
    let token = generate_session_token();

    // Set expiration to 24 hours from now
    let expires_at = chrono::Utc::now() + chrono::Duration::hours(24);

    let session = db::Session {
        token: token.clone(),
        user_id: user.id.clone(),
        created_at: chrono::Utc::now(),
        expires_at,
    };

    db::create_session(&session).await.map_err(|e| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        body: ErrorResponse {
            code: "INTERNAL_ERROR".to_string(),
            message: "Failed to create session".to_string(),
            detail: Some(e.to_string()),
            suggestion: None,
        },
    })?;

    let secure_flag = if std::env::var("OXO_FLOW_DEV_MODE").is_ok() {
        ""
    } else {
        "; Secure"
    };
    let cookie = format!(
        "oxo_session={}; HttpOnly; Path=/; Max-Age=86400; SameSite=Strict{}",
        token, secure_flag
    );

    Ok((
        StatusCode::OK,
        [("set-cookie", cookie)],
        Json(LoginResponse {
            token,
            username: user.username,
            role,
        }),
    ))
}

/// `GET /api/auth/me` — Return the identity of the current session.
pub async fn auth_me(headers: axum::http::HeaderMap) -> Json<AuthMeResponse> {
    match extract_session(&headers).await {
        Some(session) => {
            if let Ok(Some(user)) = db::get_user_by_id(&session.user_id).await {
                Json(AuthMeResponse {
                    authenticated: true,
                    username: Some(user.username),
                    role: Some(user.role),
                })
            } else {
                Json(AuthMeResponse {
                    authenticated: false,
                    username: None,
                    role: None,
                })
            }
        }
        None => Json(AuthMeResponse {
            authenticated: false,
            username: None,
            role: None,
        }),
    }
}

/// `GET /api/license` — Return current license status.
pub async fn license_status() -> Json<LicenseStatus> {
    Json(check_license())
}

/// Request body for license upload.
#[derive(Debug, Deserialize)]
pub struct LicenseUploadRequest {
    pub license_json: String,
}

/// `POST /api/license/upload` — Upload and verify a license file.
pub async fn upload_license(
    Json(req): Json<LicenseUploadRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Parse the license JSON
    let license_file: oxo_license::LicenseFile = serde_json::from_str(&req.license_json)
        .map_err(|e| ApiError::bad_request("Invalid license JSON", Some(e.to_string())))?;

    // Verify the license
    oxo_license::verify_license(&license_file, &OXO_FLOW_CONFIG)
        .map_err(|e| ApiError::bad_request("License verification failed", Some(e.to_string())))?;

    // Save license to ~/.config/oxo-flow/
    let config_dir = std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".config").join("oxo-flow"))
        .unwrap_or_else(|_| std::path::PathBuf::from("."));

    let _ = std::fs::create_dir_all(&config_dir);
    let license_path = config_dir.join(OXO_FLOW_CONFIG.license_filename);
    std::fs::write(&license_path, &req.license_json)
        .map_err(|e| ApiError::unprocessable("Failed to save license file", Some(e.to_string())))?;

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "verified",
            "license_type": license_file.payload.license_type,
            "issued_to": license_file.payload.issued_to_org,
            "message": "License verified and installed to ~/.config/oxo-flow/. Restart to apply."
        })),
    ))
}
