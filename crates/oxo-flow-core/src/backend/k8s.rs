//! Kubernetes batch/v1 Job manifest generator.
//!
//! Maps ScheduledRule resources and container specifications onto Kubernetes
//! batch/v1 Job manifests with deterministic labels. Submission/polling goes
//! through the shared `ClusterExecutor` (`backend/cluster.rs`).

use super::{sanitize_cloud_name, scheduled_resources};
use crate::cluster::ClusterJobConfig;
use crate::error::Result;
use crate::executor::process::sensitive_env_refs;
use crate::rule::Rule;
use serde_json::{Value, json};

/// Sanitise a string for use as a Kubernetes resource name or label value.
/// RFC 1123 label: lowercase alphanumeric characters or minus (-), max 63 chars.
pub fn sanitize_k8s_name(name: &str) -> String {
    sanitize_cloud_name(name, 63, true, "")
}

/// Kubernetes Secret the manifest pulls `sensitive = true` config values
/// from (issue #1120 / ledger RB-12). The operator creates it once per
/// workflow, keyed by the same `OXO_FLOW_SENSITIVE_<n>` variable names the
/// local executor routes values under, so a manifest never carries a secret
/// literal in `args`.
///
/// ```sh
/// kubectl create secret generic oxoflow-secrets \
///   --from-literal=OXO_FLOW_SENSITIVE_0=<value>
/// ```
pub const K8S_SENSITIVE_SECRET: &str = "oxoflow-secrets";

/// Attach the secret-backed `env` array to a container block: every sensitive
/// value referenced by the rendered command is projected out of the
/// operator-provided Kubernetes Secret rather than inlined as a literal.
/// A container whose command carries no routed sensitive reference is left
/// untouched, so ordinary jobs stay byte-identical.
///
/// The Secret key IS the environment variable name: the plan-level routing
/// numbers bindings once for the whole plan (`sensitive_bindings`), so the
/// name is unambiguous across rules and across array elements — a per-rule
/// key namespace would break indexed jobs, whose elements are named after
/// the template rather than the instance.
fn attach_sensitive_env(container: &mut Value, refs: &[String]) {
    let env: Vec<Value> = refs
        .iter()
        .map(|var| {
            json!({
                "name": var,
                "valueFrom": {
                    "secretKeyRef": {
                        "name": K8S_SENSITIVE_SECRET,
                        "key": var,
                    }
                }
            })
        })
        .collect();
    if !env.is_empty() {
        container["env"] = json!(env);
    }
}

/// Build the container resource block shared by single and array jobs:
/// cpu/memory requests, and cpu/memory limits plus the optional NVIDIA GPU
/// limit (`gpu` or `gpu_spec`). Array jobs previously dropped the GPU
/// limit, silently scheduling GPU rules onto CPU-only nodes.
fn container_resources(rule: &Rule) -> serde_json::Value {
    let (threads, mem_mb) = scheduled_resources(rule);

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
    // Kept for dispatch uniformity with the other generators; Kubernetes
    // manifests currently derive everything from the rule itself.
    _cluster_config: &ClusterJobConfig,
) -> Result<String> {
    let job_name = format!("oxoflow-{}", sanitize_k8s_name(&rule.name));

    let container_image = rule
        .environment
        .docker
        .as_deref()
        .unwrap_or("ubuntu:latest");

    let mut container = json!({
        "name": sanitize_k8s_name(&rule.name),
        "image": container_image,
        "command": ["/bin/bash", "-c"],
        "args": [shell_cmd],
        "resources": container_resources(rule),
    });
    attach_sensitive_env(&mut container, &sensitive_env_refs(shell_cmd));

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
                    "containers": [container]
                }
            }
        }
    });

    Ok(serde_json::to_string_pretty(&manifest)?)
}

