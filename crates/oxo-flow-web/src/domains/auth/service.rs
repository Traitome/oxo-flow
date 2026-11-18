//! Pure authentication domain logic — zero HTTP dependency.
//!
//! Each function takes plain Rust types and returns `Result<T, String>`.
//! Suitable for reuse from handlers, CLI commands, or tests without
//! coupling to axum or any web framework.

use uuid::Uuid;

use super::types::*;

/// Snapshot of the environment variables that drive credential resolution.
///
/// Built once by [`authenticate`] so the decision can live in a pure
/// function ([`authenticate_with`]) that tests drive deterministically
/// without mutating process-global state (this crate forbids `unsafe`,
/// which edition-2024 `set_var` requires).
#[derive(Debug, Default, Clone)]
struct AuthEnv {
    admin_password: Option<String>,
    user_password: Option<String>,
    viewer_password: Option<String>,
    dev_mode: bool,
}

impl AuthEnv {
    /// Read the live process environment.
    fn from_process_env() -> Self {
        let get = |key: &str| std::env::var(key).ok();
        Self {
            admin_password: get("OXO_FLOW_ADMIN_PASSWORD"),
            user_password: get("OXO_FLOW_USER_PASSWORD"),
            viewer_password: get("OXO_FLOW_VIEWER_PASSWORD"),
            dev_mode: std::env::var("OXO_FLOW_DEV_MODE").as_deref() == Ok("1"),
        }
    }
}

/// Authenticate user with username and password.
/// Uses env-var based credential checking (backward compat with existing system).
pub fn authenticate(username: &str, password: &str) -> Result<LoginResponse, String> {
    authenticate_with(username, password, &AuthEnv::from_process_env())
}

/// Pure core of [`authenticate`] — `env` is passed in so every branch,
/// including both dev-mode states, is testable without environment
/// mutation.
fn authenticate_with(
    username: &str,
    password: &str,
    env: &AuthEnv,
) -> Result<LoginResponse, String> {
    // Check admin
    if username == "admin" && env.admin_password.as_deref() == Some(password) {
        return Ok(LoginResponse {
            token: generate_token(),
            username: "admin".into(),
            role: "admin".into(),
        });
    }
    // Dev mode fallback: password equals username — ONLY when explicitly
    // enabled. In production this MUST be off, otherwise anyone can log in
    // as any username.
    //
    // Deliberately BEFORE the #516 admin guard below: the documented
    // contract is password==username for ANY user, admin included, and dev
    // mode is refused on a non-loopback bind at startup
    // (`effective_bind_host`), so the fallback cannot expose a network
    // listener. Outside dev mode the guard still makes
    // OXO_FLOW_ADMIN_PASSWORD the only credential that mints admin.
    if env.dev_mode && password == username && !username.is_empty() {
        tracing::warn!(
            username = username,
            "DEV MODE: accepted password==username login for '{}'",
            username
        );
        // The role must reflect the account, not a hardcoded "user" —
        // /api/auth/me derives admin for the admin username, and the login
        // response must agree (issue #142 persona testing, M6).
        let role = if username == "admin" {
            "admin"
        } else if username == "viewer" {
            "viewer"
        } else {
            "user"
        };
        return Ok(LoginResponse {
            token: generate_token(),
            username: username.into(),
            role: role.into(),
        });
    }
    // The shared user/viewer passwords must never mint the admin identity
    // (#516): OXO_FLOW_ADMIN_PASSWORD (or the loopback-only dev fallback
    // above) is the only credential that may sign in as "admin", otherwise
    // anyone holding the weaker shared secret gets a session whose name
    // resolves to the privileged admin row.
    if username == "admin" {
        return Err("Invalid credentials".into());
    }
    // Check user
    if env.user_password.as_deref() == Some(password) {
        return Ok(LoginResponse {
            token: generate_token(),
            username: username.into(),
            role: "user".into(),
        });
    }
    // Check viewer
    if env.viewer_password.as_deref() == Some(password) {
        return Ok(LoginResponse {
            token: generate_token(),
            username: username.into(),
            role: "viewer".into(),
        });
    }

    Err("Invalid credentials".into())
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

/// Run a CPU-heavy bcrypt operation off the async worker thread.
///
/// The handlers call the bcrypt helpers synchronously (match guards,
/// `Option::map`), so the offload has to happen inside the sync API:
/// `block_in_place` tells the multi-threaded runtime this worker is about to
/// block so it can move other tasks elsewhere. On a current-thread runtime
/// (unit tests) there is no worker to protect — run inline.
fn blocking_crypto<T>(op: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(op)
        }
        _ => op(),
    }
}

