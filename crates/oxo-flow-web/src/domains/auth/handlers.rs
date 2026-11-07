//! HTTP handlers for auth domain.
//!
//! Thin adapters: parse HTTP request → call service → serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

use crate::extract::ApiJson;
use axum::{Extension, Json, http::StatusCode};

use super::service;
use super::types::*;
use crate::domains::auth::current_user;
use crate::domains::auth::current_user::{CurrentUser, resolve};
use crate::domains::workflow::handlers::{ApiError, db_err, err, now_iso};
use crate::infra::db::models;

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

/// Session lifetime for every minted session (login + OAuth callback),
/// issue #976. Configurable via `OXO_FLOW_SESSION_TTL_HOURS` so operators
/// can align it with their security posture; the default matches the
/// historical hardcoded 24 h. Same env-knob convention as the other
/// `OXO_FLOW_*` variables: unparseable, `0`, or above the 1-year cap falls
/// back to the default with a warning — a typo can never mint a
/// permanently-valid or instantly-dead session.
pub(crate) const DEFAULT_SESSION_TTL_HOURS: u64 = 24;
pub(crate) const MAX_SESSION_TTL_HOURS: u64 = 8760; // one year
pub(crate) const SESSION_TTL_ENV: &str = "OXO_FLOW_SESSION_TTL_HOURS";

pub(crate) fn session_ttl_hours() -> u64 {
    parse_session_ttl_hours(std::env::var(SESSION_TTL_ENV).ok().as_deref())
}

