//! HTTP handlers for auth domain.
//!
//! Thin adapters: parse HTTP request → call service → serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

use axum::{Json, http::StatusCode};

use super::service;
use super::types::*;
use crate::domains::workflow::handlers::ApiError;
use crate::infra::db::models;

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

fn err(s: StatusCode, c: &str, m: String) -> (StatusCode, Json<ApiError>) {
    (
        s,
        Json(ApiError {
            code: c.into(),
            message: m,
            detail: None,
            suggestion: None,
        }),
    )
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn get_pool() -> Result<&'static sqlx::SqlitePool, (StatusCode, Json<ApiError>)> {
    crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })
}

/// Extract the Bearer token from request headers.
fn extract_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

/// Validate a session token and return (username, role) if valid.
async fn validate_token(
    pool: &sqlx::SqlitePool,
    token: &str,
) -> Result<(String, String), (StatusCode, Json<ApiError>)> {
    if token.is_empty() {
        return Err(err(
            StatusCode::UNAUTHORIZED,
            "AUTH_REQUIRED",
            "Authentication required".into(),
        ));
    }

    let session: Option<models::SessionRow> =
        sqlx::query_as("SELECT * FROM sessions WHERE token = ?")
            .bind(token)
            .fetch_optional(pool)
            .await
            .map_err(|e| {
                tracing::error!("DB error looking up session token: {e}");
                err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "DB_ERROR",
                    "Internal database error".into(),
                )
            })?;

    let session = session.ok_or_else(|| {
        err(
            StatusCode::UNAUTHORIZED,
            "INVALID_TOKEN",
            "Invalid or expired session token".into(),
        )
    })?;

    // Check expiry
    let now = chrono::Utc::now().to_rfc3339();
    if session.expires_at <= now {
        return Err(err(
            StatusCode::UNAUTHORIZED,
            "TOKEN_EXPIRED",
            "Session token has expired".into(),
        ));
    }

    // Look up the user's role
    let user: Option<models::UserRow> = sqlx::query_as("SELECT * FROM users WHERE username = ?")
        .bind(&session.user_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| {
            tracing::error!("DB error looking up user role: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?;

    let role = user.map(|u| u.role).unwrap_or_else(|| "user".to_string());

    Ok((session.user_id, role))
}

/// Require admin role — returns 403 if the caller is not an admin.
///
/// Personal mode is a single-user, localhost-bound deployment where the rest
/// of the API surface is unauthenticated; management endpoints follow the
/// same trust model instead of being permanently unusable (issue #79 P1-06:
/// create-user returned 401 forever in personal mode). Team/HPC modes keep
/// full session + role enforcement.
async fn require_admin(
    headers: &axum::http::HeaderMap,
) -> Result<String, (StatusCode, Json<ApiError>)> {
    if crate::server::running_mode() == "personal" {
        return Ok("local_user".into());
    }

    let token = extract_token(headers).ok_or_else(|| {
        err(
            StatusCode::UNAUTHORIZED,
            "AUTH_REQUIRED",
            "Authentication required".into(),
        )
    })?;

    let pool = get_pool()?;
    let (username, role) = validate_token(pool, &token).await?;

    if role != "admin" {
        return Err(err(
            StatusCode::FORBIDDEN,
            "ACCESS_DENIED",
            "Admin role required for this operation".into(),
        ));
    }

    Ok(username)
}

/// POST /api/auth/login
pub async fn login(Json(req): Json<LoginRequest>) -> ApiResult<LoginResponse> {
    // Env-var credentials first (admin/user/viewer passwords); on failure,
    // fall back to DB-created accounts (bcrypt hash in users.password_hash,
    // issue #79 P1-06 — users created via the API must be able to sign in).
    let result = match service::authenticate(&req.username, &req.password) {
        Ok(response) => response,
        Err(env_err) => {
            let pool = get_pool()?;
            let user: Option<(String, Option<String>)> =
                sqlx::query_as("SELECT role, password_hash FROM users WHERE username = ?")
                    .bind(&req.username)
                    .fetch_optional(pool)
                    .await
                    .map_err(|e| {
                        tracing::error!("DB error looking up user '{}': {e}", req.username);
                        err(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "DB_ERROR",
                            "Internal database error".into(),
                        )
                    })?;
            match user {
                Some((role, Some(hash))) if service::verify_db_password(&req.password, &hash) => {
                    service::login_response_for(req.username.clone(), role)
                }
                _ => return Err(err(StatusCode::UNAUTHORIZED, "AUTH_FAILED", env_err)),
            }
        }
    };

    // Persist session to database so the token is actually valid.
    // Without this, require_auth and auth_me would reject the token.
    if let Ok(pool) = get_pool() {
        // Auto-provision a users row for env-password logins (issue #82
        // P1-16): those previously accepted ANY username with no user
        // record, collapsing every login onto the 'default' pseudo-user
        // and making audit trails useless. id = username for these
        // legacy identities; UNIQUE(username) makes the insert a no-op
        // for API-created accounts (which already have a UUID row).
        let _ = sqlx::query(
            "INSERT OR IGNORE INTO users (id, username, role, auth_type, os_user, created_at) \
             VALUES (?, ?, ?, 'password', '', ?)",
        )
        .bind(&result.username)
        .bind(&result.username)
        .bind(&result.role)
        .bind(now_iso())
        .execute(pool)
        .await;

        let expires = chrono::Utc::now() + chrono::Duration::hours(24);
        let insert_result = sqlx::query(
            "INSERT OR REPLACE INTO sessions (token, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
        )
        .bind(&result.token)
        .bind(&result.username)
        .bind(now_iso())
        .bind(expires.to_rfc3339())
        .execute(pool)
        .await;

        if let Err(e) = insert_result {
            tracing::error!(
                "Failed to persist session for user '{}': {e}",
                result.username
            );
        }
    } else {
        tracing::error!(
            "DB pool unavailable — session NOT persisted for user '{}'",
            result.username
        );
    }

    Ok(Json(result))
}

/// GET /api/auth/me
pub async fn auth_me(headers: axum::http::HeaderMap) -> ApiResult<AuthMeResponse> {
    let token = extract_token(&headers).unwrap_or_default();

    if token.is_empty() {
        return Ok(Json(AuthMeResponse {
            authenticated: false,
            username: None,
            role: None,
        }));
    }

    // Validate against sessions table
    if let Ok(pool) = get_pool() {
        match validate_token(pool, &token).await {
            Ok((username, role)) => {
                return Ok(Json(AuthMeResponse {
                    authenticated: true,
                    username: Some(username),
                    role: Some(role),
                }));
            }
            Err(_) => {
                return Ok(Json(AuthMeResponse {
                    authenticated: false,
                    username: None,
                    role: None,
                }));
            }
        }
    }

    Ok(Json(AuthMeResponse {
        authenticated: false,
        username: None,
        role: None,
    }))
}

/// GET /api/users — admin only
pub async fn list_users(headers: axum::http::HeaderMap) -> ApiResult<Vec<UserResponse>> {
    let _admin = require_admin(&headers).await?;
    let pool = get_pool()?;

    let rows: Vec<models::UserRow> = sqlx::query_as("SELECT * FROM users ORDER BY created_at ASC")
        .fetch_all(pool)
        .await
        .map_err(|e| {
            tracing::error!("DB error listing users: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?;

    let users: Vec<UserResponse> = rows
        .into_iter()
        .map(|r| UserResponse {
            id: r.id,
            username: r.username,
            role: r.role,
            auth_type: Some(r.auth_type),
            os_user: r.os_user,
            created_at: r.created_at,
        })
        .collect();

    Ok(Json(users))
}

/// POST /api/users — admin only
pub async fn create_user(
    headers: axum::http::HeaderMap,
    Json(req): Json<CreateUserRequest>,
) -> ApiResult<UserResponse> {
    let _admin = require_admin(&headers).await?;
    let pool = get_pool()?;

    let role = req.role.as_deref().unwrap_or("user");
    if !matches!(role, "admin" | "user" | "viewer") {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "INVALID_ROLE",
            format!("Invalid role '{role}' — must be admin, user, or viewer"),
        ));
    }

    // The account must be able to sign in: hash the password (bcrypt) into
    // users.password_hash, verified by the login handler's DB fallback.
    let password_hash = req
        .password
        .as_deref()
        .map(service::hash_password)
        .transpose()
        .map_err(|e| err(StatusCode::BAD_REQUEST, "BAD_PASSWORD", e))?;

    let id = uuid::Uuid::new_v4().to_string();
    let now = now_iso();

    sqlx::query(
        "INSERT INTO users (id, username, role, auth_type, os_user, password_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&req.username)
    .bind(role)
    .bind("password")
    // The legacy users schema declares os_user NOT NULL; password accounts
    // have no OS-level user, so store the empty string.
    .bind("")
    .bind(password_hash)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|e| {
        tracing::error!("DB error creating user: {e}");
        err(
            StatusCode::CONFLICT,
            "DB_ERROR",
            "Failed to create user (duplicate username or invalid role)".into(),
        )
    })?;

    Ok(Json(UserResponse {
        id,
        username: req.username,
        role: role.to_string(),
        auth_type: Some("password".to_string()),
        os_user: None,
        created_at: now,
    }))
}

/// DELETE /api/users/{id} — admin only
pub async fn delete_user(
    headers: axum::http::HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> ApiResult<serde_json::Value> {
    let _admin = require_admin(&headers).await?;
    let pool = get_pool()?;

    let existing: Option<models::UserRow> = sqlx::query_as("SELECT * FROM users WHERE id = ?")
        .bind(&id)
        .fetch_optional(pool)
        .await
        .map_err(|e| {
            tracing::error!("DB error fetching user {id} for deletion: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?;

    if existing.is_none() {
        return Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("User {id} not found"),
        ));
    }

    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(&id)
        .execute(pool)
        .await
        .map_err(|e| {
            tracing::error!("DB error deleting user {id}: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?;

    Ok(Json(serde_json::json!({"deleted": id})))
}

/// GET /api/license
pub async fn license_status() -> ApiResult<LicenseResponse> {
    Ok(Json(service::license_status()))
}

// ---------------------------------------------------------------------------
// OAuth2 handlers
// ---------------------------------------------------------------------------

/// POST /api/auth/oauth/authorize
///
/// Initiates an OAuth2 authorization flow. Returns the provider's
/// authorization URL that the user should be redirected to.
pub async fn oauth_authorize(
    Json(req): Json<OAuthAuthorizeRequest>,
) -> ApiResult<OAuthAuthorizeResponse> {
    let redirect_uri = req
        .redirect_uri
        .as_deref()
        .unwrap_or("http://localhost:3000/api/auth/oauth/callback");

    super::service::initiate_oauth(&req.provider, redirect_uri)
        .await
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "OAUTH_ERROR", e))
}

/// POST /api/auth/oauth/callback
///
/// Handles the OAuth2 callback after the user authorizes the application.
/// Exchanges the authorization code for an access token and creates a session.
pub async fn oauth_callback(
    Json(req): Json<OAuthCallbackRequest>,
) -> ApiResult<OAuthCallbackResponse> {
    let provider = req.provider.as_deref().unwrap_or("orcid");
    let redirect_uri = std::env::var("OXO_FLOW_OAUTH_REDIRECT_URI")
        .unwrap_or_else(|_| "http://localhost:3000/api/auth/oauth/callback".to_string());

    // Verify CSRF state — the state from the callback must match what we issued.
    // In production, the state should be stored server-side (e.g. in the sessions table
    // or a short-lived cache) at authorization time and verified here.
    // For now we validate that state is non-empty; a full implementation would
    // store pending states with a TTL.
    if req.state.is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "OAUTH_INVALID_STATE",
            "Missing CSRF state parameter".into(),
        ));
    }
    // Verify + consume the CSRF state issued at authorization time, BEFORE
    // any token exchange (single use).
    super::service::verify_and_consume_state(&req.state)
        .await
        .map_err(|e| err(StatusCode::BAD_REQUEST, "OAUTH_INVALID_STATE", e))?;
    tracing::debug!(
        oauth_provider = provider,
        "OAuth callback state verified against the stored pending request"
    );

    let result =
        super::service::handle_oauth_callback(provider, &req.code, &req.state, &redirect_uri)
            .await
            .map_err(|e| err(StatusCode::BAD_REQUEST, "OAUTH_CALLBACK_ERROR", e))?;

    // Persist the session
    if let Ok(pool) = get_pool() {
        let expires = chrono::Utc::now() + chrono::Duration::hours(24);
        let insert_result = sqlx::query(
            "INSERT OR REPLACE INTO sessions (token, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
        )
        .bind(&result.token)
        .bind(&result.username)
        .bind(now_iso())
        .bind(expires.to_rfc3339())
        .execute(pool)
        .await;

        if let Err(e) = insert_result {
            tracing::error!(
                "Failed to persist OAuth session for user '{}': {e}",
                result.username
            );
        }
    } else {
        tracing::error!(
            "DB pool unavailable — OAuth session NOT persisted for user '{}'",
            result.username
        );
    }

    Ok(Json(result))
}

/// POST /api/license/upload
pub async fn upload_license(Json(req): Json<serde_json::Value>) -> ApiResult<LicenseResponse> {
    // Log the upload attempt
    if let Ok(pool) = get_pool() {
        let license_data = req
            .get("license_data")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !license_data.is_empty() {
            let _ = sqlx::query(
                "INSERT INTO audit_logs (id, user_id, action, target, metadata, timestamp) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind("system")
            .bind("upload_license")
            .bind("license")
            .bind(Some(license_data))
            .bind(now_iso())
            .execute(pool)
            .await;
        }
    }

    Ok(Json(service::license_status()))
}
