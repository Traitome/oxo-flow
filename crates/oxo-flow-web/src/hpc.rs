//! HPC scheduler integration module.
//!
//! Detects and monitors HPC workload managers (SLURM, PBS/Torque, LSF, SGE)
//! running on the host system. Provides queue status, node availability,
//! and job submission capabilities.

use serde::{Deserialize, Serialize};
use std::process::Command;

/// Detected HPC scheduler type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct QueueStatus {
    pub queue_name: String,
    pub total_jobs: usize,
    pub running: usize,
    pub pending: usize,
    pub held: usize,
    pub state: String,
}

/// Node status information.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
    if command_succeeds("sinfo", &["--version"]) {
        return SchedulerType::Slurm;
    }
    // Check PBS/Torque — the probe must honor the exit status: SGE's qstat
    // also runs on `--version` but fails it, which would otherwise be
    // misdetected as PBS (and shadow the real SGE check below).
    if command_succeeds("qstat", &["--version"])
        || command_succeeds("pbsnodes", &["--version"])
    {
        return SchedulerType::Pbs;
    }
    // Check LSF
    if command_succeeds("bjobs", &["-V"]) {
        return SchedulerType::Lsf;
    }
    // Check SGE — `qstat -help` exits non-zero on SGE, so only the banner
    // text ("GE 8.6.x", "Sun Grid Engine") is authoritative here.
    if let Ok(output) = Command::new("qstat").arg("-help").output() {
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        if text.to_lowercase().contains("grid engine") {
            return SchedulerType::Sge;
        }
    }
    SchedulerType::None
}

/// Whether `program args` spawns and exits with status 0.
fn command_succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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

/// Parse `bhosts` output (LSF) into node status entries.
///
/// Columns: HOST_NAME STATUS JL/U MAX NJOBS RUN SSUSP USUSP RSV
fn parse_lsf_bhosts(stdout: &str) -> Vec<NodeStatus> {
    let mut nodes = Vec::new();
    for line in stdout.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let cpus_total: u32 = parts.get(3).and_then(|v| v.parse().ok()).unwrap_or(0);
        let running: u32 = parts.get(5).and_then(|v| v.parse().ok()).unwrap_or(0);
        nodes.push(NodeStatus {
            name: parts[0].to_string(),
            state: parts[1].to_string(),
            cpus_total,
            cpus_alloc: running,
            cpus_free: cpus_total.saturating_sub(running),
            memory_total_mb: 0,
            memory_free_mb: 0,
        });
    }
    nodes
}

/// Parse `bjobs` output (LSF) into job entries.
///
/// Columns: JOBID USER STAT QUEUE FROM_HOST EXEC_HOST JOB_NAME SUBMIT_TIME
fn parse_lsf_bjobs(stdout: &str) -> Vec<JobInfo> {
    let mut jobs = Vec::new();
    for line in stdout.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        jobs.push(JobInfo {
            job_id: parts[0].to_string(),
            user: parts[1].to_string(),
            state: parts[2].to_string(),
            queue: parts[3].to_string(),
            nodes: parts.get(5).map(|s| s.to_string()),
            name: parts.get(6).unwrap_or(&parts[0]).to_string(),
            cpus: 0,
            elapsed: None,
            time_limit: None,
        });
    }
    jobs
}

/// Parse `bqueues -w` output (LSF) into queue entries.
///
/// Columns: QUEUE_NAME PRIO STATUS MAX JL/U JL/P JL/H NJOBS PEND RUN SUSP
fn parse_lsf_bqueues(stdout: &str) -> Vec<QueueStatus> {
    let mut queues = Vec::new();
    for line in stdout.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 8 {
            continue;
        }
        queues.push(QueueStatus {
            queue_name: parts[0].to_string(),
            state: parts[2].to_string(),
            total_jobs: parts.get(7).and_then(|v| v.parse().ok()).unwrap_or(0),
            pending: parts.get(8).and_then(|v| v.parse().ok()).unwrap_or(0),
            running: parts.get(9).and_then(|v| v.parse().ok()).unwrap_or(0),
            held: 0,
        });
    }
    queues
}

