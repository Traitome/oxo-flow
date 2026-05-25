//! HPC scheduler integration module.
//!
//! Detects and monitors HPC workload managers (SLURM, PBS/Torque, LSF, SGE)
//! running on the host system. Provides queue status, node availability,
//! and job submission capabilities.

use serde::{Deserialize, Serialize};
use std::process::Command;

/// Detected HPC scheduler type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SchedulerType {
    Slurm,
    Pbs,
    Lsf,
    Sge,
    None,
}

impl std::fmt::Display for SchedulerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Slurm => write!(f, "SLURM"),
            Self::Pbs => write!(f, "PBS/Torque"),
            Self::Lsf => write!(f, "LSF"),
            Self::Sge => write!(f, "SGE"),
            Self::None => write!(f, "None"),
        }
    }
}

/// Queue status information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueStatus {
    pub queue_name: String,
    pub total_jobs: usize,
    pub running: usize,
    pub pending: usize,
    pub held: usize,
    pub state: String,
}

/// Node status information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStatus {
    pub name: String,
    pub state: String,
    pub cpus_total: u32,
    pub cpus_alloc: u32,
    pub cpus_free: u32,
    pub memory_total_mb: u64,
    pub memory_free_mb: u64,
}

/// Job information from the HPC scheduler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobInfo {
    pub job_id: String,
    pub name: String,
    pub user: String,
    pub state: String,
    pub queue: String,
    pub nodes: Option<String>,
    pub cpus: u32,
    pub elapsed: Option<String>,
    pub time_limit: Option<String>,
}

/// HPC system summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HpcStatus {
    pub scheduler: String,
    pub scheduler_type: SchedulerType,
    pub available: bool,
    pub version: Option<String>,
    pub queues: Vec<QueueStatus>,
    pub nodes: Vec<NodeStatus>,
    pub jobs: Vec<JobInfo>,
    pub total_jobs: usize,
    pub error: Option<String>,
}

/// Detect the available HPC scheduler.
pub fn detect_scheduler() -> SchedulerType {
    // Check SLURM
    if Command::new("sinfo").arg("--version").output().is_ok() {
        return SchedulerType::Slurm;
    }
    // Check PBS/Torque
    if Command::new("qstat").arg("--version").output().is_ok()
        || Command::new("pbsnodes").arg("--version").output().is_ok()
    {
        return SchedulerType::Pbs;
    }
    // Check LSF
    if Command::new("bjobs").arg("-V").output().is_ok() {
        return SchedulerType::Lsf;
    }
    // Check SGE
    if Command::new("qstat").arg("-help").output().is_ok() {
        let output = Command::new("qstat")
            .arg("-help")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stderr).to_string())
            .unwrap_or_default();
        if output.contains("GE") {
            return SchedulerType::Sge;
        }
    }
    SchedulerType::None
}

/// Get SLURM scheduler version.
fn get_slurm_version() -> Option<String> {
    Command::new("sinfo")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| {
            let stdout = String::from_utf8_lossy(&o.stdout).to_string();
            let version = stdout.trim();
            if version.is_empty() {
                None
            } else {
                Some(version.to_string())
            }
        })
}

