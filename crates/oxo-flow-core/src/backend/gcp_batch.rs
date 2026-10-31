//! Google Cloud Batch executor and job specification generator.
//!
//! Maps ScheduledRule specifications onto Google Cloud Batch jobs submit
//! JSON configurations with cpuMilli/memoryMib, taskGroups, and allocation policies.

use super::{BackendJobStatus, ExecutorBackend, ScheduledRule, TerminalRecord};
use crate::cluster::ClusterJobConfig;
use crate::error::{OxoFlowError, Result};
use crate::rule::Rule;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Sanitise a string for use as a GCP Batch job name (lowercase alphanumeric or hyphens, max 63 chars).
pub fn sanitize_gcp_batch_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if c == '-' || c == '_' || c == '.' {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "job".to_string()
    } else if trimmed.len() > 63 {
        trimmed[..63].trim_matches('-').to_string()
    } else {
        trimmed.to_string()
    }
}

/// Generate a Google Cloud Batch job JSON configuration.
pub fn generate_gcp_batch_job_spec(
    rule: &Rule,
    shell_cmd: &str,
    cluster_config: &ClusterJobConfig,
) -> String {
    let threads = rule.resources.threads.max(1);
    let mem_mb = crate::scheduler::effective_memory_mb(rule, 1024).max(256);

    let mut runnable = json!({
        "script": {
            "text": shell_cmd
        }
    });

    if let Some(docker_image) = &rule.environment.docker {
        runnable = json!({
            "container": {
                "imageUri": docker_image,
                "commands": ["/bin/bash", "-c", shell_cmd]
            }
        });
    }

    let payload = json!({
        "taskGroups": [
            {
                "taskSpec": {
                    "runnables": [runnable],
                    "computeResource": {
                        "cpuMilli": threads * 1000,
                        "memoryMib": mem_mb
                    },
                    "maxRetryCount": 0
                }
            }
        ],
        "logsPolicy": {
            "destination": "CLOUD_LOGGING"
        }
    });

    let _ = cluster_config;
    serde_json::to_string_pretty(&payload).unwrap_or_default()
}

/// Generate a Google Cloud Batch array job JSON configuration.
pub fn generate_gcp_batch_array_spec(
    rule: &Rule,
    cmd_dir: &str,
    count: usize,
    cluster_config: &ClusterJobConfig,
) -> String {
    let threads = rule.resources.threads.max(1);
    let mem_mb = crate::scheduler::effective_memory_mb(rule, 1024).max(256);

    let dispatch_cmd =
        format!("INDEX=$((BATCH_TASK_INDEX + 1)); sh \"{cmd_dir}/cmd.${{INDEX}}.sh\"");

    let mut runnable = json!({
        "script": {
            "text": dispatch_cmd
        }
    });

    if let Some(docker_image) = &rule.environment.docker {
        runnable = json!({
            "container": {
                "imageUri": docker_image,
                "commands": ["/bin/bash", "-c", dispatch_cmd]
            }
        });
    }

    let payload = json!({
        "taskGroups": [
            {
                "taskCount": count,
                "parallelism": count.min(64),
                "taskSpec": {
                    "runnables": [runnable],
                    "computeResource": {
                        "cpuMilli": threads * 1000,
                        "memoryMib": mem_mb
                    },
                    "maxRetryCount": 0
                }
            }
        ],
        "logsPolicy": {
            "destination": "CLOUD_LOGGING"
        }
    });

    let _ = cluster_config;
    serde_json::to_string_pretty(&payload).unwrap_or_default()
}

/// Google Cloud Batch executor driver.
pub struct GcpBatchExecutor {
    cluster: ClusterJobConfig,
    project: Option<String>,
    location: String,
    bin_dir: Option<PathBuf>,
    env: Vec<(String, String)>,
    workdir: PathBuf,
}

impl GcpBatchExecutor {
    pub fn new(cluster: ClusterJobConfig) -> Self {
        Self {
            cluster,
            project: None,
            location: "us-central1".to_string(),
            bin_dir: None,
            env: Vec::new(),
            workdir: PathBuf::from("."),
        }
    }

    pub fn with_project(mut self, project: &str) -> Self {
        self.project = Some(project.to_string());
        self
    }