pub(crate) fn parse_session_ttl_hours(raw: Option<&str>) -> u64 {
    match raw.and_then(|v| v.trim().parse::<u64>().ok()) {
        Some(h) if (1..=MAX_SESSION_TTL_HOURS).contains(&h) => h,
        Some(_) => {
            tracing::warn!(
                "{} out of range (must be 1..={}) — falling back to the default {}h",
                SESSION_TTL_ENV,
                MAX_SESSION_TTL_HOURS,
                DEFAULT_SESSION_TTL_HOURS
            );
            DEFAULT_SESSION_TTL_HOURS
        }
        None => {
            if let Some(raw) = raw {
                tracing::warn!(
                    "{} is not a number ('{}') — falling back to the default {}h",
                    SESSION_TTL_ENV,
                    raw,
                    DEFAULT_SESSION_TTL_HOURS
                );
            }
            DEFAULT_SESSION_TTL_HOURS
        }
    }
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
async fn validate_token_with_backend(
    backend: &dyn crate::infra::db::StorageBackend,
    token: &str,
) -> Result<(String, String), (StatusCode, Json<ApiError>)> {
    if token.is_empty() {
        return Err(err(
            StatusCode::UNAUTHORIZED,
            "AUTH_REQUIRED",
            "Authentication required".into(),
        ));
    }

    let session = backend
        .get_session(token)
        .await
        .map_err(|e| db_err(format!("DB error looking up session token: {e}")))?;

    let session = session.ok_or_else(|| {
        err(
            StatusCode::UNAUTHORIZED,
            "INVALID_TOKEN",
            "Invalid or expired session token".into(),
        )
    })?;

    // Check expiry
    let now = now_iso();
    if session.expires_at <= now {
        return Err(err(
            StatusCode::UNAUTHORIZED,
            "TOKEN_EXPIRED",
            "Session token has expired".into(),
        ));
    }

    let user = backend
        .get_user_by_id(&session.user_id)
        .await
        .map_err(|e| db_err(format!("DB error looking up user role: {e}")))?;

    let user = user.ok_or_else(|| {
        err(
            StatusCode::UNAUTHORIZED,
            "INVALID_TOKEN",
            "Session is no longer valid; sign in again".into(),
        )
    })?;

    Ok((user.username, user.role))
}

/// Require admin role — returns 403 if the caller is not an admin.
///
/// Personal mode is a single-user, localhost-bound deployment where the rest
/// of the API surface is unauthenticated; management endpoints follow the
/// same trust model instead of being permanently unusable (issue #79 P1-06:
/// create-user returned 401 forever in personal mode). Team/HPC modes keep
/// full session + role enforcement.
pub(crate) async fn require_admin(
    headers: &axum::http::HeaderMap,
) -> Result<String, (StatusCode, Json<ApiError>)> {
    if crate::server::running_mode() == "personal" {
        return Ok("local_user".into());
    }

    let backend = crate::infra::db::global_backend().ok_or_else(|| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;
    resolve_admin(backend.as_ref(), headers).await
}

/// Resolve an admin identity from either credential the router's
/// `require_auth` accepts: a Bearer session token or an `X-API-Key`.
///
/// API keys are first-class machine credentials (issue #82 P1-13), so an
/// admin's key must work on admin-only routes instead of 401ing.
async fn resolve_admin(
    backend: &dyn crate::infra::db::StorageBackend,
    headers: &axum::http::HeaderMap,
) -> Result<String, (StatusCode, Json<ApiError>)> {
    let (username, role) = if let Some(token) = extract_token(headers) {
        validate_token_with_backend(backend, &token).await?
    } else if let Some(key) = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .filter(|k| !k.is_empty())
    {
        let user = resolve_api_key_with_backend(backend, key)
            .await
            .ok_or_else(|| {
                err(
                    StatusCode::UNAUTHORIZED,
                    "INVALID_API_KEY",
                    "Invalid or revoked API key".into(),
                )
            })?;
        (user.id, user.role)
    } else {
        return Err(err(
            StatusCode::UNAUTHORIZED,
            "AUTH_REQUIRED",
            "Authentication required".into(),
        ));
    };

    if role != "admin" {
        return Err(err(
            StatusCode::FORBIDDEN,
            "ACCESS_DENIED",
            "Admin role required for this operation".into(),
        ));
    }

    Ok(username)
}

#[utoipa::path(
    post,
    path = "/api/auth/login",
    tag = "auth",
    responses(
        (status = 200, description = "Success", body = LoginResponse),
        (status = 401, description = "Error", body = ApiError),
    )
)]
/// POST /api/auth/login
///
/// Brute-force guard (issue #907 T5): failed attempts are throttled per
/// (account, source IP) before any credential check runs. The peer address
/// comes from `ConnectInfo` when the transport supplies one (the real
/// server does); in-process routers (tests) fall back to a shared
/// `"unknown"` bucket — mirroring `rate_limit::client_key` — which headers
/// cannot rotate.
pub async fn login(
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    ApiJson(req): ApiJson<LoginRequest>,
) -> ApiResult<LoginResponse> {
    use crate::rate_limit::{check_login_attempt, clear_login_failures, record_login_failure};

    let ip = peer
        .as_ref()
        .map(|ci| ci.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    check_login_attempt(&req.username, &ip).map_err(|retry_after| {
        // Mirrors rate_limit_middleware's 429 shape: the countdown lives in
        // `detail` (the ApiResult tuple cannot carry a Retry-After header).
        (
            StatusCode::TOO_MANY_REQUESTS,
            Json(ApiError {
                code: "RATE_LIMITED".into(),
                message: "Too many failed login attempts".into(),
                detail: Some(format!("retry in {retry_after}s")),
                suggestion: Some("Wait for the lockout to expire before retrying".into()),
            }),
        )
    })?;

    // The username is persisted as the users row id and becomes a workspace
    // path component — reject unsafe usernames before any lookup or
    // auto-provisioning (issue: traversal usernames escaped the user tree).
    crate::workspace::validate_username(&req.username).map_err(|e| {
        record_login_failure(&req.username, &ip);
        err(
            StatusCode::UNAUTHORIZED,
            "INVALID_USERNAME",
            format!("{e} — login rejected"),
        )
    })?;
    // Env-var credentials first (admin/user/viewer passwords); on failure,
    // fall back to DB-created accounts (bcrypt hash in users.password_hash,
    // issue #79 P1-06 — users created via the API must be able to sign in).
    //
    // The chosen path also fixes the identity the session may carry (#516):
    // a bcrypt-verified account adopts its canonical users.id, while an
    // env-password login is bound to the provisioned row keyed
    // id == username and must never adopt a UUID-keyed managed account —
    // otherwise the shared user password could impersonate any existing
    // user (including an admin) just by naming them.
    let backend = crate::infra::db::global_backend().ok_or_else(|| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    let (session_user_id, result) = match service::authenticate(&req.username, &req.password) {
        Ok(response) => {
            if result_is_admin_identity(&response) {
                let row = backend
                    .get_user_by_username("admin")
                    .await
                    .map_err(|e| db_err(format!("DB error resolving admin identity: {e}")))?;
                let row = row.ok_or_else(|| {
                    err(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "DB_ERROR",
                        "Admin bootstrap row missing".into(),
                    )
                })?;
                let response = LoginResponse {
                    role: row.role,
                    ..response
                };
                (row.id, response)
            } else {
                let row = backend
                    .ensure_env_user(&response.username, &response.role)
                    .await
                    .map_err(|e| {
                        db_err(
                            format!(
                                "DB error resolving env identity '{}': {e}",
                                response.username
                            )
                            .as_str(),
                        )
                    })?;
                let (id, role) = row.ok_or_else(|| {
                    record_login_failure(&req.username, &ip);
                    err(
                        StatusCode::UNAUTHORIZED,
                        "AUTH_FAILED",
                        "This username is a managed account; sign in with its own password".into(),
                    )
                })?;
                let response = LoginResponse { role, ..response };
                (id, response)
            }
        }
        Err(env_err) => {
            let user = backend
                .get_user_by_username(&req.username)
                .await
                .map_err(|e| db_err(format!("DB error looking up user '{}': {e}", req.username)))?;
            match user {
                Some(u)
                    if u.password_hash
                        .as_deref()
                        .is_some_and(|hash| service::verify_db_password(&req.password, hash)) =>
                {
                    let response = service::login_response_for(req.username.clone(), u.role);
                    (u.id, response)
                }
                _ => {
                    record_login_failure(&req.username, &ip);
                    return Err(err(StatusCode::UNAUTHORIZED, "AUTH_FAILED", env_err));
                }
            }
        }
    };

    // Persist session to database so the token is actually valid.
    let expires = chrono::Utc::now() + chrono::Duration::hours(session_ttl_hours() as i64);
    backend
        .create_session(&session_user_id, &result.token, &expires.to_rfc3339())
        .await
        .map_err(|e| {
            tracing::error!("DB error creating session for user {session_user_id}: {e}");
            err(
                StatusCode::SERVICE_UNAVAILABLE,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?;

    // A successful login resets the brute-force counter for this
    // (account, ip) bucket — one typo must not poison the next attempt.
    clear_login_failures(&req.username, &ip);

    Ok(Json(result))
}

/// True when an env-password login response carries the admin identity —
/// only the ADMIN_PASSWORD branch can produce it (service::authenticate
/// rejects "admin" on the shared user/viewer branches, #516).
fn result_is_admin_identity(response: &LoginResponse) -> bool {
    response.username == "admin" && response.role == "admin"
}

#[utoipa::path(
    get,
    path = "/api/auth/me",
    tag = "auth",
    responses(
        (status = 200, description = "Success", body = AuthMeResponse),
        (status = 401, description = "Error", body = ApiError),
    )
)]
/// GET /api/auth/me
pub async fn auth_me(headers: axum::http::HeaderMap) -> ApiResult<AuthMeResponse> {
    let response = match crate::infra::db::global_backend() {
        Some(backend) => auth_me_in(backend.as_ref(), &headers).await,
        None => AuthMeResponse {
            authenticated: false,
            username: None,
            role: None,
        },
    };
    Ok(Json(response))
}

/// Storage-backed core of [`auth_me`] (testable against an in-memory backend).
async fn auth_me_in(
    backend: &dyn crate::infra::db::StorageBackend,
    headers: &axum::http::HeaderMap,
) -> AuthMeResponse {
    if let Some(key) = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .filter(|k| !k.is_empty())
        && let Some(user) = resolve_api_key_with_backend(backend, key).await
    {
        return AuthMeResponse {
            authenticated: true,
            username: Some(user.id),
            role: Some(user.role),
        };
    }

    let token = extract_token(headers).unwrap_or_default();

    if !token.is_empty()
        && let Ok((username, role)) = validate_token_with_backend(backend, &token).await
    {
        return AuthMeResponse {
            authenticated: true,
            username: Some(username),
            role: Some(role),
        };
    }

    AuthMeResponse {
        authenticated: false,
        username: None,
        role: None,
    }
}

#[utoipa::path(
    post,
    path = "/api/auth/logout",
    tag = "auth",
    security(("bearerAuth" = [])),
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
    )
)]
/// POST /api/auth/logout — revoke the presented session (#520).
///
/// Sessions used to be irrevocable until their 24-hour expiry; a stolen
/// token kept working after the user "signed out". The server deletes the
/// session row; the client drops its local copy.
pub async fn logout(headers: axum::http::HeaderMap) -> ApiResult<serde_json::Value> {
    let Some(token) = extract_token(&headers).filter(|t| !t.is_empty()) else {
        return Ok(Json(serde_json::json!({"logged_out": false})));
    };
    if let Some(backend) = crate::infra::db::global_backend() {
        match backend.delete_session(&token).await {
            Ok(()) => {
                tracing::info!("logout revoked token");
            }
            Err(e) => {
                tracing::error!("DB error deleting session on logout: {e}");
            }
        }
    }
    Ok(Json(serde_json::json!({"logged_out": true})))
}

