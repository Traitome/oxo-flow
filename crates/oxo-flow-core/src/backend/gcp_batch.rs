//! Google Cloud Batch job specification generator.
//!
//! Maps ScheduledRule specifications onto Google Cloud Batch jobs.submit
//! JSON configurations with cpuMilli/memoryMib, taskGroups, and allocation
//! policies. Submission/polling goes through the shared `ClusterExecutor`
//! (`backend/cluster.rs`).
//!
//! GPU resources (`gpu` / `gpu_spec`) are deliberately not mapped here:
//! GCP Batch attaches GPUs through the job's allocation policy
//! (InstanceTemplate or `installGpus`), not the task payload's
//! `computeResource` — GPU provisioning stays operator infrastructure.

use super::{sanitize_cloud_name, scheduled_resources};
use crate::cluster::ClusterJobConfig;
use crate::error::Result;
use crate::rule::Rule;
use serde_json::json;

/// Sanitise a string for use as a GCP Batch job name (lowercase alphanumeric
/// and hyphens, max 63 chars).
pub fn sanitize_gcp_batch_name(name: &str) -> String {
    sanitize_cloud_name(name, 63, true, "")
}

/// Generate a Google Cloud Batch job JSON configuration.
pub fn generate_gcp_batch_job_spec(
    rule: &Rule,
    shell_cmd: &str,
    // Kept for dispatch uniformity; GCP Batch specs currently derive
    // everything from the rule itself (GPU stays in operator infrastructure,
    // see the module docs).
    _cluster_config: &ClusterJobConfig,
) -> Result<String> {
    let (threads, mem_mb) = scheduled_resources(rule);

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

    Ok(serde_json::to_string_pretty(&payload)?)
}

/// Generate a Google Cloud Batch array job JSON configuration.
pub fn generate_gcp_batch_array_spec(
    rule: &Rule,
    cmd_dir: &str,
    count: usize,
    _cluster_config: &ClusterJobConfig,
) -> Result<String> {
    let (threads, mem_mb) = scheduled_resources(rule);

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

    Ok(serde_json::to_string_pretty(&payload)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::ClusterBackend;
    use crate::rule::Rule;
    use serde_json::Value;

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
            generate_gcp_batch_job_spec(&rule, "gatk HaplotypeCaller -I in.bam", &test_config())
                .expect("spec generation must succeed");
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

        let spec_str = generate_gcp_batch_array_spec(&rule, "runs/cmds", 6, &test_config())
            .expect("spec generation must succeed");
        let val: Value = serde_json::from_str(&spec_str).expect("spec must be valid json");

        let group = &val["taskGroups"][0];
        assert_eq!(group["taskCount"], 6);
        let text = group["taskSpec"]["runnables"][0]["script"]["text"]
            .as_str()
            .unwrap();
        assert!(text.contains("BATCH_TASK_INDEX"));
    }
}