/// Hash a password for storage (bcrypt, default cost).
///
/// DB-created accounts (POST /api/users) store this hash in
/// `users.password_hash`; env-var credentials are unaffected.
pub fn hash_password(password: &str) -> Result<String, String> {
    blocking_crypto(|| {
        bcrypt::hash(password, bcrypt::DEFAULT_COST)
            .map_err(|e| format!("Failed to hash password: {e}"))
    })
}

/// Verify a password against a stored bcrypt hash.
pub fn verify_db_password(password: &str, hash: &str) -> bool {
    blocking_crypto(|| bcrypt::verify(password, hash).unwrap_or(false))
}

/// Build a login response for a DB-verified account (role from the users
/// table, not the env-var vocabulary).
pub fn login_response_for(username: String, role: String) -> LoginResponse {
    LoginResponse {
        token: generate_token(),
        username,
        role,
    }
}

// ---------------------------------------------------------------------------
// OAuth2 service functions
// ---------------------------------------------------------------------------

/// Fixed redirect-URI fallback documented for the OAuth flow.
pub const DEFAULT_OAUTH_REDIRECT_URI: &str = "http://localhost:3000/api/auth/oauth/callback";

/// Resolve the redirect URI used by BOTH OAuth steps: an explicit request
/// value wins, then `OXO_FLOW_OAUTH_REDIRECT_URI`, then the documented
/// fallback.
///
/// The authorize URL and the token exchange MUST carry the same redirect
/// URI — providers reject the code with `redirect_uri` mismatch otherwise.
/// The authorize handler previously defaulted to the fixed fallback while
/// the callback read the env var, so a deployment configured through
/// `OXO_FLOW_OAUTH_REDIRECT_URI` produced an authorize link that could
/// never exchange its code.
#[must_use]
pub fn oauth_redirect_uri(explicit: Option<&str>) -> String {
    resolve_oauth_redirect_uri(
        explicit,
        std::env::var("OXO_FLOW_OAUTH_REDIRECT_URI").ok().as_deref(),
    )
}

/// Pure core of [`oauth_redirect_uri`]: blank values (including an empty
/// advertised env var) count as unset, exactly like an omitted parameter.
fn resolve_oauth_redirect_uri(explicit: Option<&str>, env_value: Option<&str>) -> String {
    explicit
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| env_value.map(str::trim).filter(|value| !value.is_empty()))
        .map(str::to_string)
        .unwrap_or_else(|| DEFAULT_OAUTH_REDIRECT_URI.to_string())
}

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
        // Test-only provider (issue #1141): lets the full authorize →
        // callback flow run without network I/O. Compiled out of every
        // production build, exactly like the AI `scripted` backend
        // (#1040 B3).
        #[cfg(test)]
        "stub" => Ok(super::oauth::OAuthConfig::stub(redirect_uri)),
        _ => Err(format!("Unsupported OAuth provider: {provider}")),
    }
}

// ---------------------------------------------------------------------------
// OAuth state binding and redirect allowlist (issue #1141, audit WEB-16)
// ---------------------------------------------------------------------------

/// Normalize a provider name for binding and config resolution: the OAuth
/// contract is case-insensitive (`ORCID` == `orcid`), and whitespace never
/// changes the provider a state was issued for.
#[must_use]
pub fn normalize_provider(provider: &str) -> String {
    provider.trim().to_ascii_lowercase()
}

