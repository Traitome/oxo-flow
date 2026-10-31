//! Hardware topology discovery, NUMA affinity, and multi-tier storage ladder.

pub mod numa;
pub mod storage_tiers;

pub use numa::{HardwareTopology, NumaNode};
pub use storage_tiers::{StorageTier, StorageTierLadder};

#[cfg(test)]
mod numa_tests;
