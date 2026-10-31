//! AWS Batch managed compute executor and job specification generator.
//!
//! Maps ScheduledRule specifications onto AWS Batch SubmitJob payloads
//! with containerOverrides, VCPU/memory resource limits, and array properties.

use super::{BackendJobStatus, ExecutorBackend, ScheduledRule, TerminalRecord};
use crate::cluster::ClusterJobConfig;
use crate::error::{OxoFlowError, Result};
use crate::rule::Rule;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Sanitise a string for use as an AWS Batch job name (letters, numbers, underscores, hyphens, max 128 chars).
pub fn sanitize_aws_batch_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "job".to_string()
    } else if trimmed.len() > 128 {
        trimmed[..128].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Generate an AWS Batch SubmitJob input JSON specification.
pub fn generate_aws_batch_job_spec(
    rule: &Rule,
    shell_cmd: &str,
    cluster_config: &ClusterJobConfig,
) -> String {
    let job_name = format!("oxoflow-{}", sanitize_aws_batch_name(&rule.name));
    let threads = rule.resources.threads.max(1);
    let mem_mb = crate::scheduler::effective_memory_mb(rule, 1024).max(256);

    let job_queue = cluster_config.queue.as_deref().unwrap_or("oxoflow-queue");

    let job_definition = cluster_config
        .account
        .as_deref()
        .unwrap_or("oxoflow-job-def");

    let mut resource_requirements = vec![
        json!({
            "type": "VCPU",
            "value": format!("{threads}")
        }),
        json!({
            "type": "MEMORY",
            "value": format!("{mem_mb}")
        }),
    ];

    if let Some(gpu_count) = rule.resources.gpu {
        if gpu_count > 0 {
            resource_requirements.push(json!({
                "type": "GPU",
                "value": format!("{gpu_count}")
            }));
        }
    } else if let Some(spec) = &rule.resources.gpu_spec
        && spec.count > 0
    {
        resource_requirements.push(json!({
            "type": "GPU",
            "value": format!("{}", spec.count)
        }));
    }

    let payload = json!({
        "jobName": job_name,
        "jobQueue": job_queue,
        "jobDefinition": job_definition,
        "containerOverrides": {
            "command": ["/bin/bash", "-c", shell_cmd],
            "resourceRequirements": resource_requirements
        }
    });

    serde_json::to_string_pretty(&payload).unwrap_or_default()
}

/// Generate an AWS Batch array job specification.
pub fn generate_aws_batch_array_spec(
    rule: &Rule,
    cmd_dir: &str,
    count: usize,
    cluster_config: &ClusterJobConfig,
) -> String {
    let job_name = format!("oxoflow-array-{}", sanitize_aws_batch_name(&rule.name));
    let threads = rule.resources.threads.max(1);
    let mem_mb = crate::scheduler::effective_memory_mb(rule, 1024).max(256);

    let job_queue = cluster_config.queue.as_deref().unwrap_or("oxoflow-queue");

    let job_definition = cluster_config
        .account
        .as_deref()
        .unwrap_or("oxoflow-job-def");

    let dispatch_cmd =
        format!("INDEX=$((AWS_BATCH_JOB_ARRAY_INDEX + 1)); sh \"{cmd_dir}/cmd.${{INDEX}}.sh\"");

    let payload = json!({
        "jobName": job_name,
        "jobQueue": job_queue,
        "jobDefinition": job_definition,
        "arrayProperties": {
            "size": count
        },
        "containerOverrides": {
            "command": ["/bin/bash", "-c", dispatch_cmd],
            "resourceRequirements": [
                {
                    "type": "VCPU",
                    "value": format!("{threads}")
                },
                {
                    "type": "MEMORY",
                    "value": format!("{mem_mb}")
                }
            ]
        }
    });

    serde_json::to_string_pretty(&payload).unwrap_or_default()
}

/// AWS Batch executor driver.
pub struct AwsBatchExecutor {
    cluster: ClusterJobConfig,
    bin_dir: Option<PathBuf>,
    env: Vec<(String, String)>,
    workdir: PathBuf,
}

impl AwsBatchExecutor {
    pub fn new(cluster: ClusterJobConfig) -> Self {
        Self {
            cluster,
            bin_dir: None,
            env: Vec::new(),
            workdir: PathBuf::from("."),
        }
    }

    pub fn with_scheduler_dir(mut self, dir: PathBuf) -> Self {
        self.bin_dir = Some(dir);
        self
    }

    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    pub fn with_workdir(mut self, dir: PathBuf) -> Self {
        self.workdir = dir;
        self
    }

    async fn run_cmd(&self, program: &str, args: &[&str]) -> Result<std::process::Output> {
        let resolved = match &self.bin_dir {
            Some(dir) => dir.join(program),
            None => PathBuf::from(program),
        };
        let out = tokio::process::Command::new(&resolved)
            .args(args)
            .envs(self.env.iter().cloned())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| OxoFlowError::Config {
                message: format!("failed to run '{}': {e}", resolved.display()),
            })?;
        Ok(out)
    }
}