/// Parse `qstat` (SGE) output into job entries.
///
/// Columns: job-ID prior name user state submit/start at queue slots ja-task-ID.
/// The submit time may be one or two tokens; queue and slots may be absent
/// for queued jobs, so the trailing columns are located after the state.
fn parse_sge_qstat(stdout: &str) -> Vec<JobInfo> {
    let mut jobs = Vec::new();
    let mut past_header = false;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("----") {
            past_header = true;
            continue;
        }
        if !past_header || trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 6 {
            continue;
        }
        // Columns 0..=4 are fixed: job-ID prior name user state.
        let mut queue = String::new();
        let mut nodes = None;
        let mut cpus = 0;
        let rest = &parts[5..];
        if rest.len() >= 2 && rest[1].len() >= 8 && rest[1].as_bytes()[2] == b':' {
            // Two-token submit time ("08/14/2026 10:00:00") — queue follows.
            if rest.len() >= 3 {
                queue = rest[2].to_string();
                if rest.len() >= 4 {
                    cpus = rest[3].parse().unwrap_or(0);
                }
            }
        } else if rest.len() >= 2 {
            queue = rest[1].to_string();
            if rest.len() >= 3 {
                cpus = rest[2].parse().unwrap_or(0);
            }
        }
        if let Some(host) = queue.split('@').nth(1) {
            nodes = Some(host.to_string());
        }
        jobs.push(JobInfo {
            job_id: parts[0].to_string(),
            name: parts[2].to_string(),
            user: parts[3].to_string(),
            state: parts[4].to_string(),
            queue,
            nodes,
            cpus,
            elapsed: None,
            time_limit: None,
        });
    }
    jobs
}

/// Parse a size string with a K/M/G/T suffix into megabytes.
fn parse_size_mb(s: &str) -> u64 {
    let s = s.trim();
    let Some((num, unit)) = s.split_at_checked(s.len().saturating_sub(1)) else {
        return 0;
    };
    let v: f64 = num.trim().parse().unwrap_or(0.0);
    let mult = match unit {
        "K" | "k" => 1.0 / 1024.0,
        "M" | "m" => 1.0,
        "G" | "g" => 1024.0,
        "T" | "t" => 1024.0 * 1024.0,
        _ => 1.0,
    };
    (v * mult) as u64
}

/// Parse `qhost` output (SGE) into node status entries.
///
/// Column sets vary across SGE versions (older ones lack NSOC/NCOR), so the
/// host/CPU columns are positional while memory columns are found by their
/// unit suffix: the last two unit-carrying tokens are MEMUSE and MEMTOT.
fn parse_sge_qhost(stdout: &str) -> Vec<NodeStatus> {
    let mut nodes = Vec::new();
    let mut past_header = false;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("----") {
            past_header = true;
            continue;
        }
        if !past_header || trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        let cpus_total: u32 = parts.get(2).and_then(|v| v.parse().ok()).unwrap_or(0);
        // The unit-suffixed tokens, from the end: MEMUSE then MEMTOT.
        // (SWAPTO/SWAPUS follow them without units, so a plain reverse
        // take-while would stop too early.)
        let sized: Vec<&str> = parts
            .iter()
            .rev()
            .filter(|t| matches!(t.chars().last(), Some('G' | 'M' | 'T' | 'K')))
            .copied()
            .collect();
        let (memory_total_mb, memory_free_mb) = match sized.as_slice() {
            [memuse, memtot, ..] => {
                let total = parse_size_mb(memtot);
                let used = parse_size_mb(memuse);
                (total, total.saturating_sub(used))
            }
            _ => (0, 0),
        };
        nodes.push(NodeStatus {
            name: parts[0].to_string(),
            state: "up".to_string(),
            cpus_total,
            cpus_alloc: 0,
            cpus_free: cpus_total,
            memory_total_mb,
            memory_free_mb,
        });
    }
    nodes
}

/// Collect node status from `bhosts` (LSF).
fn get_lsf_nodes() -> Vec<NodeStatus> {
    let output = match Command::new("bhosts").output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    parse_lsf_bhosts(&String::from_utf8_lossy(&output.stdout))
}

/// Collect job status from `bjobs` (LSF).
fn get_lsf_jobs() -> Vec<JobInfo> {
    let output = match Command::new("bjobs").output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    parse_lsf_bjobs(&String::from_utf8_lossy(&output.stdout))
}

/// Collect queue status from `bqueues -w` (LSF).
fn get_lsf_queues() -> Vec<QueueStatus> {
    let output = match Command::new("bqueues").arg("-w").output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    parse_lsf_bqueues(&String::from_utf8_lossy(&output.stdout))
}

/// Collect job status from `qstat` (SGE).
fn get_sge_jobs() -> Vec<JobInfo> {
    let output = match Command::new("qstat").output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    parse_sge_qstat(&String::from_utf8_lossy(&output.stdout))
}

