//! Kubernetes batch/v1 Job manifest generator.
//!
//! Maps ScheduledRule resources and container specifications onto Kubernetes
//! batch/v1 Job manifests with deterministic labels. Submission/polling goes
//! through the shared `ClusterExecutor` (`backend/cluster.rs`).

use super::sanitize_cloud_name;
use crate::cluster::ClusterJobConfig;
use crate::rule::Rule;
use serde_json::json;

/// Sanitise a string for use as a Kubernetes resource name or label value.
/// RFC 1123 label: lowercase alphanumeric characters or minus (-), max 63 chars.
pub fn sanitize_k8s_name(name: &str) -> String {
    sanitize_cloud_name(name, 63, true, "")
}

/// Build the container resource block shared by single and array jobs:
/// cpu/memory requests, and cpu/memory limits plus the optional NVIDIA GPU
/// limit (`gpu` or `gpu_spec`). Array jobs previously dropped the GPU
/// limit, silently scheduling GPU rules onto CPU-only nodes.
fn container_resources(rule: &Rule) -> serde_json::Value {
    let threads = rule.resources.threads.max(1);
    let mem_mb = crate::scheduler::effective_memory_mb(rule, 1024).max(256);

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

    json!({
        "requests": {
            "cpu": format!("{threads}"),
            "memory": format!("{mem_mb}Mi"),
        },
        "limits": limits,
    })
}

/// Generate a Kubernetes batch/v1 Job manifest for a single rule.
pub fn generate_k8s_job_manifest(
    rule: &Rule,
    shell_cmd: &str,
    cluster_config: &ClusterJobConfig,
) -> String {
    let job_name = format!("oxoflow-{}", sanitize_k8s_name(&rule.name));

    let container_image = rule
        .environment
        .docker
        .as_deref()
        .unwrap_or("ubuntu:latest");

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
                            "resources": container_resources(rule),
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
                            "resources": container_resources(rule),
                        }
                    ]
                }
            }
        }
    });

    let _ = cluster_config;
    serde_json::to_string_pretty(&manifest).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::ClusterBackend;
    use crate::rule::Rule;
    use serde_json::Value;

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
            gpu: Some(2),
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
        // Array jobs must carry the same GPU limit as single jobs.
        assert_eq!(container["resources"]["limits"]["nvidia.com/gpu"], "2");
    }
}
