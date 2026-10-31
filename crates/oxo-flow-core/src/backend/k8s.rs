//! Native Kubernetes batch Jobs executor and manifest generator.
//!
//! Maps ScheduledRule resources and container specifications onto Kubernetes
//! batch/v1 Job manifests with deterministic labels and fail-fast semantics.

use super::{BackendJobStatus, ExecutorBackend, ScheduledRule, TerminalRecord};
use crate::cluster::ClusterJobConfig;
use crate::error::{OxoFlowError, Result};
use crate::rule::Rule;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Sanitise a string for use as a Kubernetes resource name or label value.
/// RFC 1123 label: lowercase alphanumeric characters or minus (-), max 63 chars.
pub fn sanitize_k8s_name(name: &str) -> String {
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

/// Generate a Kubernetes batch/v1 Job manifest for a single rule.
pub fn generate_k8s_job_manifest(
    rule: &Rule,
    shell_cmd: &str,
    cluster_config: &ClusterJobConfig,
) -> String {
    let job_name = format!("oxoflow-{}", sanitize_k8s_name(&rule.name));
    let threads = rule.resources.threads.max(1);
    let mem_mb = crate::scheduler::effective_memory_mb(rule, 1024).max(256);

    let container_image = rule
        .environment
        .docker
        .as_deref()
        .unwrap_or("ubuntu:latest");

    let mut limits = json!({
        "cpu": format!("{threads}"),
        "memory": format!("{mem_mb}Mi"),
    });

    if let Some(gpu_count) = rule.resources.gpu {
        if gpu_count > 0 {
            limits["nvidia.com/gpu"] = json!(format!("{gpu_count}"));
        }
    } else if let Some(spec) = &rule.resources.gpu_spec
        && spec.count > 0
    {
        limits["nvidia.com/gpu"] = json!(format!("{}", spec.count));
    }

    let manifest = json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": {
            "name": job_name,
            "labels": {
                "oxoflow.io/managed-by": "oxo-flow",
                "oxoflow.io/rule": sanitize_k8s_name(&rule.name),
            }
        },
        "spec": {
            "backoffLimit": 0,
            "template": {
                "metadata": {
                    "labels": {
                        "oxoflow.io/managed-by": "oxo-flow",
                        "oxoflow.io/rule": sanitize_k8s_name(&rule.name),
                    }
                },
                "spec": {
                    "restartPolicy": "Never",
                    "containers": [
                        {
                            "name": sanitize_k8s_name(&rule.name),
                            "image": container_image,
                            "command": ["/bin/bash", "-c"],
                            "args": [shell_cmd],
                            "resources": {
                                "requests": {
                                    "cpu": format!("{threads}"),
                                    "memory": format!("{mem_mb}Mi"),
                                },
                                "limits": limits,
                            }
                        }
                    ]
                }
            }
        }
    });

    let _ = cluster_config;
    serde_json::to_string_pretty(&manifest).unwrap_or_default()
}