#[utoipa::path(
    get,
    path = "/api/users",
    tag = "auth",
    security(("bearerAuth" = [])),
    responses(
        (status = 200, description = "Success", body = Vec<UserResponse>),
        (status = 403, description = "Error", body = ApiError),
    )
)]
/// GET /api/users — admin only
pub async fn list_users(headers: axum::http::HeaderMap) -> ApiResult<Vec<UserResponse>> {
    let _admin = require_admin(&headers).await?;
    let backend = crate::infra::db::global_backend().ok_or_else(|| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    let rows = backend
        .list_users()
        .await
        .map_err(|e| db_err(format!("DB error listing users: {e}")))?;

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

#[utoipa::path(
    post,
    path = "/api/users",
    tag = "auth",
    security(("bearerAuth" = [])),
    responses(
        (status = 200, description = "Success", body = UserResponse),
        (status = 403, description = "Error", body = ApiError),
    )
)]
/// POST /api/users — admin only
pub async fn create_user(
    headers: axum::http::HeaderMap,
    ApiJson(req): ApiJson<CreateUserRequest>,
) -> ApiResult<UserResponse> {
    let _admin = require_admin(&headers).await?;
    // The username becomes the workspace path component — same rule as
    // login auto-provisioning.
    crate::workspace::validate_username(&req.username)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "INVALID_USERNAME", format!("{e}")))?;
    let backend = crate::infra::db::global_backend().ok_or_else(|| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

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

    let row = backend
        .create_user(&req.username, role, password_hash.as_deref())
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
        id: row.id,
        username: row.username,
        role: row.role,
        auth_type: Some(row.auth_type),
        os_user: row.os_user,
        created_at: row.created_at,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/users/{id}",
    tag = "auth",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 403, description = "Error", body = ApiError),
    )
)]
/// DELETE /api/users/{id} — admin only
pub async fn delete_user(
    headers: axum::http::HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> ApiResult<serde_json::Value> {
    let _admin = require_admin(&headers).await?;
    let backend = crate::infra::db::global_backend().ok_or_else(|| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    let existing = backend
        .get_user_by_id(&id)
        .await
        .map_err(|e| db_err(format!("DB error fetching user {id} for deletion: {e}")))?;

    if existing.is_none() {
        return Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("User {id} not found"),
        ));
    }

    backend
        .delete_user(&id)
        .await
        .map_err(|e| db_err(format!("DB error deleting user {id}: {e}")))?;

    // Live credentials (#520: sessions and API keys) are cascaded directly
    // within StorageBackend::delete_user across all supported engines.
    Ok(Json(serde_json::json!({"deleted": id})))
}

