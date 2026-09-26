//! Cluster connection types — SSH endpoints for remote servers/clusters.

use serde::{Deserialize, Serialize};

/// One remote cluster/server connection (SSH endpoint + scheduler).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ClusterInfo {
    pub id: String,
    pub name: String,
    pub ssh_host: String,
    pub ssh_port: u16,
    pub ssh_user: Option<String>,
    /// SSH key path/material as stored. Never serialized to API responses
    /// (#517) — the field exists so the probe/submit paths can use the
    /// credential in-process; clients see [`ssh_key_set`] instead.
    #[serde(skip_serializing)]
    pub ssh_key: Option<String>,
    /// Whether an SSH key is configured (the key itself is not returned).
    pub ssh_key_set: bool,
    /// slurm | pbs | lsf | sge | auto
    pub scheduler: Option<String>,
    pub remote_dir: Option<String>,
    pub enabled: bool,
    pub created_at: String,
}

/// Create/update payload for the clusters API.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct ClusterUpsertRequest {
    pub id: String,
    pub name: String,
    pub ssh_host: String,
    #[serde(default = "default_port")]
    pub ssh_port: u16,
    pub ssh_user: Option<String>,
    pub ssh_key: Option<String>,
    pub scheduler: Option<String>,
    pub remote_dir: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_port() -> u16 {
    22
}
fn default_true() -> bool {
    true
}

/// Result of an SSH probe against a cluster endpoint.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ClusterProbeResult {
    pub ok: bool,
    pub hostname: Option<String>,
    /// Detected scheduler on the remote (or "none").
    pub scheduler: Option<String>,
    pub version: Option<String>,
    pub error: Option<String>,
    /// Probe duration in milliseconds.
    pub duration_ms: u64,
}