/// Generate a Kubernetes indexed Job manifest for an array submission.
pub fn generate_k8s_array_job_manifest(
    rule: &Rule,
    cmd_dir: &str,
    count: usize,
    // Union of `OXO_FLOW_SENSITIVE_<n>` references over every per-index
    // command file — the dispatch line carries none of them itself.
    sensitive_refs: &[String],
    _cluster_config: &ClusterJobConfig,
) -> Result<String> {
    let job_name = format!("oxoflow-array-{}", sanitize_k8s_name(&rule.name));

    let container_image = rule
        .environment
        .docker
        .as_deref()
        .unwrap_or("ubuntu:latest");

    let dispatch_cmd =
        format!("INDEX=$((JOB_COMPLETION_INDEX + 1)); sh \"{cmd_dir}/cmd.${{INDEX}}.sh\"");

    let mut container = json!({
        "name": sanitize_k8s_name(&rule.name),
        "image": container_image,
        "command": ["/bin/bash", "-c"],
        "args": [dispatch_cmd],
        "resources": container_resources(rule),
    });
    attach_sensitive_env(&mut container, sensitive_refs);

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
                    "containers": [container]
                }
            }
        }
    });

    Ok(serde_json::to_string_pretty(&manifest)?)
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
            generate_k8s_job_manifest(&rule, "bwa mem ref.fa reads.fq", &test_config())
                .expect("manifest generation must succeed");
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

        let manifest_str =
            generate_k8s_array_job_manifest(&rule, "run/cmds", 5, &[], &test_config())
                .expect("manifest generation must succeed");
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

    /// Sentinel regression (ledger RB-12): once the plan routes a
    /// `sensitive = true` value into an `OXO_FLOW_SENSITIVE_<n>` reference,
    /// the manifest carries a `secretKeyRef`, never the literal.
    #[test]
    fn test_k8s_manifest_projects_sensitive_values_from_a_secret() {
        let sentinel = "SENTINEL_SECRET_VALUE_D4";
        let (routed, env) = crate::executor::process::route_sensitive_values_through_env(
            &format!("tool --token {sentinel} --out x"),
            &[sentinel.to_string()],
        );
        assert_eq!(
            env.get("OXO_FLOW_SENSITIVE_0").map(String::as_str),
            Some(sentinel)
        );

        let rule = Rule {
            name: "call_variants".into(),
            ..Default::default()
        };
        let manifest_str =
            generate_k8s_job_manifest(&rule, &routed, &test_config()).expect("manifest");
        assert!(
            !manifest_str.contains(sentinel),
            "sentinel leaked into the k8s manifest: {manifest_str}"
        );
        let val: Value = serde_json::from_str(&manifest_str).unwrap();
        let env_entry = &val["spec"]["template"]["spec"]["containers"][0]["env"][0];
        assert_eq!(env_entry["name"], "OXO_FLOW_SENSITIVE_0");
        let key_ref = &env_entry["valueFrom"]["secretKeyRef"];
        assert_eq!(key_ref["name"], K8S_SENSITIVE_SECRET);
        assert_eq!(key_ref["key"], "OXO_FLOW_SENSITIVE_0");
    }

    /// Indexed jobs dispatch to per-index command files, so the dispatch line
    /// carries no reference — the driver passes the union in. The env block is
    /// still attached, and ordinary (reference-free) jobs stay untouched.
    #[test]
    fn test_k8s_array_manifest_attaches_the_routed_secret_refs() {
        let rule = Rule {
            name: "scatter_qc".into(),
            ..Default::default()
        };
        let refs = vec!["OXO_FLOW_SENSITIVE_0".to_string()];
        let manifest_str =
            generate_k8s_array_job_manifest(&rule, "run/cmds", 5, &refs, &test_config()).unwrap();
        let val: Value = serde_json::from_str(&manifest_str).unwrap();
        let env_entry = &val["spec"]["template"]["spec"]["containers"][0]["env"][0];
        assert_eq!(env_entry["name"], "OXO_FLOW_SENSITIVE_0");

        let plain =
            generate_k8s_array_job_manifest(&rule, "run/cmds", 5, &[], &test_config()).unwrap();
        assert!(
            !plain.contains("\"env\""),
            "a reference-free job must stay byte-identical: {plain}"
        );
    }
}