#[utoipa::path(
    get,
    path = "/api/license",
    tag = "license",
    responses(
        (status = 200, description = "Success", body = LicenseResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// GET /api/license
pub async fn license_status() -> ApiResult<LicenseResponse> {
    Ok(Json(service::license_status()))
}

// ---------------------------------------------------------------------------
// OAuth2 handlers
// ---------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/api/auth/oauth/authorize",
    tag = "auth",
    responses(
        (status = 200, description = "Success", body = OAuthAuthorizeResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/auth/oauth/authorize
///
/// Initiates an OAuth2 authorization flow. Returns the provider's
/// authorization URL that the user should be redirected to.
pub async fn oauth_authorize(
    ApiJson(req): ApiJson<OAuthAuthorizeRequest>,
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

#[utoipa::path(
    post,
    path = "/api/auth/oauth/callback",
    tag = "auth",
    responses(
        (status = 200, description = "Success", body = OAuthCallbackResponse),
        (status = 400, description = "Error", body = ApiError),
        (status = 503, description = "User row could not be ensured (database failure)", body = ApiError),
    )
)]
/// POST /api/auth/oauth/callback
///
/// Handles the OAuth2 callback after the user authorizes the application.
/// Exchanges the authorization code for an access token and creates a session.
pub async fn oauth_callback(
    ApiJson(req): ApiJson<OAuthCallbackRequest>,
) -> ApiResult<OAuthCallbackResponse> {
    let provider = req.provider.as_deref().unwrap_or("orcid");
    let redirect_uri = std::env::var("OXO_FLOW_OAUTH_REDIRECT_URI")
        .unwrap_or_else(|_| "http://localhost:3000/api/auth/oauth/callback".to_string());

    // Verify CSRF state: the state issued at authorization time was
    // persisted server-side (`store_pending_state`), and here it is
    // consumed single-use — before any token exchange — after an
    // `OAUTH_STATE_TTL_MINS` (10-minute) freshness window.
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

    // Persist the session. The OAuth identity is namespaced (#516): the
    // provider-supplied username is attacker-chosen, so it must never equal
    // — or resolve to — a local users.username (e.g. "admin"). Both the
    // users.id and the username carry provider prefixes; the session binds
    // to the namespaced id, and role resolution only ever reads that row.
    // The user row MUST exist before the session is persisted: two provider
    // accounts can share a display name, and a silently skipped insert left
    // the session pointing at a nonexistent user (every request 401'd).
    let backend = crate::infra::db::global_backend().ok_or_else(|| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;
    let oauth_user_id = format!("oauth:{provider}:{}", result.provider_user_id);
    let oauth_username = format!("{provider}:{}", result.username);
    let fallback_username = format!("{provider}:{}", result.provider_user_id);
    let stored_username = backend
        .ensure_oauth_user(&oauth_user_id, &oauth_username, &fallback_username)
        .await
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, "DB_ERROR", e))?;
    if stored_username != oauth_username {
        // Display name was taken — the row carries the unique fallback and
        // the response field stays the provider display name (display only).
        tracing::info!(
            "OAuth username '{}' already taken; persisted identity '{}' as '{}'",
            oauth_username,
            oauth_user_id,
            stored_username
        );
    }

    let expires = chrono::Utc::now() + chrono::Duration::hours(session_ttl_hours() as i64);
    // Same contract as login: a session that cannot be persisted must not
    // come back as 200 + a dead token — every subsequent request would 401
    // with no hint the server was at fault. Answer 503 so the client knows
    // the callback failed server-side.
    let backend = crate::infra::db::global_backend().ok_or_else(|| {
        tracing::error!(
            "Failed to persist OAuth session for user '{}': database backend unavailable",
            result.username
        );
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;
    backend
        .create_session(&oauth_user_id, &result.token, &expires.to_rfc3339())
        .await
        .map_err(|e| {
            tracing::error!(
                "Failed to persist OAuth session for user '{}': {e}",
                result.username
            );
            err(
                StatusCode::SERVICE_UNAVAILABLE,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?;

    Ok(Json(result))
}

#[utoipa::path(
    post,
    path = "/api/license/upload",
    tag = "license",
    responses(
        (status = 200, description = "Success", body = LicenseResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/license/upload
///
/// Admin-only outside personal mode; the payload is bounded and the raw
/// blob is NOT persisted (#519) — the endpoint never processes a license
/// (license_status() is static), so storing unvalidated attacker-sized
/// strings in audit metadata was pure liability.
pub async fn upload_license(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(req): ApiJson<serde_json::Value>,
) -> ApiResult<LicenseResponse> {
    let user = resolve(authenticated.as_ref());
    current_user::require_role_outside_personal(
        &user,
        "admin",
        "Admin role required for license operations",
    )?;
    const MAX_LICENSE_BYTES: usize = 64 * 1024;
    let license_data = req
        .get("license_data")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if license_data.len() > MAX_LICENSE_BYTES {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "LICENSE_TOO_LARGE",
            format!(
                "license_data exceeds the {} KiB bound",
                MAX_LICENSE_BYTES / 1024
            ),
        ));
    }
    if !license_data.is_empty()
        && let Some(backend) = crate::infra::db::global_backend()
    {
        use std::fmt::Write as _;

        use sha2::{Digest, Sha256};
        let mut metadata = String::new();
        let digest = Sha256::digest(license_data.as_bytes());
        let mut hex = String::with_capacity(64);
        for byte in digest {
            let _ = write!(hex, "{byte:02x}");
        }
        let _ = write!(
            metadata,
            "{{\"bytes\":{},\"sha256\":\"{hex}\"}}",
            license_data.len(),
        );
        if let Err(error) = backend
            .log_action_with_metadata(&user.id, "upload_license", "license", Some(&metadata))
            .await
        {
            tracing::error!(
                user_id = %user.id,
                error = %error,
                "license event could not be written to the audit log"
            );
        }
    }

    Ok(Json(service::license_status()))
}

// ---------------------------------------------------------------------------
// API keys (issue #82 P1-13) — machine credentials, stored hashed,
// individually revocable. The plaintext is shown exactly once.
// ---------------------------------------------------------------------------

fn hash_api_key(key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[utoipa::path(
    post,
    path = "/api/auth/keys",
    tag = "auth",
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Invalid request", body = ApiError),
        (status = 401, description = "Error", body = ApiError),
    )
)]
/// POST /api/auth/keys — generate a new API key for the acting user.
pub async fn create_api_key(
    authenticated: Option<axum::Extension<crate::domains::auth::current_user::CurrentUser>>,
    ApiJson(req): ApiJson<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    let user = crate::domains::auth::current_user::resolve(authenticated.as_ref());
    let name = req
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unnamed")
        .trim()
        .to_string();
    if name.is_empty() || name.len() > 64 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "INVALID_NAME",
            "name must be 1-64 characters".into(),
        ));
    }
    let backend = crate::infra::db::global_backend().ok_or_else(|| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;
    let plaintext = format!("oxo_{}", uuid::Uuid::new_v4().simple());
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_iso();
    let key_row = models::ApiKeyRow {
        id: id.clone(),
        user_id: user.id.clone(),
        name: name.clone(),
        key_hash: hash_api_key(&plaintext),
        created_at: now,
        last_used_at: None,
        revoked: 0,
    };
    backend
        .create_api_key(&key_row)
        .await
        .map_err(|e| db_err(format!("DB error creating API key: {e}")))?;

    Ok(Json(serde_json::json!({
        "id": id,
        "name": name,
        "key": plaintext,
    })))
}

#[utoipa::path(
    get,
    path = "/api/auth/keys",
    tag = "auth",
    responses(
        (status = 200, description = "Success", body = Vec<serde_json::Value>),
        (status = 401, description = "Error", body = ApiError),
    )
)]
/// GET /api/auth/keys — the acting user's keys (hashes only, never
/// plaintext).
pub async fn list_api_keys(
    authenticated: Option<axum::Extension<crate::domains::auth::current_user::CurrentUser>>,
) -> ApiResult<Vec<serde_json::Value>> {
    let user = crate::domains::auth::current_user::resolve(authenticated.as_ref());
    let backend = crate::infra::db::global_backend().ok_or_else(|| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".to_string(),
        )
    })?;
    let rows = backend
        .list_api_keys(&user.id)
        .await
        .map_err(|e| db_err(format!("DB error listing API keys: {e}")))?;
    Ok(Json(
        rows.into_iter()
            .map(|k| {
                serde_json::json!({
                    "id": k.id,
                    "name": k.name,
                    "created_at": k.created_at,
                    "last_used_at": k.last_used_at,
                })
            })
            .collect(),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/auth/keys/{id}",
    tag = "auth",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 404, description = "Error", body = ApiError),
    )
)]
/// DELETE /api/auth/keys/{id} — revoke one of the acting user's keys.
pub async fn revoke_api_key(
    authenticated: Option<axum::Extension<crate::domains::auth::current_user::CurrentUser>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> ApiResult<serde_json::Value> {
    let user = crate::domains::auth::current_user::resolve(authenticated.as_ref());
    let backend = crate::infra::db::global_backend().ok_or_else(|| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".to_string(),
        )
    })?;
    let revoked = backend
        .revoke_api_key(&id, &user.id)
        .await
        .map_err(|e| db_err(format!("DB error revoking API key: {e}")))?;
    if !revoked {
        return Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "API key not found".into(),
        ));
    }
    Ok(Json(serde_json::json!({"revoked": id})))
}