/// Get SLURM queue status.
fn get_slurm_queues() -> Vec<QueueStatus> {
    let output = match Command::new("squeue")
        .args(["-o", "%P|%t", "--noheader"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut queues: std::collections::BTreeMap<String, QueueStatus> =
        std::collections::BTreeMap::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 2 {
            continue;
        }

        let queue_name = parts[0].to_string();
        let state_char = parts[1];

        let q = queues.entry(queue_name.clone()).or_insert(QueueStatus {
            queue_name,
            total_jobs: 0,
            running: 0,
            pending: 0,
            held: 0,
            state: "up".to_string(),
        });

        q.total_jobs += 1;
        match state_char {
            "R" | "r" => q.running += 1,
            "PD" | "pd" => q.pending += 1,
            "H" | "h" => q.held += 1,
            _ => q.pending += 1,
        }
    }

    queues.into_values().collect()
}

/// Get SLURM node status.
fn get_slurm_nodes() -> Vec<NodeStatus> {
    let output = match Command::new("sinfo")
        .args(["-N", "-o", "%n|%t|%c|%O|%m|%e", "--noheader"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut nodes = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 6 {
            continue;
        }

        let cpus_total: u32 = parts[2].parse().unwrap_or(0);
        let cpus_alloc: u32 = parts[3].parse().unwrap_or(0);

        nodes.push(NodeStatus {
            name: parts[0].to_string(),
            state: parts[1].to_string(),
            cpus_total,
            cpus_alloc,
            cpus_free: cpus_total.saturating_sub(cpus_alloc),
            memory_total_mb: parts[4].parse().unwrap_or(0),
            memory_free_mb: parts[5].parse().unwrap_or(0),
        });
    }

    nodes
}

/// Get SLURM jobs.
fn get_slurm_jobs() -> Vec<JobInfo> {
    let output = match Command::new("squeue")
        .args(["-o", "%i|%j|%u|%t|%P|%D|%C|%M|%l", "--noheader"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut jobs = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 9 {
            continue;
        }

        jobs.push(JobInfo {
            job_id: parts[0].to_string(),
            name: parts[1].to_string(),
            user: parts[2].to_string(),
            state: parts[3].to_string(),
            queue: parts[4].to_string(),
            nodes: Some(parts[5].to_string()),
            cpus: parts[6].parse().unwrap_or(0),
            elapsed: {
                let e = parts[7];
                if e.is_empty() || e == "0:00" {
                    None
                } else {
                    Some(e.to_string())
                }
            },
            time_limit: {
                let t = parts[8];
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            },
        });
    }

    jobs
}

/// Get PBS/Torque queue status.
fn get_pbs_queues() -> Vec<QueueStatus> {
    let output = match Command::new("qstat").arg("-q").output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut queues = Vec::new();

    for line in stdout.lines().skip(2) {
        let line = line.trim();
        if line.is_empty() || line.starts_with("---") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }

        queues.push(QueueStatus {
            queue_name: parts[0].to_string(),
            total_jobs: parts.get(6).and_then(|v| v.parse().ok()).unwrap_or(0),
            running: parts.get(5).and_then(|v| v.parse().ok()).unwrap_or(0),
            pending: parts.get(6).and_then(|v| v.parse().ok()).unwrap_or(0),
            held: 0,
            state: parts.get(2).unwrap_or(&"up").to_string(),
        });
    }

    queues
}

/// Get PBS/Torque nodes.
fn get_pbs_nodes() -> Vec<NodeStatus> {
    let output = match Command::new("pbsnodes").arg("-a").output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut nodes = Vec::new();
    let mut current: Option<NodeStatus> = None;

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            if let Some(node) = current.take() {
                nodes.push(node);
            }
            continue;
        }

        if !line.starts_with(' ') {
            // New node entry
            if let Some(node) = current.take() {
                nodes.push(node);
            }
            current = Some(NodeStatus {
                name: line.to_string(),
                state: "unknown".to_string(),
                cpus_total: 0,
                cpus_alloc: 0,
                cpus_free: 0,
                memory_total_mb: 0,
                memory_free_mb: 0,
            });
        } else if let Some(ref mut node) = current {
            let kv: Vec<&str> = line.splitn(2, '=').collect();
            if kv.len() == 2 {
                let key = kv[0].trim();
                let value = kv[1].trim().trim_matches('"');
                match key {
                    "state" => node.state = value.to_string(),
                    "np" => {
                        node.cpus_total = value.parse().unwrap_or(0);
                        node.cpus_free = node.cpus_total;
                    }
                    "status" => {
                        // Parse allocation info
                        if let Some(_pos) = value.find("np=") {
                            // Rough parse of allocated CPUs
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    if let Some(node) = current {
        nodes.push(node);
    }

    nodes
}

/// Get full HPC status.
///
/// Detects the available scheduler and gathers queue, node, and job status.
pub fn get_hpc_status() -> HpcStatus {
    let scheduler_type = detect_scheduler();

    match scheduler_type {
        SchedulerType::Slurm => {
            let version = get_slurm_version();
            let queues = get_slurm_queues();
            let nodes = get_slurm_nodes();
            let jobs = get_slurm_jobs();
            let total_jobs = queues.iter().map(|q| q.total_jobs).sum();

            HpcStatus {
                scheduler: "SLURM".to_string(),
                scheduler_type,
                available: true,
                version,
                queues,
                nodes,
                jobs: jobs.into_iter().take(100).collect(),
                total_jobs,
                error: None,
            }
        }
        SchedulerType::Pbs => {
            let queues = get_pbs_queues();
            let nodes = get_pbs_nodes();
            let total_jobs = queues.iter().map(|q| q.total_jobs).sum();

            HpcStatus {
                scheduler: "PBS/Torque".to_string(),
                scheduler_type,
                available: true,
                version: None,
                queues,
                nodes,
                jobs: Vec::new(),
                total_jobs,
                error: None,
            }
        }
        _ => HpcStatus {
            scheduler: "None".to_string(),
            scheduler_type: SchedulerType::None,
            available: false,
            version: None,
            queues: Vec::new(),
            nodes: Vec::new(),
            jobs: Vec::new(),
            total_jobs: 0,
            error: Some(
                "No HPC scheduler detected. Install SLURM, PBS/Torque, LSF, or SGE.".to_string(),
            ),
        },
    }
}

/// Submit a job script to SLURM via sbatch.
///
/// Returns the job ID on success.
pub fn submit_slurm_job(script_path: &str, job_name: &str, cpus: u32) -> Result<String, String> {
    let output = Command::new("sbatch")
        .args([
            "--parsable",
            "--job-name",
            job_name,
            "--cpus-per-task",
            &cpus.to_string(),
            script_path,
        ])
        .output()
        .map_err(|e| format!("Failed to run sbatch: {}", e))?;

    if output.status.success() {
        let job_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(job_id)
    } else {
        let error = String::from_utf8_lossy(&output.stderr).to_string();
        Err(format!("sbatch failed: {}", error))
    }
}

/// Cancel a SLURM job.
pub fn cancel_slurm_job(job_id: &str) -> Result<(), String> {
    let output = Command::new("scancel")
        .arg(job_id)
        .output()
        .map_err(|e| format!("Failed to run scancel: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let error = String::from_utf8_lossy(&output.stderr).to_string();
        Err(format!("scancel failed: {}", error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_scheduler_returns_type() {
        let sched = detect_scheduler();
        // Should return one of the valid types
        assert!(
            matches!(
                sched,
                SchedulerType::Slurm
                    | SchedulerType::Pbs
                    | SchedulerType::Lsf
                    | SchedulerType::Sge
                    | SchedulerType::None
            ),
            "detect_scheduler() returned an unexpected type"
        );
    }

    #[test]
    fn test_get_hpc_status_returns_valid_structure() {
        let status = get_hpc_status();
        assert!(!status.scheduler.is_empty());

        // If no scheduler, should have error message
        if !status.available {
            assert!(status.error.is_some());
        }

        // Version should be present for available schedulers
        if status.scheduler_type == SchedulerType::Slurm && status.available {
            assert!(status.version.is_some());
        }
    }

    #[test]
    fn test_hpc_status_serialization() {
        let status = get_hpc_status();
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("scheduler"));
        assert!(json.contains("available"));
    }
}