#[async_trait::async_trait]
impl ExecutorBackend for AwsBatchExecutor {
    fn name(&self) -> &'static str {
        "aws-batch"
    }

    fn render_script(&self, rule: &ScheduledRule) -> Result<String> {
        Ok(generate_aws_batch_job_spec(
            &rule.rule,
            &rule.shell_cmd,
            &self.cluster,
        ))
    }

    fn render_array_script(
        &self,
        rule: &crate::rule::Rule,
        cmd_dir: &str,
        count: usize,
    ) -> Result<String> {
        Ok(generate_aws_batch_array_spec(
            rule,
            cmd_dir,
            count,
            &self.cluster,
        ))
    }

    async fn submit(&self, script_path: &Path) -> Result<String> {
        let path_str = script_path.to_string_lossy().to_string();
        let file_arg = format!("file://{path_str}");
        let args = vec![
            "batch",
            "submit-job",
            "--cli-input-json",
            &file_arg,
            "--query",
            "jobId",
            "--output",
            "text",
        ];

        let out = self.run_cmd("aws", &args).await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(OxoFlowError::Config {
                message: format!("aws batch submit-job failed: {stderr}"),
            });
        }
        let job_id = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if job_id.is_empty() {
            return Err(OxoFlowError::Config {
                message: "aws batch submit-job returned empty jobId".into(),
            });
        }
        Ok(job_id)
    }

    async fn poll(&self, job_ids: &[String]) -> Result<HashMap<String, BackendJobStatus>> {
        let mut statuses = HashMap::new();
        if job_ids.is_empty() {
            return Ok(statuses);
        }

        let mut args = vec!["batch", "describe-jobs", "--jobs"];
        for id in job_ids {
            args.push(id.as_str());
        }

        let out = match self.run_cmd("aws", &args).await {
            Ok(o) => o,
            Err(_) => return Ok(statuses),
        };
        if !out.status.success() {
            return Ok(statuses);
        }

        let stdout = String::from_utf8_lossy(&out.stdout);
        if let Ok(val) = serde_json::from_str::<Value>(&stdout)
            && let Some(jobs) = val["jobs"].as_array()
        {
            for job in jobs {
                let id = job["jobId"].as_str().unwrap_or_default().to_string();
                let st = job["status"].as_str().unwrap_or_default();
                let status = match st {
                    "SUBMITTED" | "PENDING" | "RUNNABLE" | "STARTING" => BackendJobStatus::Pending,
                    "RUNNING" => BackendJobStatus::Running,
                    "SUCCEEDED" => BackendJobStatus::Completed,
                    "FAILED" => BackendJobStatus::Failed,
                    _ => BackendJobStatus::Unknown,
                };
                statuses.insert(id, status);
            }
        }

        Ok(statuses)
    }

    async fn cancel(&self, job_id: &str) -> Result<()> {
        let args = vec![
            "batch",
            "cancel-job",
            "--job-id",
            job_id,
            "--reason",
            "Cancelled by oxo-flow workflow engine",
        ];
        let out = self.run_cmd("aws", &args).await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(OxoFlowError::Config {
                message: format!("aws batch cancel-job failed: {stderr}"),
            });
        }
        Ok(())
    }

    async fn logs(&self, job_id: &str) -> Result<String> {
        let args = vec![
            "batch",
            "describe-jobs",
            "--jobs",
            job_id,
            "--query",
            "jobs[0].container.logStreamName",
            "--output",
            "text",
        ];
        let out = self.run_cmd("aws", &args).await?;
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    async fn terminal_status(&self, job_id: &str) -> Option<TerminalRecord> {
        let args = vec!["batch", "describe-jobs", "--jobs", job_id];
        let out = self.run_cmd("aws", &args).await.ok()?;
        if !out.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let val: Value = serde_json::from_str(&stdout).ok()?;
        let job = val["jobs"].as_array()?.first()?;
        let st = job["status"].as_str()?;

        let status = match st {
            "SUCCEEDED" => BackendJobStatus::Completed,
            "FAILED" => BackendJobStatus::Failed,
            _ => return None,
        };

        let exit_code = job["container"]["exitCode"].as_i64().map(|c| c as i32);
        Some(TerminalRecord {
            status,
            exit_code,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::ClusterBackend;
    use crate::rule::Rule;

    fn test_config() -> ClusterJobConfig {
        ClusterJobConfig {
            backend: ClusterBackend::AwsBatch,
            queue: Some("biotech-compute-queue".into()),
            account: Some("arn:aws:batch:job-def:1".into()),
            walltime: None,
            extra_args: Vec::new(),
        }
    }

    #[test]
    fn test_sanitize_aws_batch_name() {
        assert_eq!(sanitize_aws_batch_name("fastqc.sample1"), "fastqc-sample1");
        assert_eq!(sanitize_aws_batch_name("align_bwa-mem"), "align_bwa-mem");
    }

    #[test]
    fn test_generate_aws_batch_job_spec() {
        let resources = crate::rule::Resources {
            threads: 16,
            memory: Some("64G".into()),
            ..Default::default()
        };
        let rule = Rule {
            name: "star_align".into(),
            resources,
            ..Default::default()
        };

        let spec_str = generate_aws_batch_job_spec(&rule, "STAR --genomeDir ref", &test_config());
        let val: Value = serde_json::from_str(&spec_str).expect("spec must be valid json");

        assert_eq!(val["jobName"], "oxoflow-star_align");
        assert_eq!(val["jobQueue"], "biotech-compute-queue");
        assert_eq!(val["jobDefinition"], "arn:aws:batch:job-def:1");

        let container = &val["containerOverrides"];
        assert_eq!(container["command"][2], "STAR --genomeDir ref");
        assert_eq!(container["resourceRequirements"][0]["type"], "VCPU");
        assert_eq!(container["resourceRequirements"][0]["value"], "16");
    }

    #[test]
    fn test_generate_aws_batch_array_spec() {
        let resources = crate::rule::Resources {
            threads: 8,
            memory: Some("32G".into()),
            ..Default::default()
        };
        let rule = Rule {
            name: "mutect2".into(),
            resources,
            ..Default::default()
        };

        let spec_str = generate_aws_batch_array_spec(&rule, "runs/cmds", 10, &test_config());
        let val: Value = serde_json::from_str(&spec_str).expect("spec must be valid json");

        assert_eq!(val["arrayProperties"]["size"], 10);
        let cmd = val["containerOverrides"]["command"][2].as_str().unwrap();
        assert!(cmd.contains("AWS_BATCH_JOB_ARRAY_INDEX"));
    }
}