/// Resolve an `X-API-Key` header to a CurrentUser, touching last_used_at.
/// Used by the auth middleware when no Bearer session is present.
pub async fn resolve_api_key(key: &str) -> Option<crate::domains::auth::current_user::CurrentUser> {
    let backend = crate::infra::db::global_backend()?;
    resolve_api_key_with_backend(backend.as_ref(), key).await
}

/// Backend-taking form of [`resolve_api_key`].
pub async fn resolve_api_key_with_backend(
    backend: &dyn crate::infra::db::StorageBackend,
    key: &str,
) -> Option<crate::domains::auth::current_user::CurrentUser> {
    let hash = hash_api_key(key);
    let now = now_iso();
    let (user_id, role) = backend.resolve_api_key(&hash, &now).await.ok()??;
    Some(crate::domains::auth::current_user::CurrentUser { id: user_id, role })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::infra::db::StorageBackend;

    /// A one-connection in-memory backend with just the tables `resolve_admin` touches.
    async fn admin_backend() -> crate::infra::db::sqlite::SqliteBackend {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory pool");
        sqlx::query(
            "CREATE TABLE users (id TEXT PRIMARY KEY, username TEXT UNIQUE NOT NULL, \
             role TEXT NOT NULL, auth_type TEXT NOT NULL, os_user TEXT, \
             password_hash TEXT, created_at TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE api_keys (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, \
             name TEXT NOT NULL, key_hash TEXT NOT NULL, created_at TEXT NOT NULL, \
             last_used_at TEXT, revoked INTEGER NOT NULL DEFAULT 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE sessions (token TEXT PRIMARY KEY, user_id TEXT NOT NULL, \
             created_at TEXT NOT NULL, expires_at TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        crate::infra::db::sqlite::SqliteBackend::from_pool(pool)
    }

    async fn seed_key(
        backend: &crate::infra::db::sqlite::SqliteBackend,
        user_id: &str,
        role: &str,
        key: &str,
    ) {
        let now = now_iso();
        sqlx::query(
            "INSERT INTO users (id, username, role, auth_type, created_at) \
             VALUES (?, ?, ?, 'password', ?)",
        )
        .bind(user_id)
        .bind(user_id)
        .bind(role)
        .bind(&now)
        .execute(backend.inner_pool())
        .await
        .unwrap();

        let key_row = models::ApiKeyRow {
            id: format!("k-{user_id}"),
            user_id: user_id.to_string(),
            name: "ci".to_string(),
            key_hash: hash_api_key(key),
            created_at: now,
            last_used_at: None,
            revoked: 0,
        };
        backend.create_api_key(&key_row).await.unwrap();
    }

    fn headers_with(name: &'static str, value: &'static str) -> axum::http::HeaderMap {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(name, axum::http::HeaderValue::from_static(value));
        headers
    }

    /// Admin-only routes must accept an admin's API key — the router's
    /// `require_auth` already treats `X-API-Key` as a first-class credential,
    /// so `require_admin` rejecting it was an inconsistent 401.
    #[tokio::test]
    async fn admin_identity_accepts_api_keys() {
        let backend = admin_backend().await;
        seed_key(&backend, "u-admin", "admin", "oxo_admin_key").await;
        seed_key(&backend, "u-user", "user", "oxo_user_key").await;

        let admin = headers_with("x-api-key", "oxo_admin_key");
        match resolve_admin(&backend, &admin).await {
            Ok(name) => assert_eq!(
                name, "u-admin",
                "an admin API key must resolve to the admin identity"
            ),
            Err((status, body)) => {
                panic!("admin API key rejected: {status} {}", body.0.message)
            }
        }

        // Non-admin key: authenticated but forbidden.
        let user = headers_with("x-api-key", "oxo_user_key");
        let (status, body) = resolve_admin(&backend, &user).await.unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body.0.code, "ACCESS_DENIED");

        // Unknown key: unauthenticated.
        let unknown = headers_with("x-api-key", "not-a-key");
        let (status, body) = resolve_admin(&backend, &unknown).await.unwrap_err();
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body.0.code, "INVALID_API_KEY");

        // No credential at all: the original AUTH_REQUIRED contract.
        let (status, body) = resolve_admin(&backend, &axum::http::HeaderMap::new())
            .await
            .unwrap_err();
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body.0.code, "AUTH_REQUIRED");
    }

    /// /auth/me must recognize X-API-Key — the router's `require_auth`
    /// accepts it on every protected route, so "who am I?" returning
    /// `authenticated:false` for the same key was an inconsistent answer
    /// (live API test: key authenticated /quota and /users but /auth/me
    /// reported `authenticated:false`).
    #[tokio::test]
    async fn auth_me_recognizes_api_keys() {
        let backend = admin_backend().await;
        seed_key(&backend, "u-admin", "admin", "oxo_admin_key").await;
        seed_key(&backend, "u-user", "user", "oxo_user_key").await;

        // Valid admin key → authenticated with identity.
        let admin = headers_with("x-api-key", "oxo_admin_key");
        let me = auth_me_in(&backend, &admin).await;
        assert!(me.authenticated);
        assert_eq!(me.username.as_deref(), Some("u-admin"));
        assert_eq!(me.role.as_deref(), Some("admin"));

        // Valid non-admin key → authenticated with its role.
        let user = headers_with("x-api-key", "oxo_user_key");
        let me = auth_me_in(&backend, &user).await;
        assert!(me.authenticated);
        assert_eq!(me.role.as_deref(), Some("user"));

        // Invalid key → not authenticated (status probe, not a gate).
        let bad = headers_with("x-api-key", "not-a-key");
        let me = auth_me_in(&backend, &bad).await;
        assert!(!me.authenticated);

        // No credential → not authenticated.
        let me = auth_me_in(&backend, &axum::http::HeaderMap::new()).await;
        assert!(!me.authenticated);
    }
}
