use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use sysinfo::{ProcessesToUpdate, System};

static SYS: LazyLock<Mutex<System>> = LazyLock::new(|| Mutex::new(System::new_all()));

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostResources {
    pub cpu_usage_percent: f32,
    pub total_memory_mb: u64,
    pub used_memory_mb: u64,
    pub total_swap_mb: u64,
    pub used_swap_mb: u64,
}

/// Retrieve current host resource metrics.
///
/// # Panics
/// Panics if the system lock is poisoned (which should never happen in normal operation).
pub fn get_host_resources() -> HostResources {
    let mut sys = SYS.lock().expect("System lock should never be poisoned");
    sys.refresh_cpu_usage();
    sys.refresh_memory();

    let cpu_usage_percent = sys.global_cpu_usage();
    let total_memory_mb = sys.total_memory() / 1024 / 1024;
    let used_memory_mb = sys.used_memory() / 1024 / 1024;
    let total_swap_mb = sys.total_swap() / 1024 / 1024;
    let used_swap_mb = sys.used_swap() / 1024 / 1024;

    HostResources {
        cpu_usage_percent,
        total_memory_mb,
        used_memory_mb,
        total_swap_mb,
        used_swap_mb,
    }
}

/// Aggregate memory/cpu/process-count of a process tree rooted at
/// `root_pid` (issue #82 P1-2: per-run resource sampling). Walks
/// parent→child links in one sysinfo refresh.
///
/// Returns `(memory_mb, cpu_pct, process_count)`.
pub fn process_tree_usage(root_pid: u32) -> Option<(f64, f64, usize)> {
    let mut sys = SYS.lock().ok()?;
    sys.refresh_processes(ProcessesToUpdate::All, true);

    // parent map: pid -> parent pid
    let parents: HashMap<sysinfo::Pid, sysinfo::Pid> = sys
        .processes()
        .iter()
        .filter_map(|(pid, p)| p.parent().map(|parent| (*pid, parent)))
        .collect();

    let mut memory_bytes: u64 = 0;
    let mut cpu: f32 = 0.0;
    let mut count = 0;
    let mut seen: HashSet<sysinfo::Pid> = HashSet::new();
    let mut frontier: Vec<sysinfo::Pid> = vec![sysinfo::Pid::from_u32(root_pid)];
    while let Some(pid) = frontier.pop() {
        if !seen.insert(pid) {
            continue;
        }
        for (child, parent) in &parents {
            if *parent == pid {
                frontier.push(*child);
            }
        }
        if let Some(proc) = sys.process(pid) {
            memory_bytes += proc.memory();
            cpu += proc.cpu_usage();
            count += 1;
        }
    }
    if count == 0 {
        return None;
    }
    Some((memory_bytes as f64 / 1024.0 / 1024.0, cpu as f64, count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_resources_returns_valid_values() {
        let resources = get_host_resources();
        assert!(resources.cpu_usage_percent >= 0.0);
        assert!(resources.cpu_usage_percent <= 100.0);
        assert!(resources.total_memory_mb > 0);
        assert!(resources.used_memory_mb <= resources.total_memory_mb);
    }

    #[test]
    fn host_resources_serialization() {
        let resources = get_host_resources();
        let json = serde_json::to_string(&resources).unwrap();
        assert!(json.contains("cpu_usage_percent"));
        assert!(json.contains("total_memory_mb"));
        let parsed: HostResources = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.total_memory_mb, resources.total_memory_mb);
    }
}
