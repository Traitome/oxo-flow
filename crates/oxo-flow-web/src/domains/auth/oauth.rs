//! OAuth2 authentication flows for ORCID and GitHub.
//!
//! ORCID is the preferred provider: every scientist has one, it provides
//! academic identity verification, and requires no extra registration.
//! GitHub OAuth2 is available as a fallback for developer-oriented deployments.
//! Invite-code auth is available for air-gapped environments.
//!
//! Zero HTTP dependency — these are pure async functions that build
//! authorization URLs and exchange codes for tokens.

/// Supported OAuth2 providers.
#[derive(Debug, Clone, PartialEq)]
pub enum OAuthProvider {
    Orcid,
    GitHub,
}

/// Configuration for an OAuth2 provider.
#[derive(Debug, Clone)]
pub struct OAuthConfig {
    pub provider: OAuthProvider,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
}

impl OAuthConfig {
    /// Create an ORCID OAuth2 configuration.
    ///
    /// Uses the public ORCID API endpoints. For the sandbox, set
    /// `ORCID_SANDBOX=true` in the environment.
    pub fn orcid(client_id: &str, client_secret: &str, redirect_uri: &str) -> Self {
        Self {
            provider: OAuthProvider::Orcid,
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            redirect_uri: redirect_uri.to_string(),
        }
    }

    /// Create a GitHub OAuth2 configuration.
    pub fn github(client_id: &str, client_secret: &str, redirect_uri: &str) -> Self {
        Self {
            provider: OAuthProvider::GitHub,
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            redirect_uri: redirect_uri.to_string(),
        }
    }

    /// Build the authorization URL for the provider.
    pub fn authorize_url(&self, state: &str) -> String {
        match self.provider {
            OAuthProvider::Orcid => {
                let base = if std::env::var("ORCID_SANDBOX").is_ok() {
                    "https://sandbox.orcid.org/oauth/authorize"
                } else {
                    "https://orcid.org/oauth/authorize"
                };
                format!(
                    "{}?client_id={}&response_type=code&scope=/authenticate&redirect_uri={}&state={}",
                    base,
                    urlencoding::encode(&self.client_id),
                    urlencoding::encode(&self.redirect_uri),
                    urlencoding::encode(state),
                )
            }
            OAuthProvider::GitHub => {
                format!(
                    "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&state={}&scope=read:user",
                    urlencoding::encode(&self.client_id),
                    urlencoding::encode(&self.redirect_uri),
                    urlencoding::encode(state),
                )
            }
        }
    }

    /// Exchange an authorization code for an access token.
    ///
    /// Returns the access token string on success.
    pub async fn exchange_code(&self, code: &str) -> Result<String, String> {
        match self.provider {
            OAuthProvider::Orcid => self.exchange_orcid_code(code).await,
            OAuthProvider::GitHub => self.exchange_github_code(code).await,
        }
    }

