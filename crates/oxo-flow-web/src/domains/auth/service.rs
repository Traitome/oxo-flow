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
        _ => Err(format!("Unsupported OAuth provider: {provider}")),
    }
}

/// Persist a pending OAuth CSRF state so the callback can verify it.
pub async fn store_pending_state(state: &str) -> Result<(), String> {
    let backend =
        crate::infra::db::global_backend().ok_or_else(|| "Database unavailable".to_string())?;
    let now = chrono::Utc::now().to_rfc3339();
    backend.store_oauth_state(state, &now).await
}

/// A CSRF state is short-lived by design: the daily cleanup prunes the
/// table, but without a consumption-time freshness check a leaked state
/// would stay replayable for up to that full 24h window.
const OAUTH_STATE_TTL_MINS: i64 = 10;

/// Verify a callback's CSRF state was issued by this server, is fresh, and
/// consume it (single use).
pub async fn verify_and_consume_state(state: &str) -> Result<(), String> {
    let backend =
        crate::infra::db::global_backend().ok_or_else(|| "Database unavailable".to_string())?;
    let created = backend.consume_oauth_state(state).await?;
    let Some(created) = created else {
        return Err("Invalid or expired CSRF state".into());
    };
    let created_ts = chrono::DateTime::parse_from_rfc3339(&created)
        .map_err(|_| "Invalid or expired CSRF state".to_string())?;
    if chrono::Utc::now() - created_ts.with_timezone(&chrono::Utc)
        > chrono::Duration::minutes(OAUTH_STATE_TTL_MINS)
    {
        return Err("OAuth state expired — restart the sign-in flow".into());
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
    use crate::infra::db::StorageBackend;

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
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory pool");
        sqlx::query("CREATE TABLE oauth_states (state TEXT PRIMARY KEY, created_at TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        let backend = std::sync::Arc::new(crate::infra::db::sqlite::SqliteBackend::from_pool(pool));
        crate::infra::db::set_global_backend(backend.clone());

        store_pending_state("state-fresh")
            .await
            .expect("store fresh");
        verify_and_consume_state("state-fresh")
            .await
            .expect("a fresh state verifies");
        // Single use: the row is gone.
        assert!(
            verify_and_consume_state("state-fresh").await.is_err(),
            "a consumed state must not verify twice"
        );

        // A leaked state does not stay replayable until the daily purge:
        // consumption rejects anything older than the TTL.
        let stale_time = (chrono::Utc::now() - chrono::Duration::minutes(11)).to_rfc3339();
        backend
            .store_oauth_state("state-stale", &stale_time)
            .await
            .unwrap();
        let err = verify_and_consume_state("state-stale")
            .await
            .expect_err("a stale state must be rejected");
        assert!(err.contains("expired"), "unexpected error: {err}");
    }
}
