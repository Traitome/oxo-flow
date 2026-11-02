//! AWS Batch job specification generator.
//!
//! Maps ScheduledRule specifications onto AWS Batch SubmitJob payloads
//! with containerOverrides, VCPU/memory resource limits, and array properties.
//! Submission/polling goes through the shared `ClusterExecutor`
//! (`backend/cluster.rs`).
//!
//! `walltime` and `extra_args` are deliberately not mapped: an AWS Batch
//! SubmitJob payload carries neither — retry behavior lives in the job
//! definition and timeouts in `timeout.retryStrategy`, both operator-managed.

use super::{sanitize_cloud_name, scheduled_resources};
use crate::cluster::ClusterJobConfig;
use crate::error::Result;
use crate::rule::Rule;
use serde_json::json;

/// Sanitise a string for use as an AWS Batch job name (letters, numbers,
/// underscores, hyphens, max 128 chars, case preserved).
pub fn sanitize_aws_batch_name(name: &str) -> String {
    sanitize_cloud_name(name, 128, false, "_")
}

/// Build the AWS Batch `resourceRequirements` entries shared by single and
/// array jobs: VCPU + MEMORY plus the optional GPU entry (`gpu` or
/// `gpu_spec`). Array specs previously dropped the GPU requirement,
/// silently scheduling GPU rules onto CPU-only compute environments.
fn resource_requirements(rule: &Rule) -> Vec<serde_json::Value> {
    let (threads, mem_mb) = scheduled_resources(rule);

    let mut requirements = vec![
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
            requirements.push(json!({
                "type": "GPU",
                "value": format!("{gpu_count}")
            }));
        }
    } else if let Some(spec) = &rule.resources.gpu_spec
        && spec.count > 0
    {
        requirements.push(json!({
            "type": "GPU",
            "value": format!("{}", spec.count)
        }));
    }

    requirements
}

/// Generate an AWS Batch SubmitJob input JSON specification.
pub fn generate_aws_batch_job_spec(
    rule: &Rule,
    shell_cmd: &str,
    cluster_config: &ClusterJobConfig,
) -> Result<String> {
    let job_name = format!("oxoflow-{}", sanitize_aws_batch_name(&rule.name));

    let job_queue = cluster_config.queue.as_deref().unwrap_or("oxoflow-queue");

    let job_definition = cluster_config
        .account
        .as_deref()
        .unwrap_or("oxoflow-job-def");

    let payload = json!({
        "jobName": job_name,
        "jobQueue": job_queue,
        "jobDefinition": job_definition,
        "containerOverrides": {
            "command": ["/bin/bash", "-c", shell_cmd],
            "resourceRequirements": resource_requirements(rule)
        }
    });

    Ok(serde_json::to_string_pretty(&payload)?)
}

/// Generate an AWS Batch array job specification.
pub fn generate_aws_batch_array_spec(
    rule: &Rule,
    cmd_dir: &str,
    count: usize,
    cluster_config: &ClusterJobConfig,
) -> Result<String> {
    let job_name = format!("oxoflow-array-{}", sanitize_aws_batch_name(&rule.name));

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
            "resourceRequirements": resource_requirements(rule)
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

        let spec_str = generate_aws_batch_job_spec(&rule, "STAR --genomeDir ref", &test_config())
            .expect("spec generation must succeed");
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
            gpu: Some(1),
            ..Default::default()
        };
        let rule = Rule {
            name: "mutect2".into(),
            resources,
            ..Default::default()
        };

        let spec_str = generate_aws_batch_array_spec(&rule, "runs/cmds", 10, &test_config())
            .expect("spec generation must succeed");
        let val: Value = serde_json::from_str(&spec_str).expect("spec must be valid json");

        assert_eq!(val["arrayProperties"]["size"], 10);
        let cmd = val["containerOverrides"]["command"][2].as_str().unwrap();
        assert!(cmd.contains("AWS_BATCH_JOB_ARRAY_INDEX"));
        // Array specs must carry the same GPU requirement as single jobs.
        let gpu = val["containerOverrides"]["resourceRequirements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["type"] == "GPU")
            .expect("array spec must include GPU requirement");
        assert_eq!(gpu["value"], "1");
    }
}