    async fn exchange_orcid_code(&self, code: &str) -> Result<String, String> {
        let token_url = if std::env::var("ORCID_SANDBOX").is_ok() {
            "https://sandbox.orcid.org/oauth/token"
        } else {
            "https://orcid.org/oauth/token"
        };

        let client = reqwest::Client::new();
        let params = [
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", self.redirect_uri.as_str()),
        ];

        let resp = client
            .post(token_url)
            .form(&params)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| format!("ORCID token request failed: {e}"))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("ORCID token parse failed: {e}"))?;

        body["access_token"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| format!("ORCID token response missing access_token: {}", body))
    }

    async fn exchange_github_code(&self, code: &str) -> Result<String, String> {
        let client = reqwest::Client::new();
        let params = [
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("code", code),
            ("redirect_uri", self.redirect_uri.as_str()),
        ];

        let resp = client
            .post("https://github.com/login/oauth/access_token")
            .form(&params)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| format!("GitHub token request failed: {e}"))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("GitHub token parse failed: {e}"))?;

        body["access_token"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| format!("GitHub token response missing access_token: {}", body))
    }

    /// Fetch user identity from the provider using an access token.
    ///
    /// Returns (provider_user_id, username).
    pub async fn fetch_identity(&self, access_token: &str) -> Result<(String, String), String> {
        match self.provider {
            OAuthProvider::Orcid => {
                let client = reqwest::Client::new();
                let orcid_api = if std::env::var("ORCID_SANDBOX").is_ok() {
                    "https://pub.sandbox.orcid.org/v3.0"
                } else {
                    "https://pub.orcid.org/v3.0"
                };

                let resp = client
                    .get(format!("{orcid_api}/{access_token}/record"))
                    .header("Accept", "application/json")
                    .send()
                    .await
                    .map_err(|e| format!("ORCID identity request failed: {e}"))?;

                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| format!("ORCID identity parse failed: {e}"))?;

                let orcid_id = body["orcid-identifier"]["path"]
                    .as_str()
                    .unwrap_or(access_token)
                    .to_string();

                let name = body["person"]["name"]["given-names"]["value"]
                    .as_str()
                    .unwrap_or("Unknown")
                    .to_string();

                Ok((orcid_id, name))
            }
            OAuthProvider::GitHub => {
                let client = reqwest::Client::new();
                let resp = client
                    .get("https://api.github.com/user")
                    .header("Authorization", format!("Bearer {access_token}"))
                    .header("User-Agent", "oxo-flow")
                    .send()
                    .await
                    .map_err(|e| format!("GitHub identity request failed: {e}"))?;

                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| format!("GitHub identity parse failed: {e}"))?;

                let github_id = body["id"]
                    .as_u64()
                    .map(|id| id.to_string())
                    .unwrap_or_default();
                let login = body["login"].as_str().unwrap_or("unknown").to_string();

                Ok((github_id, login))
            }
        }
    }
}

/// Generate an invite code for team enrollment.
///
/// Invite codes are used in air-gapped environments where OAuth2 providers
/// are unreachable.
pub fn generate_invite_code() -> String {
    use rand::RngExt;
    let chars: Vec<char> = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789".chars().collect();
    let mut rng = rand::rng();
    (0..12)
        .map(|_| chars[rng.random_range(0..chars.len())])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_authorize_url_orcid() {
        let config = OAuthConfig::orcid("test-client", "test-secret", "http://localhost/callback");
        let url = config.authorize_url("state123");
        assert!(url.contains("orcid.org/oauth/authorize"));
        assert!(url.contains("client_id=test-client"));
        assert!(url.contains("state=state123"));
    }

    #[test]
    fn test_authorize_url_github() {
        let config = OAuthConfig::github("gh-client", "gh-secret", "http://localhost/callback");
        let url = config.authorize_url("state456");
        assert!(url.contains("github.com/login/oauth/authorize"));
        assert!(url.contains("client_id=gh-client"));
        assert!(url.contains("state=state456"));
        assert!(url.contains("scope=read:user"));
    }

    #[test]
    fn test_generate_invite_code() {
        let code1 = generate_invite_code();
        let code2 = generate_invite_code();
        assert_eq!(code1.len(), 12);
        assert_eq!(code2.len(), 12);
        assert_ne!(code1, code2);
        // Should only contain allowed characters
        for c in code1.chars() {
            assert!("ABCDEFGHJKLMNPQRSTUVWXYZ23456789".contains(c));
        }
    }

    #[test]
    fn test_oauth_config_orcid() {
        let config = OAuthConfig::orcid("id", "secret", "https://example.com/cb");
        assert_eq!(config.provider, OAuthProvider::Orcid);
        assert_eq!(config.client_id, "id");
    }

    #[test]
    fn test_oauth_config_github() {
        let config = OAuthConfig::github("id", "secret", "https://example.com/cb");
        assert_eq!(config.provider, OAuthProvider::GitHub);
        assert_eq!(config.client_id, "id");
    }
}
