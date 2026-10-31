//! NUMA topology discovery and affinity allocation.

use std::path::Path;

/// A single NUMA locality domain.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NumaNode {
    /// Logical NUMA node index (e.g. 0, 1).
    pub id: u32,
    /// Logical CPU core indices associated with this node.
    pub cpus: Vec<usize>,
    /// Available memory on this NUMA node in megabytes.
    pub memory_mb: u64,
}

/// Physical machine hardware topology representation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HardwareTopology {
    /// Discovered NUMA nodes.
    pub numa_nodes: Vec<NumaNode>,
    /// Total system memory across all nodes in megabytes.
    pub total_memory_mb: u64,
    /// L3 cache size in megabytes.
    pub cache_l3_mb: u64,
}

impl Default for HardwareTopology {
    fn default() -> Self {
        Self::discover()
    }
}

impl HardwareTopology {
    /// Discover hardware topology by interrogating Linux sysfs or macOS sysctl.
    pub fn discover() -> Self {
        #[cfg(target_os = "linux")]
        {
            if let Some(topo) = Self::discover_linux() {
                return topo;
            }
        }

        #[cfg(target_os = "macos")]
        {
            if let Some(topo) = Self::discover_macos() {
                return topo;
            }
        }

        Self::fallback()
    }

    /// Linux sysfs NUMA node discovery under `/sys/devices/system/node/`.
    pub fn discover_from_sysfs(node_dir: &Path) -> Option<Self> {
        if !node_dir.is_dir() {
            return None;
        }

        let mut nodes = Vec::new();
        let mut total_memory_mb = 0u64;

        let entries = std::fs::read_dir(node_dir).ok()?;
        for entry in entries.flatten() {
            let filename = entry.file_name();
            let name = filename.to_string_lossy();
            if let Some(stripped) = name.strip_prefix("node")
                && let Ok(id) = stripped.parse::<u32>()
            {
                let path = entry.path();
                let cpus = read_cpulist(&path.join("cpulist")).unwrap_or_default();
                let mem_mb = read_node_meminfo(&path.join("meminfo")).unwrap_or(0);
                total_memory_mb += mem_mb;
                nodes.push(NumaNode {
                    id,
                    cpus,
                    memory_mb: mem_mb,
                });
            }
        }

        if nodes.is_empty() {
            return None;
        }

        nodes.sort_by_key(|n| n.id);
        Some(Self {
            numa_nodes: nodes,
            total_memory_mb,
            cache_l3_mb: 32,
        })
    }

    #[cfg(target_os = "linux")]
    fn discover_linux() -> Option<Self> {
        Self::discover_from_sysfs(Path::new("/sys/devices/system/node"))
    }

    #[cfg(target_os = "macos")]
    fn discover_macos() -> Option<Self> {
        let ncpu = sysctl_u64("hw.ncpu").unwrap_or(1) as usize;
        let mem = sysctl_u64("hw.memsize").unwrap_or(4 * 1024 * 1024 * 1024) / (1024 * 1024);
        let l3 = sysctl_u64("hw.l3cachesize").unwrap_or(16 * 1024 * 1024) / (1024 * 1024);
        Some(Self {
            numa_nodes: vec![NumaNode {
                id: 0,
                cpus: (0..ncpu).collect(),
                memory_mb: mem,
            }],
            total_memory_mb: mem,
            cache_l3_mb: l3,
        })
    }

    /// Single-node fallback for virtualized or unsupported environments.
    pub fn fallback() -> Self {
        let ncpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let mem_mb = 4096;
        Self {
            numa_nodes: vec![NumaNode {
                id: 0,
                cpus: (0..ncpus).collect(),
                memory_mb: mem_mb,
            }],
            total_memory_mb: mem_mb,
            cache_l3_mb: 16,
        }
    }

    /// Select the best NUMA node for a job requiring `threads` and `memory_mb`.
    pub fn select_node_for_job(&self, _threads: u32, memory_mb: Option<u64>) -> Option<u32> {
        if self.numa_nodes.is_empty() {
            return None;
        }
        if self.numa_nodes.len() == 1 {
            return Some(self.numa_nodes[0].id);
        }

        if let Some(req_mb) = memory_mb {
            // Pick the first node that fits the memory requirement
            if let Some(node) = self.numa_nodes.iter().find(|n| n.memory_mb >= req_mb) {
                return Some(node.id);
            }
        }

        // Default to node with maximum memory
        self.numa_nodes
            .iter()
            .max_by_key(|n| n.memory_mb)
            .map(|n| n.id)
    }

    /// Generate command wrapper with numactl.
    pub fn wrap_command_with_numa(&self, cmd: &str, node_id: u32) -> String {
        format!("numactl --cpunodebind={node_id} --membind={node_id} -- {cmd}")
    }
}

/// Parse Linux `cpulist` syntax (e.g. "0-3,8-11" -> [0, 1, 2, 3, 8, 9, 10, 11]).
pub fn parse_cpulist(s: &str) -> Vec<usize> {
    let mut cpus = Vec::new();
    for part in s.trim().split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((start, end)) = part.split_once('-') {
            if let (Ok(a), Ok(b)) = (start.parse::<usize>(), end.parse::<usize>()) {
                cpus.extend(a..=b);
            }
        } else if let Ok(n) = part.parse::<usize>() {
            cpus.push(n);
        }
    }
    cpus.sort_unstable();
    cpus.dedup();
    cpus
}

fn read_cpulist(path: &Path) -> Option<Vec<usize>> {
    let content = std::fs::read_to_string(path).ok()?;
    Some(parse_cpulist(&content))
}

/// Parse `MemTotal` from a NUMA node `meminfo` file.
pub fn parse_node_meminfo(content: &str) -> Option<u64> {
    for line in content.lines() {
        if line.contains("MemTotal:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                // Format: "Node X MemTotal: 16384000 kB"
                if let Ok(kb) = parts[3].parse::<u64>() {
                    return Some(kb / 1024);
                }
            } else if parts.len() >= 2 {
                // Format: "MemTotal: 16384000 kB"
                if let Ok(kb) = parts[1].parse::<u64>() {
                    return Some(kb / 1024);
                }
            }
        }
    }
    None
}

fn read_node_meminfo(path: &Path) -> Option<u64> {
    let content = std::fs::read_to_string(path).ok()?;
    parse_node_meminfo(&content)
}

#[cfg(target_os = "macos")]
fn sysctl_u64(name: &str) -> Option<u64> {
    let out = std::process::Command::new("sysctl")
        .arg("-n")
        .arg(name)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    s.trim().parse::<u64>().ok()
}
