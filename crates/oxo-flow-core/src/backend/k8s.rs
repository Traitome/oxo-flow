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

/// Deterministic chunk/run-scoped suffix for a generated K8s array job name.
///
/// `seed` is the per-chunk job directory (`.../jobs/<template>/chunk-<k>`),
/// so the digest is stable for a given chunk on re-render (idempotent) and
/// differs across chunks of one template and across runs. Two chunks of the
/// same template no longer render the same `metadata.name` and collide on
/// `kubectl create` (issue #1226).
fn array_name_suffix(seed: &str) -> String {
    use sha2::{Digest, Sha256};
    // 8 hex chars is ample to separate the handful of concurrent chunks of
    // one template, and short enough to leave the rule name room under the
    // RFC 1123 63-char cap.
    hex::encode(Sha256::digest(seed.as_bytes()))[..8].to_string()
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
    // The job name carries a deterministic per-chunk suffix so sibling
    // chunks of one template (and repeat runs) get distinct names and do not
    // collide on `kubectl create` (issue #1226). The rule name is budgeted so
    // the whole name stays within the RFC 1123 63-char cap:
    // 14 ("oxoflow-array-") + 40 (rule) + 1 ("-") + 8 (suffix) = 63.
    let job_name = format!(
        "oxoflow-array-{}-{}",
        sanitize_cloud_name(&rule.name, 40, true, ""),
        array_name_suffix(cmd_dir)
    );

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

    /// Regression (issue #1226): two chunks of the SAME template must not
    /// render the same `metadata.name`, or the second `kubectl create` fails
    /// the run on a name collision. The suffix is derived from the per-chunk
    /// job dir, so sibling chunks differ, the same chunk re-renders stably
    /// (idempotent), and the whole name stays within the RFC 1123 cap.
    #[test]
    fn test_k8s_array_job_name_is_unique_per_chunk() {
        let rule = Rule {
            name: "scatter_qc".into(),
            ..Default::default()
        };
        let chunk1 = generate_k8s_array_job_manifest(
            &rule,
            "run/jobs/scatter_qc/chunk-1",
            3,
            &[],
            &test_config(),
        )
        .expect("chunk 1 manifest must render");
        let chunk2 = generate_k8s_array_job_manifest(
            &rule,
            "run/jobs/scatter_qc/chunk-2",
            3,
            &[],
            &test_config(),
        )
        .expect("chunk 2 manifest must render");
        let chunk1_again = generate_k8s_array_job_manifest(
            &rule,
            "run/jobs/scatter_qc/chunk-1",
            3,
            &[],
            &test_config(),
        )
        .expect("chunk 1 re-render must succeed");

        let name = |m: &str| -> String {
            let v: Value = serde_json::from_str(m).expect("manifest must be valid json");
            v["metadata"]["name"]
                .as_str()
                .expect("name must be a string")
                .to_string()
        };
        let n1 = name(&chunk1);
        let n2 = name(&chunk2);
        let n1_again = name(&chunk1_again);

        assert_ne!(n1, n2, "sibling chunks must not share a job name");
        assert_eq!(n1, n1_again, "the same chunk must re-render identically");
        // RFC 1123 label cap.
        assert!(n1.len() <= 63, "job name {n1} exceeds the 63-char cap");
        // A very long rule name must be truncated, never overflow the cap.
        let long_rule = Rule {
            name: "x".repeat(200),
            ..Default::default()
        };
        let long_name = name(
            &generate_k8s_array_job_manifest(
                &long_rule,
                "run/jobs/long/chunk-1",
                2,
                &[],
                &test_config(),
            )
            .expect("long-name manifest must render"),
        );
        assert!(
            long_name.len() <= 63,
            "truncated job name {long_name} still exceeds the cap"
        );
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

    /// Issue #1171: a submit-host ambient credential must never reach a
    /// Kubernetes job body. The manifest is built from the rule and the
    /// rendered command only; the pod's environment is the image `ENV` plus
    /// what this generator writes, and it writes no ambient state. The probe
    /// runs in a child process that carries sentinel credentials (the crate
    /// forbids `std::env::set_var`), so a future change that forwards the
    /// submit host's environment or inlines its values fails here.
    #[test]
    fn kubernetes_job_body_carries_no_submit_host_environment() {
        crate::backend::assert_cloud_body_ignores_submit_host_env("kubernetes");
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