/// Generate a Kubernetes indexed Job manifest for an array submission.
pub fn generate_k8s_array_job_manifest(
    rule: &Rule,
    cmd_dir: &str,
    count: usize,
    cluster_config: &ClusterJobConfig,
) -> String {
    let job_name = format!("oxoflow-array-{}", sanitize_k8s_name(&rule.name));
    let threads = rule.resources.threads.max(1);
    let mem_mb = crate::scheduler::effective_memory_mb(rule, 1024).max(256);

    let container_image = rule
        .environment
        .docker
        .as_deref()
        .unwrap_or("ubuntu:latest");

    let dispatch_cmd =
        format!("INDEX=$((JOB_COMPLETION_INDEX + 1)); sh \"{cmd_dir}/cmd.${{INDEX}}.sh\"");

    let manifest = json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": {
            "name": job_name,
            "labels": {
                "oxoflow.io/managed-by": "oxo-flow",
                "oxoflow.io/rule": sanitize_k8s_name(&rule.name),
            }
        },
        "spec": {
            "completions": count,
            "parallelism": count.min(64),
            "completionMode": "Indexed",
            "backoffLimit": 0,
            "template": {
                "metadata": {
                    "labels": {
                        "oxoflow.io/managed-by": "oxo-flow",
                        "oxoflow.io/rule": sanitize_k8s_name(&rule.name),
                    }
                },
                "spec": {
                    "restartPolicy": "Never",
                    "containers": [
                        {
                            "name": sanitize_k8s_name(&rule.name),
                            "image": container_image,
                            "command": ["/bin/bash", "-c"],
                            "args": [dispatch_cmd],
                            "resources": {
                                "requests": {
                                    "cpu": format!("{threads}"),
                                    "memory": format!("{mem_mb}Mi"),
                                },
                                "limits": {
                                    "cpu": format!("{threads}"),
                                    "memory": format!("{mem_mb}Mi"),
                                }
                            }
                        }
                    ]
                }
            }
        }
    });

    let _ = cluster_config;
    serde_json::to_string_pretty(&manifest).unwrap_or_default()
}

/// Kubernetes batch Jobs executor driver.
pub struct KubernetesExecutor {
    cluster: ClusterJobConfig,
    namespace: Option<String>,
    bin_dir: Option<PathBuf>,
    env: Vec<(String, String)>,
    workdir: PathBuf,
}

impl KubernetesExecutor {
    pub fn new(cluster: ClusterJobConfig) -> Self {
        Self {
            cluster,
            namespace: None,
            bin_dir: None,
            env: Vec::new(),
            workdir: PathBuf::from("."),
        }
    }

    pub fn with_namespace(mut self, ns: &str) -> Self {
        self.namespace = Some(ns.to_string());
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
impl ExecutorBackend for KubernetesExecutor {
    fn name(&self) -> &'static str {
        "kubernetes"
    }

    fn render_script(&self, rule: &ScheduledRule) -> Result<String> {
        Ok(generate_k8s_job_manifest(
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
        Ok(generate_k8s_array_job_manifest(
            rule,
            cmd_dir,
            count,
            &self.cluster,
        ))
    }

    async fn submit(&self, script_path: &Path) -> Result<String> {
        let path_str = script_path.to_string_lossy().to_string();
        let mut args = vec!["create", "-f", &path_str, "-o", "jsonpath={.metadata.name}"];
        let ns_flag;
        if let Some(ns) = &self.namespace {
            ns_flag = format!("--namespace={ns}");
            args.push(&ns_flag);
        }
        let out = self.run_cmd("kubectl", &args).await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(OxoFlowError::Config {
                message: format!("kubectl create failed: {stderr}"),
            });
        }
        let job_id = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if job_id.is_empty() {
            return Err(OxoFlowError::Config {
                message: "kubectl create returned empty job name".into(),
            });
        }
        Ok(job_id)
    }

    async fn poll(&self, job_ids: &[String]) -> Result<HashMap<String, BackendJobStatus>> {
        let mut statuses = HashMap::new();
        if job_ids.is_empty() {
            return Ok(statuses);
        }

        for id in job_ids {
            let mut args = vec!["get", "job", id, "-o", "json"];
            let ns_flag;
            if let Some(ns) = &self.namespace {
                ns_flag = format!("--namespace={ns}");
                args.push(&ns_flag);
            }
            let out = match self.run_cmd("kubectl", &args).await {
                Ok(o) => o,
                Err(_) => continue,
            };
            if !out.status.success() {
                continue;
            }
            let stdout = String::from_utf8_lossy(&out.stdout);
            if let Ok(v) = serde_json::from_str::<Value>(&stdout) {
                let status_obj = &v["status"];
                let active = status_obj["active"].as_u64().unwrap_or(0);
                let succeeded = status_obj["succeeded"].as_u64().unwrap_or(0);
                let failed = status_obj["failed"].as_u64().unwrap_or(0);

                let status = if succeeded > 0 {
                    BackendJobStatus::Completed
                } else if failed > 0 {
                    BackendJobStatus::Failed
                } else if active > 0 {
                    BackendJobStatus::Running
                } else if status_obj["startTime"].is_null() {
                    BackendJobStatus::Pending
                } else {
                    BackendJobStatus::Running
                };
                statuses.insert(id.clone(), status);
            }
        }

        Ok(statuses)
    }

    async fn cancel(&self, job_id: &str) -> Result<()> {
        let mut args = vec!["delete", "job", job_id, "--cascade=foreground"];
        let ns_flag;
        if let Some(ns) = &self.namespace {
            ns_flag = format!("--namespace={ns}");
            args.push(&ns_flag);
        }
        let out = self.run_cmd("kubectl", &args).await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(OxoFlowError::Config {
                message: format!("kubectl delete failed: {stderr}"),
            });
        }
        Ok(())
    }

    async fn logs(&self, job_id: &str) -> Result<String> {
        let job_ref = format!("job/{job_id}");
        let mut args = vec!["logs", &job_ref];
        let ns_flag;
        if let Some(ns) = &self.namespace {
            ns_flag = format!("--namespace={ns}");
            args.push(&ns_flag);
        }
        let out = self.run_cmd("kubectl", &args).await?;
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
            backend: ClusterBackend::Kubernetes,
            queue: None,
            account: None,
            walltime: None,
            extra_args: Vec::new(),
        }
    }

