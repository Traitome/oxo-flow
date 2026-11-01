//! Multi-tier storage hierarchy ladder for high-throughput bioinformatics I/O.
//!
//! Tiers represent storage medium locality and bandwidth:
//! - Tier 0: In-memory RAM disk / tmpfs (lowest latency, highest IOPS, capacity-constrained)
//! - Tier 1: Local node NVMe / SSD scratch (high IOPS, moderate capacity)
//! - Tier 2: Shared cluster distributed filesystem (Lustre, GPFS, BeeGFS, NFS)
//! - Tier 3: Cloud remote object storage (S3, GCS, Azure Blob)

use std::path::{Path, PathBuf};

use crate::scheduler::check_available_disk_mb;

/// Storage tier locality classification.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum StorageTier {
    /// In-memory RAM scratch (/dev/shm or tmpfs).
    Tier0Ram = 0,
    /// Fast local NVMe scratch.
    Tier1Nvme = 1,
    /// Cluster shared parallel filesystem scratch.
    Tier2Shared = 2,
    /// Remote cloud object storage.
    Tier3Cloud = 3,
}

impl StorageTier {
    /// Canonical identifier for the tier.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tier0Ram => "tier0_ram",
            Self::Tier1Nvme => "tier1_nvme",
            Self::Tier2Shared => "tier2_shared",
            Self::Tier3Cloud => "tier3_cloud",
        }
    }
}

/// Multi-tier storage ladder configuration and allocation engine.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct StorageTierLadder {
    /// Tier 0 RAM scratch directory.
    pub tier0_ram_scratch: Option<PathBuf>,
    /// Tier 1 local NVMe scratch directory.
    pub tier1_nvme_scratch: Option<PathBuf>,
    /// Tier 2 shared cluster scratch directory.
    pub tier2_shared_scratch: Option<PathBuf>,
    /// Tier 3 remote cloud storage URI prefix.
    pub tier3_cloud_remote: Option<String>,
}

impl StorageTierLadder {
    /// Create a new ladder with explicit tier specifications.
    pub fn new(
        tier0: Option<impl Into<PathBuf>>,
        tier1: Option<impl Into<PathBuf>>,
        tier2: Option<impl Into<PathBuf>>,
        tier3: Option<impl Into<String>>,
    ) -> Self {
        Self {
            tier0_ram_scratch: tier0.map(Into::into),
            tier1_nvme_scratch: tier1.map(Into::into),
            tier2_shared_scratch: tier2.map(Into::into),
            tier3_cloud_remote: tier3.map(Into::into),
        }
    }

    /// Construct ladder from workflow `EngineConfig`.
    pub fn from_engine_config(engine: Option<&crate::config::EngineConfig>) -> Self {
        let Some(cfg) = engine else {
            return Self::default();
        };

        Self {
            tier0_ram_scratch: cfg.tier0_ram_scratch.as_ref().map(PathBuf::from),
            tier1_nvme_scratch: cfg.tier1_nvme_scratch.as_ref().map(PathBuf::from),
            tier2_shared_scratch: cfg.tier2_shared_scratch.as_ref().map(PathBuf::from),
            tier3_cloud_remote: None,
        }
    }

    /// Select the highest-performance tier with sufficient free capacity.
    ///
    /// If `allow_ram` is true and Tier 0 is configured with `>= required_mb`,
    /// Tier 0 is selected. Otherwise Tier 1, then Tier 2 are probed.
    /// Returns `None` if no configured tier has sufficient capacity.
    ///
    /// When the capacity probe fails (e.g. the path does not yet exist or
    /// statfs returns an error), the tier is **skipped** with a `tracing`
    /// warning rather than silently selected without verifying capacity.
    /// This prevents routing a large scratch job onto a nearly-full disk.
    pub fn select_tier_for_scratch(
        &self,
        required_mb: u64,
        allow_ram: bool,
    ) -> Option<(StorageTier, PathBuf)> {
        if allow_ram && let Some(ref ram_path) = self.tier0_ram_scratch {
            match probe_available_mb(ram_path) {
                Some(avail) if avail >= required_mb => {
                    return Some((StorageTier::Tier0Ram, ram_path.clone()));
                }
                Some(_) => {} // Insufficient capacity — fall through.
                None => {
                    tracing::warn!(
                        "storage-tier: cannot probe capacity of '{}'; \
                         skipping Tier0Ram for {} MB scratch",
                        ram_path.display(),
                        required_mb,
                    );
                }
            }
        }

        if let Some(ref nvme_path) = self.tier1_nvme_scratch {
            match probe_available_mb(nvme_path) {
                Some(avail) if avail >= required_mb => {
                    return Some((StorageTier::Tier1Nvme, nvme_path.clone()));
                }
                Some(_) => {}
                None => {
                    tracing::warn!(
                        "storage-tier: cannot probe capacity of '{}'; \
                         skipping Tier1Nvme for {} MB scratch",
                        nvme_path.display(),
                        required_mb,
                    );
                }
            }
        }

        if let Some(ref shared_path) = self.tier2_shared_scratch {
            match probe_available_mb(shared_path) {
                Some(avail) if avail >= required_mb => {
                    return Some((StorageTier::Tier2Shared, shared_path.clone()));
                }
                Some(_) => {}
                None => {
                    tracing::warn!(
                        "storage-tier: cannot probe capacity of '{}'; \
                         skipping Tier2Shared for {} MB scratch",
                        shared_path.display(),
                        required_mb,
                    );
                }
            }
        }

        None
    }
}

fn probe_available_mb(path: &Path) -> Option<u64> {
    if path.exists() {
        check_available_disk_mb(path)
    } else if let Some(parent) = path.parent() {
        if parent.exists() {
            check_available_disk_mb(parent)
        } else {
            None
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// When the capacity probe fails, the tier must NOT be silently selected
    /// (regression guard for the capacity-bypass bug).
    #[test]
    fn probe_failure_does_not_select_tier() {
        let ladder = StorageTierLadder {
            tier0_ram_scratch: Some(PathBuf::from("/nonexistent-dir-xyz/ram")),
            tier1_nvme_scratch: Some(PathBuf::from("/nonexistent-dir-xyz/nvme")),
            tier2_shared_scratch: Some(PathBuf::from("/nonexistent-dir-xyz/shared")),
            ..Default::default()
        };
        // All probes fail → None, not a silent fallback.
        assert!(ladder.select_tier_for_scratch(1024, true).is_none());
    }

    /// When no tiers are configured at all, selection returns None.
    #[test]
    fn empty_ladder_returns_none() {
        let ladder = StorageTierLadder::default();
        assert!(ladder.select_tier_for_scratch(1024, true).is_none());
    }
}
