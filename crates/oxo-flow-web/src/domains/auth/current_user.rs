//! Acting-user context for ownership enforcement (issue #82 P0-4).
//!
//! The `require_auth` middleware (team/hpc modes) inserts a `CurrentUser`
//! into request extensions after validating the session token; personal
//! mode has no middleware, so handlers fall back to the 'default'
//! pseudo-user that personal-mode rows are attributed to.

use axum::Extension;

/// The user acting on a request: their canonical `users.id` (the value
/// ownership columns like `runs.user_id` are keyed by) plus their role.
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub id: String,
    pub role: String,
}

impl CurrentUser {
    /// The personal-mode pseudo-user: no auth middleware, single local user.
    pub fn default_user() -> Self {
        Self {
            id: "default".to_string(),
            role: "user".to_string(),
        }
    }

    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }
}

/// Resolve the acting user from the optional auth extension.
///
/// `None` means personal mode (or a public route) — fall back to the
/// `default` pseudo-user so ownership columns always carry a valid
/// `users.id` (they are foreign keys).
pub fn resolve(ext: Option<&Extension<CurrentUser>>) -> CurrentUser {
    ext.map(|e| e.0.clone())
        .unwrap_or_else(CurrentUser::default_user)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_falls_back_to_default_user_without_extension() {
        let user = resolve(None);
        assert_eq!(user.id, "default");
        assert!(!user.is_admin());
    }

    #[test]
    fn resolve_returns_extension_user() {
        let ext = Extension(CurrentUser {
            id: "alice-id".into(),
            role: "admin".into(),
        });
        let user = resolve(Some(&ext));
        assert_eq!(user.id, "alice-id");
        assert!(user.is_admin());
    }
}