    #[test]
    fn test_sanitize_k8s_name() {
        assert_eq!(sanitize_k8s_name("align_reads.1"), "align-reads-1");
        assert_eq!(sanitize_k8s_name("STAR-genome_index"), "star-genome-index");
        assert_eq!(sanitize_k8s_name("---test---"), "test");
        assert_eq!(sanitize_k8s_name(""), "job");
    }

    #[test]
    fn test_generate_k8s_job_manifest() {
        let resources = crate::rule::Resources {
            threads: 8,
            memory: Some("16G".into()),
            gpu: Some(1),
            ..Default::default()
        };
        let rule = Rule {
            name: "align_sample".into(),
            resources,
            ..Default::default()
        };

        let manifest_str =
            generate_k8s_job_manifest(&rule, "bwa mem ref.fa reads.fq", &test_config());
        let val: Value = serde_json::from_str(&manifest_str).expect("manifest must be valid json");

        assert_eq!(val["apiVersion"], "batch/v1");
        assert_eq!(val["kind"], "Job");
        assert_eq!(val["metadata"]["name"], "oxoflow-align-sample");
        assert_eq!(val["spec"]["backoffLimit"], 0);

        let container = &val["spec"]["template"]["spec"]["containers"][0];
        assert_eq!(container["resources"]["limits"]["cpu"], "8");
        assert_eq!(container["resources"]["limits"]["nvidia.com/gpu"], "1");
        assert_eq!(container["command"][0], "/bin/bash");
        assert_eq!(container["args"][0], "bwa mem ref.fa reads.fq");
    }

    #[test]
    fn test_generate_k8s_array_job_manifest() {
        let resources = crate::rule::Resources {
            threads: 4,
            memory: Some("8G".into()),
            ..Default::default()
        };
        let rule = Rule {
            name: "scatter_qc".into(),
            resources,
            ..Default::default()
        };

        let manifest_str = generate_k8s_array_job_manifest(&rule, "run/cmds", 5, &test_config());
        let val: Value = serde_json::from_str(&manifest_str).expect("manifest must be valid json");

        assert_eq!(val["spec"]["completions"], 5);
        assert_eq!(val["spec"]["completionMode"], "Indexed");
        assert_eq!(val["spec"]["backoffLimit"], 0);

        let container = &val["spec"]["template"]["spec"]["containers"][0];
        assert!(
            container["args"][0]
                .as_str()
                .unwrap()
                .contains("JOB_COMPLETION_INDEX")
        );
    }
}
