//! HPC scheduler integration — detect available schedulers and submit jobs.
//! Currently supports Slurm; PBS/LSF/SGE detection only.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
            Self::Slurm => write!(f, "slurm"),
            Self::Pbs => write!(f, "pbs"),
            Self::Lsf => write!(f, "lsf"),
            Self::Sge => write!(f, "sge"),
            Self::None => write!(f, "none"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HpcStatus {
    pub scheduler: String,
    pub scheduler_type: SchedulerType,
    pub available: bool,
    pub version: Option<String>,
    pub error: Option<String>,
}

/// Detect available HPC scheduler on the system.
pub fn detect_scheduler() -> SchedulerType {
    if std::process::Command::new("sinfo")
        .arg("--version")
        .output()
        .is_ok()
    {
        return SchedulerType::Slurm;
    }
    if std::process::Command::new("qstat")
        .arg("--version")
        .output()
        .is_ok()
        || std::process::Command::new("pbsnodes")
            .arg("--version")
            .output()
            .is_ok()
    {
        return SchedulerType::Pbs;
    }
    if std::process::Command::new("bjobs")
        .arg("-V")
        .output()
        .is_ok()
    {
        return SchedulerType::Lsf;
    }
    if std::process::Command::new("qstat")
        .arg("-help")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stderr).contains("GE"))
    {
        return SchedulerType::Sge;
    }
    SchedulerType::None
}

/// Get HPC status for the detected scheduler.
pub fn get_hpc_status() -> HpcStatus {
    let scheduler = detect_scheduler();
    match scheduler {
        SchedulerType::Slurm => {
            let version = std::process::Command::new("sinfo")
                .arg("--version")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
            HpcStatus {
                scheduler: "slurm".into(),
                scheduler_type: SchedulerType::Slurm,
                available: true,
                version,
                error: None,
            }
        }
        SchedulerType::None => HpcStatus {
            scheduler: "none".into(),
            scheduler_type: SchedulerType::None,
            available: false,
            version: None,
            error: Some("No HPC scheduler detected. Install Slurm, PBS, LSF, or SGE.".into()),
        },
        other => HpcStatus {
            scheduler: other.to_string(),
            scheduler_type: other,
            available: true,
            version: None,
            error: Some(
                "Scheduler detected but full support not yet implemented for this backend.".into(),
            ),
        },
    }
}

/// Generate a Slurm submission script for a pipeline run.
pub fn generate_slurm_script(
    job_name: &str,
    workdir: &str,
    cpus: u32,
    memory_gb: u32,
    walltime: &str,
    command: &str,
) -> String {
    format!(
        "#!/bin/bash
#SBATCH --job-name={job_name}
#SBATCH --output={workdir}/slurm-%j.out
#SBATCH --error={workdir}/slurm-%j.err
#SBATCH --cpus-per-task={cpus}
#SBATCH --mem={memory_gb}G
#SBATCH --time={walltime}

{command}
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_scheduler_returns_type() {
        let s = detect_scheduler();
        // Just verify it doesn't panic and returns a valid variant
        let _ = s.to_string();
    }

    #[test]
    fn test_generate_slurm_script() {
        let script = generate_slurm_script(
            "test_job",
            "/tmp/work",
            8,
            32,
            "24:00:00",
            "oxo-flow run test.oxoflow",
        );
        assert!(script.contains("--job-name=test_job"));
        assert!(script.contains("--cpus-per-task=8"));
        assert!(script.contains("--mem=32G"));
        assert!(script.contains("oxo-flow run test.oxoflow"));
    }

    #[test]
    fn test_hpc_status_none() {
        // This test works even without a scheduler installed
        let status = get_hpc_status();
        assert!(!status.scheduler.is_empty());
    }
}