    pub fn with_location(mut self, loc: &str) -> Self {
        self.location = loc.to_string();
        self
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
impl ExecutorBackend for GcpBatchExecutor {
    fn name(&self) -> &'static str {
        "gcp-batch"
    }

    fn render_script(&self, rule: &ScheduledRule) -> Result<String> {
        Ok(generate_gcp_batch_job_spec(
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
        Ok(generate_gcp_batch_array_spec(
            rule,
            cmd_dir,
            count,
            &self.cluster,
        ))
    }

    async fn submit(&self, script_path: &Path) -> Result<String> {
        let path_str = script_path.to_string_lossy().to_string();
        let stem = script_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("job");
        let job_name = format!("oxoflow-{}", sanitize_gcp_batch_name(stem));

        let config_arg = format!("--config={path_str}");
        let loc_arg = format!("--location={}", self.location);
        let mut args = vec![
            "batch",
            "jobs",
            "submit",
            &job_name,
            &config_arg,
            &loc_arg,
            "--format=value(name)",
        ];

        let proj_arg;
        if let Some(proj) = &self.project {
            proj_arg = format!("--project={proj}");
            args.push(&proj_arg);
        }

        let out = self.run_cmd("gcloud", &args).await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(OxoFlowError::Config {
                message: format!("gcloud batch jobs submit failed: {stderr}"),
            });
        }
        let job_id = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let final_id = if job_id.is_empty() { job_name } else { job_id };
        Ok(final_id)
    }

    async fn poll(&self, job_ids: &[String]) -> Result<HashMap<String, BackendJobStatus>> {
        let mut statuses = HashMap::new();
        if job_ids.is_empty() {
            return Ok(statuses);
        }

        for id in job_ids {
            let loc_arg = format!("--location={}", self.location);
            let mut args = vec![
                "batch",
                "jobs",
                "describe",
                id.as_str(),
                &loc_arg,
                "--format=json",
            ];
            let proj_arg;
            if let Some(proj) = &self.project {
                proj_arg = format!("--project={proj}");
                args.push(&proj_arg);
            }

            let out = match self.run_cmd("gcloud", &args).await {
                Ok(o) => o,
                Err(_) => continue,
            };
            if !out.status.success() {
                continue;
            }

            let stdout = String::from_utf8_lossy(&out.stdout);
            if let Ok(val) = serde_json::from_str::<Value>(&stdout) {
                let st = val["status"]["state"].as_str().unwrap_or_default();
                let status = match st {
                    "QUEUED" | "SCHEDULED" => BackendJobStatus::Pending,
                    "RUNNING" => BackendJobStatus::Running,
                    "SUCCEEDED" => BackendJobStatus::Completed,
                    "FAILED" | "DELETION_IN_PROGRESS" => BackendJobStatus::Failed,
                    _ => BackendJobStatus::Unknown,
                };
                statuses.insert(id.clone(), status);
            }
        }

        Ok(statuses)
    }

    async fn cancel(&self, job_id: &str) -> Result<()> {
        let loc_arg = format!("--location={}", self.location);
        let mut args = vec!["batch", "jobs", "delete", job_id, &loc_arg, "--quiet"];
        let proj_arg;
        if let Some(proj) = &self.project {
            proj_arg = format!("--project={proj}");
            args.push(&proj_arg);
        }
        let out = self.run_cmd("gcloud", &args).await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(OxoFlowError::Config {
                message: format!("gcloud batch jobs delete failed: {stderr}"),
            });
        }
        Ok(())
    }

    async fn logs(&self, job_id: &str) -> Result<String> {
        let filter =
            format!("resource.type=batch.googleapis.com/Job AND resource.labels.job_id={job_id}");
        let args = vec![
            "logging",
            "read",
            &filter,
            "--limit=50",
            "--format=value(textPayload)",
        ];
        let out = self.run_cmd("gcloud", &args).await?;
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    async fn terminal_status(&self, job_id: &str) -> Option<TerminalRecord> {
        let poll_map = self.poll(&[job_id.to_string()]).await.ok()?;
        let status = poll_map.get(job_id).copied()?;
        Some(TerminalRecord::status_only(status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::ClusterBackend;
    use crate::rule::Rule;

    fn test_config() -> ClusterJobConfig {
        ClusterJobConfig {
            backend: ClusterBackend::GcpBatch,
            queue: None,
            account: None,
            walltime: None,
            extra_args: Vec::new(),
        }
    }

    #[test]
    fn test_sanitize_gcp_batch_name() {
        assert_eq!(
            sanitize_gcp_batch_name("Call_Variants.v1"),
            "call-variants-v1"
        );
        assert_eq!(sanitize_gcp_batch_name("STAR_Align"), "star-align");
    }

    #[test]
    fn test_generate_gcp_batch_job_spec() {
        let resources = crate::rule::Resources {
            threads: 8,
            memory: Some("16G".into()),
            ..Default::default()
        };
        let rule = Rule {
            name: "call_variants".into(),
            resources,
            ..Default::default()
        };

        let spec_str =
            generate_gcp_batch_job_spec(&rule, "gatk HaplotypeCaller -I in.bam", &test_config());
        let val: Value = serde_json::from_str(&spec_str).expect("spec must be valid json");

        let group = &val["taskGroups"][0];
        let compute = &group["taskSpec"]["computeResource"];
        assert_eq!(compute["cpuMilli"], 8000);
        assert_eq!(compute["memoryMib"], 16384);

        let runnable = &group["taskSpec"]["runnables"][0];
        assert_eq!(runnable["script"]["text"], "gatk HaplotypeCaller -I in.bam");
    }

    #[test]
    fn test_generate_gcp_batch_array_spec() {
        let resources = crate::rule::Resources {
            threads: 2,
            memory: Some("4G".into()),
            ..Default::default()
        };
        let rule = Rule {
            name: "trim_galore".into(),
            resources,
            ..Default::default()
        };

        let spec_str = generate_gcp_batch_array_spec(&rule, "runs/cmds", 6, &test_config());
        let val: Value = serde_json::from_str(&spec_str).expect("spec must be valid json");

        let group = &val["taskGroups"][0];
        assert_eq!(group["taskCount"], 6);
        let text = group["taskSpec"]["runnables"][0]["script"]["text"]
            .as_str()
            .unwrap();
        assert!(text.contains("BATCH_TASK_INDEX"));
    }
}
