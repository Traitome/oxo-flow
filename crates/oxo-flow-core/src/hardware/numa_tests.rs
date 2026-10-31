//! Tests for hardware topology discovery, NUMA affinity, and storage tier ladder.

use std::path::PathBuf;

use super::numa::{HardwareTopology, NumaNode, parse_cpulist, parse_node_meminfo};
use super::storage_tiers::{StorageTier, StorageTierLadder};

#[test]
fn test_parse_cpulist_syntax() {
    assert_eq!(parse_cpulist(""), Vec::<usize>::new());
    assert_eq!(parse_cpulist("0"), vec![0]);
    assert_eq!(parse_cpulist("0-3"), vec![0, 1, 2, 3]);
    assert_eq!(parse_cpulist("0,2,4"), vec![0, 2, 4]);
    assert_eq!(parse_cpulist("0-2,5,8-10"), vec![0, 1, 2, 5, 8, 9, 10]);
    assert_eq!(parse_cpulist("4-5,1-2,2"), vec![1, 2, 4, 5]);
}

#[test]
fn test_parse_node_meminfo() {
    let meminfo_text = "\
Node 0 MemTotal:       32907404 kB
Node 0 MemFree:        12345678 kB
Node 0 MemUsed:        20561726 kB
";
    assert_eq!(parse_node_meminfo(meminfo_text), Some(32136));

    let standard_meminfo = "MemTotal:       16777216 kB\nMemFree: 1000 kB\n";
    assert_eq!(parse_node_meminfo(standard_meminfo), Some(16384));

    assert_eq!(parse_node_meminfo("Invalid data without total"), None);
}

#[test]
fn test_select_node_for_job() {
    let topo = HardwareTopology {
        numa_nodes: vec![
            NumaNode {
                id: 0,
                cpus: vec![0, 1, 2, 3],
                memory_mb: 16384,
            },
            NumaNode {
                id: 1,
                cpus: vec![4, 5, 6, 7],
                memory_mb: 65536,
            },
        ],
        total_memory_mb: 81920,
        cache_l3_mb: 64,
    };

    // If required memory fits only on node 1
    assert_eq!(topo.select_node_for_job(4, Some(32768)), Some(1));

    // If no memory requested, picks node with maximum memory
    assert_eq!(topo.select_node_for_job(2, None), Some(1));

    // Single node returns that node
    let single = HardwareTopology {
        numa_nodes: vec![NumaNode {
            id: 0,
            cpus: vec![0, 1],
            memory_mb: 8192,
        }],
        total_memory_mb: 8192,
        cache_l3_mb: 16,
    };
    assert_eq!(single.select_node_for_job(1, Some(4096)), Some(0));
}

#[test]
fn test_wrap_command_with_numa() {
    let topo = HardwareTopology::fallback();
    let wrapped = topo.wrap_command_with_numa("bwa mem ref.fa reads.fq", 1);
    assert_eq!(
        wrapped,
        "numactl --cpunodebind=1 --membind=1 -- bwa mem ref.fa reads.fq"
    );
}

#[test]
fn test_storage_tier_ladder_selection() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ram_scratch = dir.path().join("ram");
    let nvme_scratch = dir.path().join("nvme");
    let shared_scratch = dir.path().join("shared");

    std::fs::create_dir_all(&ram_scratch).unwrap();
    std::fs::create_dir_all(&nvme_scratch).unwrap();
    std::fs::create_dir_all(&shared_scratch).unwrap();

    let ladder = StorageTierLadder::new(
        Some(ram_scratch.clone()),
        Some(nvme_scratch.clone()),
        Some(shared_scratch.clone()),
        Some("s3://bucket/scratch"),
    );

    // When allow_ram = true, selects Tier 0 (RAM)
    let (tier, path) = ladder
        .select_tier_for_scratch(1024, true)
        .expect("ram selection");
    assert_eq!(tier, StorageTier::Tier0Ram);
    assert_eq!(path, ram_scratch);

    // When allow_ram = false, skips RAM and selects Tier 1 (NVMe)
    let (tier, path) = ladder
        .select_tier_for_scratch(1024, false)
        .expect("nvme selection");
    assert_eq!(tier, StorageTier::Tier1Nvme);
    assert_eq!(path, nvme_scratch);
}

#[test]
fn test_storage_tier_ladder_from_engine_config() {
    let engine = crate::config::EngineConfig {
        tier0_ram_scratch: Some("/dev/shm".to_string()),
        tier1_nvme_scratch: Some("/mnt/nvme/scratch".to_string()),
        tier2_shared_scratch: Some("/shared/scratch".to_string()),
        ..Default::default()
    };

    let ladder = StorageTierLadder::from_engine_config(Some(&engine));
    assert_eq!(ladder.tier0_ram_scratch, Some(PathBuf::from("/dev/shm")));
    assert_eq!(
        ladder.tier1_nvme_scratch,
        Some(PathBuf::from("/mnt/nvme/scratch"))
    );
    assert_eq!(
        ladder.tier2_shared_scratch,
        Some(PathBuf::from("/shared/scratch"))
    );
    assert_eq!(ladder.tier3_cloud_remote, None);
    assert_eq!(StorageTier::Tier0Ram.as_str(), "tier0_ram");
    assert_eq!(StorageTier::Tier1Nvme.as_str(), "tier1_nvme");
    assert_eq!(StorageTier::Tier2Shared.as_str(), "tier2_shared");
    assert_eq!(StorageTier::Tier3Cloud.as_str(), "tier3_cloud");
}