/// Collect node status from `qhost` (SGE).
fn get_sge_nodes() -> Vec<NodeStatus> {
    let output = match Command::new("qhost").output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    parse_sge_qhost(&String::from_utf8_lossy(&output.stdout))
}

/// Derive queue status from SGE job rows — `qstat -g c` column layouts vary
/// across versions, but the per-job queue/state columns are stable.
fn sge_queues_from_jobs(jobs: &[JobInfo]) -> Vec<QueueStatus> {
    let mut map: std::collections::BTreeMap<String, QueueStatus> =
        std::collections::BTreeMap::new();
    for job in jobs {
        let queue_name = job.queue.split('@').next().unwrap_or(&job.queue);
        if queue_name.is_empty() {
            continue;
        }
        let queue = map
            .entry(queue_name.to_string())
            .or_insert_with(|| QueueStatus {
                queue_name: queue_name.to_string(),
                state: "up".to_string(),
                total_jobs: 0,
                running: 0,
                pending: 0,
                held: 0,
            });
        queue.total_jobs += 1;
        match job.state.as_str() {
            "r" | "dr" | "t" => queue.running += 1,
            "qw" | "hqw" | "Eqw" => queue.pending += 1,
            _ => {}
        }
    }
    map.into_values().collect()
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
        SchedulerType::Lsf => {
            let version = Command::new("bjobs").arg("-V").output().ok().and_then(|o| {
                let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
                text.push_str(&String::from_utf8_lossy(&o.stderr));
                text.lines().next().map(|l| l.trim().to_string())
            });
            let queues = get_lsf_queues();
            let nodes = get_lsf_nodes();
            let jobs = get_lsf_jobs();
            let total_jobs = jobs.len();

            HpcStatus {
                scheduler: "LSF".to_string(),
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
        SchedulerType::Sge => {
            let version = Command::new("qstat")
                .arg("-help")
                .output()
                .ok()
                .and_then(|o| {
                    String::from_utf8_lossy(&o.stderr)
                        .lines()
                        .next()
                        .map(|l| l.trim().to_string())
                });
            let nodes = get_sge_nodes();
            let jobs = get_sge_jobs();
            let queues = sge_queues_from_jobs(&jobs);
            let total_jobs = jobs.len();

            HpcStatus {
                scheduler: "SGE".to_string(),
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
    fn command_succeeds_respects_exit_status() {
        // Arrange — /usr/bin/true and /usr/bin/false exist on every Unix
        // Act / Assert
        assert!(command_succeeds("true", &[]));
        assert!(!command_succeeds("false", &[]));
        assert!(!command_succeeds("no-such-binary-xyz", &[]));
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

    #[test]
    fn parses_lsf_bhosts_nodes_with_cpu_accounting() {
        // Arrange — real bhosts column layout (LSF 10.x)
        let out = "HOST_NAME          STATUS       JL/U    MAX  NJOBS    RUN  SSUSP  USUSP    RSV\n\
                   node01             ok              -     16      0      0      0      0      0\n\
                   node02             closed          -     16      4      4      0      0      0\n";

        // Act
        let nodes = parse_lsf_bhosts(out);

        // Assert
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].name, "node01");
        assert_eq!(nodes[0].state, "ok");
        assert_eq!(nodes[0].cpus_total, 16);
        assert_eq!(nodes[0].cpus_alloc, 0);
        assert_eq!(nodes[0].cpus_free, 16);
        assert_eq!(nodes[1].state, "closed");
        assert_eq!(nodes[1].cpus_alloc, 4);
        assert_eq!(nodes[1].cpus_free, 12);
    }

    #[test]
    fn parses_lsf_bjobs_job_rows() {
        // Arrange
        let out = "JOBID   USER    STAT  QUEUE      FROM_HOST   EXEC_HOST   JOB_NAME   SUBMIT_TIME\n\
                   12345   bioinf  RUN   normal     login01     node01      align      Aug 14 10:22\n";

        // Act
        let jobs = parse_lsf_bjobs(out);

        // Assert
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].job_id, "12345");
        assert_eq!(jobs[0].user, "bioinf");
        assert_eq!(jobs[0].state, "RUN");
        assert_eq!(jobs[0].queue, "normal");
        assert_eq!(jobs[0].nodes.as_deref(), Some("node01"));
        assert_eq!(jobs[0].name, "align");
    }

    #[test]
    fn parses_lsf_bqueues_queue_rows() {
        // Arrange
        let out = "QUEUE_NAME      PRIO STATUS          MAX JL/U JL/P JL/H NJOBS  PEND   RUN  SUSP\n\
                   normal            30  Open:Active       -    -    -    -     2     1     1     0\n\
                   long              20  Open:Active       -    -    -    -     0     0     0     0\n";

        // Act
        let queues = parse_lsf_bqueues(out);

        // Assert
        assert_eq!(queues.len(), 2);
        assert_eq!(queues[0].queue_name, "normal");
        assert_eq!(queues[0].state, "Open:Active");
        assert_eq!(queues[0].total_jobs, 2);
        assert_eq!(queues[0].pending, 1);
        assert_eq!(queues[0].running, 1);
    }

    #[test]
    fn parses_sge_qstat_jobs_with_and_without_queue_column() {
        // Arrange — running job has queue@node + slots; queued job has none
        let out = "job-ID  prior   name       user         state submit/start at     queue                          slots ja-task-ID\n\
                   --------------------------------------------------------------------------------------------------------------\n\
                        99 0.55500 fastqc      bioinf       r     08/14/2026 10:00:00 all.q@node01                     1\n\
                       100 0.55500 align       bioinf       qw    08/14/2026 10:05:00\n";

        // Act
        let jobs = parse_sge_qstat(out);

        // Assert
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].job_id, "99");
        assert_eq!(jobs[0].state, "r");
        assert_eq!(jobs[0].queue, "all.q@node01");
        assert_eq!(jobs[0].nodes.as_deref(), Some("node01"));
        assert_eq!(jobs[0].cpus, 1);
        assert_eq!(jobs[1].job_id, "100");
        assert_eq!(jobs[1].state, "qw");
        assert_eq!(jobs[1].queue, "");
    }

    #[test]
    fn parses_sge_qhost_nodes_and_memory_units() {
        // Arrange — older SGE ships fewer columns (no NSOC/NCOR)
        let out = "HOSTNAME                ARCH         NCPU NSOC NCOR NTHR  LOAD  MEMTOT  MEMUSE  SWAPTO  SWAPUS\n\
                   ------------------------------------------------------------------------------------------\n\
                   node01                  lx-amd64        16    2    8   16  0.01   62.7G    1.2G     0.0     0.0\n\
                   node02                  lx-amd64         8         4    0.50   2048M    512M     0.0     0.0\n";

        // Act
        let nodes = parse_sge_qhost(out);

        // Assert
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].name, "node01");
        assert_eq!(nodes[0].cpus_total, 16);
        assert_eq!(nodes[0].memory_total_mb, 64204); // 62.7G
        assert_eq!(nodes[0].memory_free_mb, 62976); // 62.7G - 1.2G used
        assert_eq!(nodes[1].cpus_total, 8);
        assert_eq!(nodes[1].memory_total_mb, 2048); // 2048M
        assert_eq!(nodes[1].memory_free_mb, 1536); // 2048M - 512M used
    }

    #[test]
    fn derives_sge_queues_from_job_rows_with_running_and_pending_counts() {
        // Arrange — jobs on all.q and long.q, one queued job without a host
        let jobs = vec![
            JobInfo {
                job_id: "99".into(),
                name: "fastqc".into(),
                user: "bioinf".into(),
                state: "r".into(),
                queue: "all.q@node01".into(),
                nodes: Some("node01".into()),
                cpus: 1,
                elapsed: None,
                time_limit: None,
            },
            JobInfo {
                job_id: "100".into(),
                name: "align".into(),
                user: "bioinf".into(),
                state: "qw".into(),
                queue: "all.q".into(),
                nodes: None,
                cpus: 0,
                elapsed: None,
                time_limit: None,
            },
            JobInfo {
                job_id: "101".into(),
                name: "rnaseq".into(),
                user: "other".into(),
                state: "r".into(),
                queue: "long.q@node02".into(),
                nodes: Some("node02".into()),
                cpus: 8,
                elapsed: None,
                time_limit: None,
            },
        ];

        // Act
        let queues = sge_queues_from_jobs(&jobs);

        // Assert
        assert_eq!(queues.len(), 2);
        assert_eq!(queues[0].queue_name, "all.q");
        assert_eq!(queues[0].total_jobs, 2);
        assert_eq!(queues[0].running, 1);
        assert_eq!(queues[0].pending, 1);
        assert_eq!(queues[1].queue_name, "long.q");
        assert_eq!(queues[1].running, 1);
    }
}
