//! Platform configuration file — server-tier settings and SSH cluster
//! definitions, editable as a TOML file (lowest precedence) and via the
//! web Settings/Clusters pages (DB tier, wins at runtime).
//!
//! Resolution order for server settings: CLI flag, then env var, then
//! config file, then built-in default. AI provider: env (registry init),
//! then DB (user settings), then config file. Clusters: the file is
//! imported into the DB table at startup (idempotent by id), then managed
//! from the UI.

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebConfig {
    #[serde(default)]
    pub server: ServerSection,
    #[serde(default)]
    pub ai: AiSection,
    #[serde(default)]
    pub clusters: Vec<ClusterDefinition>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSection {
    pub mode: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub base_path: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiSection {
    /// anthropic | openai | deepseek | ollama | disabled
    pub provider: Option<String>,
    pub api_url: Option<String>,
    pub model: Option<String>,
    /// Environment variable that holds the API key — secrets are never
    /// stored inline in the config file.
    pub api_key_env: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterDefinition {
    /// Stable identifier — the import key (upsert on startup).
    pub id: String,
    pub name: String,
    pub ssh_host: String,
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    #[serde(default)]
    pub ssh_user: Option<String>,
    /// Path to an SSH private key (absolute or ~-prefixed).
    #[serde(default)]
    pub ssh_key: Option<String>,
    /// slurm | pbs | lsf | sge | auto
    #[serde(default)]
    pub scheduler: Option<String>,
    /// Remote working directory for submitted jobs.
    #[serde(default)]
    pub remote_dir: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_ssh_port() -> u16 {
    22
}
fn default_true() -> bool {
    true
}

/// Candidate config file locations, first hit wins:
/// 1. `OXO_FLOW_CONFIG` env var
/// 2. `oxo-flow.web.toml` in the current directory
/// 3. `~/.config/oxo-flow/web.toml`
pub fn load() -> Option<WebConfig> {
    let candidates: Vec<std::path::PathBuf> = std::env::var("OXO_FLOW_CONFIG")
        .ok()
        .map(std::path::PathBuf::from)
        .into_iter()
        .chain(std::iter::once(std::path::PathBuf::from("oxo-flow.web.toml")))
        .chain(home_config())
        .collect();

    for path in candidates {
        match std::fs::read_to_string(&path) {
            Ok(content) => match toml::from_str::<WebConfig>(&content) {
                Ok(config) => {
                    tracing::info!("Loaded platform config from {}", path.display());
                    return Some(config);
                }
                Err(e) => {
                    tracing::warn!(
                        "Platform config {} failed to parse ({e}) — ignored",
                        path.display()
                    );
                    return None;
                }
            },
            Err(_) => continue,
        }
    }
    None
}

fn home_config() -> Option<std::path::PathBuf> {
    std::env::var("HOME").ok().map(|home| {
        std::path::PathBuf::from(home).join(".config/oxo-flow/web.toml")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config_file() {
        let toml_content = r#"
[server]
mode = "team"
port = 9090
base_path = "/oxoflow"

[ai]
provider = "deepseek"
api_url = "https://api.deepseek.com"
model = "deepseek-v4-pro"
api_key_env = "DEEPSEEK_API_KEY"

[[clusters]]
id = "lab-slurm"
name = "Lab SLURM"
ssh_host = "login.lab.example.edu"
ssh_user = "bioinf"
ssh_key = "~/.ssh/id_ed25519"
scheduler = "slurm"
remote_dir = "~/oxo-flow-jobs"
"#;
        let config: WebConfig = toml::from_str(toml_content).expect("valid config");
        assert_eq!(config.server.mode.as_deref(), Some("team"));
        assert_eq!(config.server.port, Some(9090));
        assert_eq!(config.ai.provider.as_deref(), Some("deepseek"));
        assert_eq!(config.ai.api_key_env.as_deref(), Some("DEEPSEEK_API_KEY"));
        assert_eq!(config.clusters.len(), 1);
        let cluster = &config.clusters[0];
        assert_eq!(cluster.ssh_port, 22);
        assert_eq!(cluster.scheduler.as_deref(), Some("slurm"));
        assert!(cluster.enabled);
    }

    #[test]
    fn empty_sections_default() {
        let config: WebConfig = toml::from_str("[server]\n").expect("valid");
        assert_eq!(config.server.port, None);
        assert!(config.clusters.is_empty());
    }

    #[test]
    fn unknown_fields_are_rejected_not_silently_ignored() {
        // Config typos must fail loudly, not vanish (issue #79's silent-
        // failure class).
        let result = toml::from_str::<WebConfig>("[server]\nprot = 9999\n");
        assert!(result.is_err());
    }
}