/// Why a callback's OAuth CSRF state was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthStateError {
    /// Missing, never issued, malformed, or already consumed (replay).
    Invalid,
    /// Issued by this server but past the 10-minute state TTL.
    Expired,
    /// Issued for a different provider.
    ProviderMismatch { expected: String, actual: String },
    /// Issued for a different redirect URI.
    RedirectMismatch { expected: String, actual: String },
}

impl OAuthStateError {
    /// Structured API error code for this rejection.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            // Unknown, replayed, and expired states keep the historical
            // code (the expiry detail stays in the message).
            Self::Invalid | Self::Expired => "OAUTH_INVALID_STATE",
            // A state that this server did issue, but not for this
            // provider/redirect, is a distinct, actionable failure.
            Self::ProviderMismatch { .. } | Self::RedirectMismatch { .. } => "OAUTH_STATE_MISMATCH",
        }
    }

    /// Human-readable message (surfaced as the API error's `message`).
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Invalid => "Invalid or expired CSRF state".to_string(),
            Self::Expired => "OAuth state expired — restart the sign-in flow".to_string(),
            Self::ProviderMismatch { expected, actual } => format!(
                "OAuth state was issued for provider '{expected}', not '{actual}' — restart the sign-in flow"
            ),
            Self::RedirectMismatch { expected, actual } => format!(
                "OAuth state was issued for redirect URI '{expected}', not '{actual}' — restart the sign-in flow"
            ),
        }
    }
}

impl std::fmt::Display for OAuthStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// Normalize an absolute `http`/`https` URL to its origin
/// (`scheme://host[:port]`): scheme and host lowercased, default ports
/// dropped, path/query/fragment ignored.
///
/// Returns `None` for anything that is not an absolute http(s) URL, and for
/// authorities carrying userinfo — `https://trusted.example@evil.example/`
/// must never resolve to `trusted.example`.
#[must_use]
pub fn url_origin(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let (scheme, rest) = raw.split_once("://")?;
    let scheme = scheme.trim().to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let (host, port) = split_host_port(authority)?;
    if host.is_empty() {
        return None;
    }
    let host = host.to_ascii_lowercase();
    // A bracketed IPv6 literal arrives without its brackets; re-bracket it
    // so the normalized origin stays parseable.
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    let port = match port {
        Some(p) => Some(p.parse::<u16>().ok()?),
        None => None,
    };
    // Default ports are equivalent to no port (`https://x` == `https://x:443`).
    let port = match (scheme.as_str(), port) {
        ("http", Some(80)) | ("https", Some(443)) => None,
        (_, p) => p,
    };
    Some(match port {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

/// Split an authority into `(host, port)`.
///
/// IPv6 literals must be bracketed (`[::1]`, `[::1]:3000`); an unbracketed
/// literal is ambiguous with a port and is rejected — the allowlist must
/// never guess at the host it is matching.
fn split_host_port(authority: &str) -> Option<(&str, Option<&str>)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        return match tail {
            "" => Some((host, None)),
            _ => Some((host, Some(tail.strip_prefix(':')?))),
        };
    }
    if authority.matches(':').count() > 1 {
        return None;
    }
    match authority.split_once(':') {
        Some((host, port)) if !port.is_empty() => Some((host, Some(port))),
        Some(_) => None, // a trailing colon with no port
        None => Some((authority, None)),
    }
}

/// Server-side origins a post-callback redirect may target.
///
/// Derived from the origin configuration the server already trusts — there
/// is deliberately no parallel allowlist setting:
///
/// - `OXO_FLOW_ALLOWED_ORIGINS` (comma-separated), or the built-in
///   localhost origins when it is unset — the very list the CORS layer uses
///   ([`crate::server::configured_origins`]).
/// - the origin of the server-configured `OXO_FLOW_OAUTH_REDIRECT_URI` (or
///   the documented fallback), which a working deployment always needs.
/// - the origin declared by `OXO_FLOW_PUBLIC_URL` (reverse-proxy
///   deployments).
#[must_use]
pub fn oauth_redirect_allowlist() -> Vec<String> {
    let mut origins: Vec<String> = crate::server::configured_origins()
        .iter()
        .filter_map(|origin| url_origin(origin))
        .collect();
    origins.extend(url_origin(&oauth_redirect_uri(None)));
    origins.extend(crate::server::declared_public_origin().and_then(|origin| url_origin(&origin)));
    origins.sort_unstable();
    origins.dedup();
    origins
}

/// True when `redirect_uri` is an absolute http(s) URL whose origin appears
/// in the server-side allowlist.
///
/// The path is deliberately not constrained: the provider's registered
/// callback URI pins the path, while the origin is what stops a crafted
/// start request from steering the authorization code to another host.
#[must_use]
pub fn oauth_redirect_allowed(redirect_uri: &str) -> bool {
    redirect_allowed_within(redirect_uri, &oauth_redirect_allowlist())
}

/// Pure core of [`oauth_redirect_allowed`] for tests and callers that
/// already hold the allowlist.
fn redirect_allowed_within(redirect_uri: &str, allowed_origins: &[String]) -> bool {
    let Some(origin) = url_origin(redirect_uri) else {
        return false;
    };
    allowed_origins
        .iter()
        .any(|allowed| url_origin(allowed).as_deref() == Some(origin.as_str()))
}

/// Persist a pending OAuth CSRF state, bound to the provider and redirect
/// URI it was issued for (issue #1141).
pub async fn store_pending_state(
    state: &str,
    provider: &str,
    redirect_uri: &str,
) -> Result<(), String> {
    let backend =
        crate::infra::db::global_backend().ok_or_else(|| "Database unavailable".to_string())?;
    let now = chrono::Utc::now().to_rfc3339();
    backend
        .store_oauth_state(
            state,
            &normalize_provider(provider),
            redirect_uri.trim(),
            &now,
        )
        .await
}

/// A CSRF state is short-lived by design: the daily cleanup prunes the
/// table, but without a consumption-time freshness check a leaked state
/// would stay replayable for up to that full 24h window.
const OAUTH_STATE_TTL_MINS: i64 = 10;

/// Verify a callback's CSRF state was issued by this server, is fresh, and
/// was issued for THIS provider and redirect URI — then consume it.
///
/// Consumption happens first and unconditionally (`DELETE … RETURNING`), so
/// a state is single-use even when the binding check rejects it: a replayed
/// state can never be retried, and concurrent callbacks cannot both win.
pub async fn verify_and_consume_state(
    state: &str,
    provider: &str,
    redirect_uri: &str,
) -> Result<(), OAuthStateError> {
    let backend = crate::infra::db::global_backend().ok_or(OAuthStateError::Invalid)?;
    let row = backend
        .consume_oauth_state(state)
        .await
        .map_err(|_| OAuthStateError::Invalid)?;
    let Some(row) = row else {
        return Err(OAuthStateError::Invalid);
    };
    let created_ts = chrono::DateTime::parse_from_rfc3339(&row.created_at)
        .map_err(|_| OAuthStateError::Invalid)?;
    if chrono::Utc::now() - created_ts.with_timezone(&chrono::Utc)
        > chrono::Duration::minutes(OAUTH_STATE_TTL_MINS)
    {
        return Err(OAuthStateError::Expired);
    }
    let provider = normalize_provider(provider);
    if row.provider != provider {
        return Err(OAuthStateError::ProviderMismatch {
            expected: row.provider,
            actual: provider,
        });
    }
    let redirect_uri = redirect_uri.trim();
    if row.redirect_uri != redirect_uri {
        return Err(OAuthStateError::RedirectMismatch {
            expected: row.redirect_uri,
            actual: redirect_uri.to_string(),
        });
    }
    Ok(())
}

/// Initiate an OAuth2 authorization flow.
///
/// Returns the provider's authorization URL and a CSRF state token bound to
/// `(provider, redirect_uri)`.
pub async fn initiate_oauth(
    provider: &str,
    redirect_uri: &str,
) -> Result<OAuthAuthorizeResponse, String> {
    // Defense in depth: callers gate this too (with the structured
    // OAUTH_REDIRECT_NOT_ALLOWED code), but a bound state must never be
    // issued for a target the callback would refuse.
    if !oauth_redirect_allowed(redirect_uri) {
        return Err(format!(
            "OAuth redirect target '{redirect_uri}' is not in the server allowlist"
        ));
    }
    let provider = normalize_provider(provider);
    let config = oauth_config_from_env(&provider, redirect_uri)?;
    let state = generate_token();
    let authorize_url = config.authorize_url(&state);

    // Persist the pending state so the callback can verify it (CSRF defense).
    store_pending_state(&state, &provider, redirect_uri).await?;

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

/// Ensure the canonical OAuth users row exists for this identity, returning
/// the username the row carries.
///
/// Idempotent on the namespaced id: an existing row (re-login) is reused
/// untouched. Otherwise the row is inserted with the provider display name;
/// when that username is already taken — two ORCID accounts can legitimately
/// share a given name, and INSERT OR IGNORE would then silently skip the row
/// while the session still binds to the id — the insert is retried with a
/// fallback username derived from the unique provider user id
/// (`orcid:0000-0002-1825-0097`). The session must never outlive its user row:
/// role resolution reads the row by id and fails closed (#516), so a skipped
/// insert meant every subsequent request answered 401.
pub async fn ensure_oauth_user(
    backend: &dyn crate::infra::db::StorageBackend,
    oauth_user_id: &str,
    oauth_username: &str,
    fallback_username: &str,
) -> Result<String, String> {
    backend
        .ensure_oauth_user(oauth_user_id, oauth_username, fallback_username)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth_env(dev_mode: bool) -> AuthEnv {
        AuthEnv {
            dev_mode,
            ..AuthEnv::default()
        }
    }

    #[test]
    fn test_dev_mode_login_role_matches_account() {
        // M6/#1040 regression: the dev-mode password==username fallback must
        // accept the admin account and must not hardcode "user" — the role
        // must agree with /api/auth/me's derivation. Driven through the pure
        // core so this runs identically in CI (no DEV_MODE env var there).
        let env = auth_env(true);
        let resp = authenticate_with("admin", "admin", &env).unwrap();
        assert_eq!(resp.username, "admin");
        assert_eq!(resp.role, "admin");
        let resp = authenticate_with("alice", "alice", &env).unwrap();
        assert_eq!(resp.role, "user");
        let resp = authenticate_with("viewer", "viewer", &env).unwrap();
        assert_eq!(resp.role, "viewer");
    }

    #[test]
    fn test_dev_mode_requires_the_matching_password() {
        let env = auth_env(true);
        assert!(
            authenticate_with("admin", "not-admin", &env).is_err(),
            "dev mode must not accept an arbitrary password for admin"
        );
        assert!(
            authenticate_with("alice", "bob", &env).is_err(),
            "dev mode accepts password == username only"
        );
    }

    #[test]
    fn test_authenticate_password_equals_username_rejected_in_production() {
        // Without OX_FLOW_DEV_MODE=1, password==username MUST be rejected —
        // for admin and for every other account.
        let env = auth_env(false);
        assert!(
            authenticate_with("admin", "admin", &env).is_err(),
            "admin/admin must be rejected without OXO_FLOW_DEV_MODE=1"
        );
        assert!(
            authenticate_with("testuser", "testuser", &env).is_err(),
            "password==username should be rejected without OXO_FLOW_DEV_MODE=1"
        );
    }

    #[test]
    fn test_env_user_password_never_mints_admin() {
        // #516: the shared user/viewer passwords must not mint the admin
        // identity — a session named "admin" resolves to the privileged
        // bootstrap row. Checked with and without dev mode: only
        // ADMIN_PASSWORD (or the explicit loopback-only dev fallback) may
        // sign in as "admin".
        for dev_mode in [false, true] {
            let env = AuthEnv {
                admin_password: None,
                user_password: Some("shared-user".into()),
                viewer_password: Some("shared-viewer".into()),
                dev_mode,
            };
            // The shared passwords never authenticate the admin name, even
            // in dev mode where the dev fallback itself would (the fallback
            // requires password == username).
            assert!(
                authenticate_with("admin", "shared-user", &env).is_err(),
                "user password minted admin (dev_mode={dev_mode})"
            );
            assert!(
                authenticate_with("admin", "shared-viewer", &env).is_err(),
                "viewer password minted admin (dev_mode={dev_mode})"
            );
            assert!(
                authenticate_with("admin", "anything", &env).is_err(),
                "admin must not authenticate without its own password (dev_mode={dev_mode})"
            );
            // Sanity: the shared passwords still authenticate non-admin names.
            assert_eq!(
                authenticate_with("alice", "shared-user", &env)
                    .expect("user password authenticates a user")
                    .role,
                "user"
            );
        }
    }

    #[test]
    fn test_admin_password_still_authenticates_admin() {
        let env = AuthEnv {
            admin_password: Some("secret-admin".into()),
            ..AuthEnv::default()
        };
        let resp = authenticate_with("admin", "secret-admin", &env).unwrap();
        assert_eq!(resp.role, "admin");
        assert!(
            authenticate_with("admin", "admin", &env).is_err(),
            "the admin password is the only production credential for admin"
        );
    }

    #[test]
    fn test_authenticate_invalid() {
        let result = authenticate_with("nobody", "wrongpassword", &AuthEnv::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_license_status() {
        let status = license_status();
        assert!(status.valid);
        assert_eq!(status.license_type, Some("academic".into()));
    }

    #[tokio::test]
    async fn test_bcrypt_roundtrip_current_thread_runtime() {
        // Exercises the inline path (no multi-thread runtime to protect).
        let hash = hash_password("s3cret").expect("hash succeeds");
        assert!(verify_db_password("s3cret", &hash));
        assert!(!verify_db_password("wrong", &hash));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_bcrypt_roundtrip_multi_thread_runtime() {
        // Exercises the block_in_place offload path used by the real server.
        let hash = hash_password("s3cret").expect("hash succeeds");
        assert!(verify_db_password("s3cret", &hash));
        assert!(!verify_db_password("wrong", &hash));
    }

    /// A one-connection in-memory backend (per-connection `sqlite::memory:`).
    async fn users_backend() -> crate::infra::db::sqlite::SqliteBackend {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory pool");
        sqlx::query(
            "CREATE TABLE users (id TEXT PRIMARY KEY, username TEXT UNIQUE NOT NULL,              role TEXT NOT NULL, auth_type TEXT NOT NULL, os_user TEXT,              password_hash TEXT, created_at TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        crate::infra::db::sqlite::SqliteBackend::from_pool(pool)
    }

    #[tokio::test]
    async fn test_ensure_oauth_user_creates_and_reuses_row() {
        let backend = users_backend().await;
        let name = ensure_oauth_user(&backend, "oauth:orcid:id-A", "orcid:Alice", "orcid:id-A")
            .await
            .expect("first insert succeeds");
        assert_eq!(name, "orcid:Alice");

        // Re-login: same id, untouched — even with a different display name.
        let name = ensure_oauth_user(&backend, "oauth:orcid:id-A", "orcid:Renamed", "orcid:id-A")
            .await
            .expect("re-login reuses the existing row");
        assert_eq!(name, "orcid:Alice");
    }

    #[tokio::test]
    async fn test_ensure_oauth_user_same_display_name_both_rows_exist() {
        // The reported bug: two provider accounts sharing a given name. Both
        // must end up with a users row; neither may be silently skipped.
        let backend = users_backend().await;
        let first = ensure_oauth_user(&backend, "oauth:orcid:id-A", "orcid:Wei", "orcid:id-A")
            .await
            .expect("first account inserts");
        assert_eq!(first, "orcid:Wei");
        let second = ensure_oauth_user(&backend, "oauth:orcid:id-B", "orcid:Wei", "orcid:id-B")
            .await
            .expect("second account falls back to its unique id");
        assert_eq!(first, "orcid:Wei");
        assert_eq!(
            second, "orcid:id-B",
            "collision falls back to the unique id"
        );

        let rows = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM users WHERE username = 'orcid:Wei' OR username = 'orcid:id-B'",
        )
        .fetch_one(backend.inner_pool())
        .await
        .unwrap();
        assert_eq!(rows, 2, "both identities have a persisted row");
    }

    #[test]
    fn oauth_redirect_uri_precedence_and_blank_handling() {
        // Both OAuth steps must resolve the SAME URI: request value, then
        // OXO_FLOW_OAUTH_REDIRECT_URI, then the documented fallback.
        assert_eq!(
            resolve_oauth_redirect_uri(
                Some("https://flows.example.com/cb"),
                Some("https://env.example.com/cb")
            ),
            "https://flows.example.com/cb"
        );
        assert_eq!(
            resolve_oauth_redirect_uri(None, Some("https://env.example.com/cb")),
            "https://env.example.com/cb"
        );
        // Blank values count as unset (an empty env var must not produce an
        // authorize URL with `redirect_uri=`).
        assert_eq!(
            resolve_oauth_redirect_uri(Some("  "), Some("https://env.example.com/cb")),
            "https://env.example.com/cb"
        );
        assert_eq!(
            resolve_oauth_redirect_uri(None, Some("")),
            DEFAULT_OAUTH_REDIRECT_URI
        );
        assert_eq!(
            resolve_oauth_redirect_uri(None, None),
            DEFAULT_OAUTH_REDIRECT_URI
        );
        // Surrounding whitespace is trimmed from the winner.
        assert_eq!(
            resolve_oauth_redirect_uri(None, Some("  https://env.example.com/cb  ")),
            "https://env.example.com/cb"
        );
    }

    #[tokio::test]
    async fn test_ensure_oauth_user_missing_id_rejected() {
        let backend = users_backend().await;
        let result = ensure_oauth_user(&backend, "", "orcid:Wei", "orcid:").await;
        assert!(result.is_err(), "empty id must fail closed");
    }

    #[tokio::test]
    async fn test_oauth_state_single_use_and_freshness() {
        let backend = crate::infra::db::oauth_test_backend().await;
        crate::infra::db::set_global_backend(backend.clone());

        store_pending_state("state-fresh", "orcid", "https://flows.example/cb")
            .await
            .expect("store fresh");
        verify_and_consume_state("state-fresh", "orcid", "https://flows.example/cb")
            .await
            .expect("a fresh state verifies");
        // Single use: the row is gone.
        assert!(
            verify_and_consume_state("state-fresh", "orcid", "https://flows.example/cb")
                .await
                .is_err(),
            "a consumed state must not verify twice"
        );

        // A leaked state does not stay replayable until the daily purge:
        // consumption rejects anything older than the TTL.
        let stale_time = (chrono::Utc::now() - chrono::Duration::minutes(11)).to_rfc3339();
        backend
            .store_oauth_state(
                "state-stale",
                "orcid",
                "https://flows.example/cb",
                &stale_time,
            )
            .await
            .unwrap();
        let err = verify_and_consume_state("state-stale", "orcid", "https://flows.example/cb")
            .await
            .expect_err("a stale state must be rejected");
        assert_eq!(err, OAuthStateError::Expired, "unexpected error: {err}");
        assert!(err.message().contains("expired"));
    }

    #[tokio::test]
    async fn test_oauth_state_binding_rejects_provider_and_redirect_mismatch() {
        let backend = crate::infra::db::oauth_test_backend().await;
        crate::infra::db::set_global_backend(backend.clone());

        // Issued for orcid + R1, presented on github's callback.
        store_pending_state("bound-provider", "orcid", "https://flows.example/cb")
            .await
            .unwrap();
        let err = verify_and_consume_state("bound-provider", "github", "https://flows.example/cb")
            .await
            .expect_err("a provider mismatch must be rejected");
        assert_eq!(err.code(), "OAUTH_STATE_MISMATCH");
        assert!(
            matches!(err, OAuthStateError::ProviderMismatch { .. }),
            "unexpected error: {err}"
        );
        // The mismatch consumed the state: it cannot be retried with the
        // right provider afterwards (fail closed, single use).
        assert!(
            verify_and_consume_state("bound-provider", "orcid", "https://flows.example/cb")
                .await
                .is_err()
        );

        // Issued for R1, presented with R2.
        store_pending_state("bound-redirect", "orcid", "https://flows.example/cb")
            .await
            .unwrap();
        let err = verify_and_consume_state("bound-redirect", "orcid", "https://other.example/cb")
            .await
            .expect_err("a redirect mismatch must be rejected");
        assert_eq!(err.code(), "OAUTH_STATE_MISMATCH");
        assert!(
            matches!(err, OAuthStateError::RedirectMismatch { .. }),
            "unexpected error: {err}"
        );

        // Provider normalization: an upper-case name for the SAME provider
        // still verifies (the contract is case-insensitive).
        store_pending_state("bound-case", "orcid", "https://flows.example/cb")
            .await
            .unwrap();
        verify_and_consume_state("bound-case", "ORCID", "https://flows.example/cb")
            .await
            .expect("provider names are case-insensitive");
    }

    #[test]
    fn test_url_origin_normalization() {
        assert_eq!(
            url_origin("https://Flows.Example.com/cb?x=1#f"),
            Some("https://flows.example.com".into())
        );
        assert_eq!(
            url_origin("https://flows.example.com:443/cb"),
            Some("https://flows.example.com".into())
        );
        assert_eq!(
            url_origin("http://flows.example.com:80"),
            Some("http://flows.example.com".into())
        );
        assert_eq!(
            url_origin("http://localhost:3000/api/auth/oauth/callback"),
            Some("http://localhost:3000".into())
        );
        assert_eq!(
            url_origin("http://[::1]:8080/cb"),
            Some("http://[::1]:8080".into())
        );
        // Not absolute http(s), or not a URL we may match.
        assert_eq!(url_origin("flows.example.com/cb"), None);
        assert_eq!(url_origin("ftp://flows.example.com/cb"), None);
        assert_eq!(
            url_origin("https://flows.example.com@evil.example/cb"),
            None
        );
        assert_eq!(url_origin("https:///cb"), None);
        assert_eq!(url_origin("https://::1/cb"), None);
    }

    #[test]
    fn test_redirect_allowlist_matches_origins_not_paths() {
        let allowed = vec![
            "https://flows.example.com".to_string(),
            "http://localhost:3000".to_string(),
        ];
        assert!(redirect_allowed_within(
            "https://flows.example.com/api/auth/oauth/callback",
            &allowed
        ));
        assert!(redirect_allowed_within(
            "https://flows.example.com:443/anything?x=1",
            &allowed
        ));
        assert!(redirect_allowed_within(
            "http://localhost:3000/cb",
            &allowed
        ));
        // Different origin, scheme, or host must fail.
        assert!(!redirect_allowed_within(
            "https://evil.example.com/cb",
            &allowed
        ));
        assert!(!redirect_allowed_within(
            "http://flows.example.com/cb",
            &allowed
        ));
        assert!(!redirect_allowed_within(
            "https://flows.example.com.evil.example/cb",
            &allowed
        ));
        assert!(!redirect_allowed_within(
            "https://flows.example.com@evil.example/cb",
            &allowed
        ));
        assert!(!redirect_allowed_within("", &allowed));
    }
}
