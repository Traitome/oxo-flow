//! Kubernetes batch/v1 Job manifest generator.
//!
//! Maps ScheduledRule resources and container specifications onto Kubernetes
//! batch/v1 Job manifests with deterministic labels. Submission/polling goes
//! through the shared `ClusterExecutor` (`backend/cluster.rs`).

use super::{BackendJobStatus, sanitize_cloud_name, scheduled_resources};
use crate::cluster::ClusterJobConfig;
use crate::error::Result;
use crate::executor::process::sensitive_env_refs;
use crate::rule::Rule;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Extract the `cd '<path>' &&` workdir prefix the shared plan builder folds
/// into every rule command. Kubernetes Jobs run in a pod with its own
/// filesystem, so the run workdir must be mounted at the SAME absolute path
/// for the rule's declared outputs (and the engine's checkpoint/log files) to
/// land on the shared host tree. Returns `None` when the prefix is absent
/// (backends/plans that do not fold `cd`, e.g. hand-written commands).
fn workdir_from_shell_cmd(shell_cmd: &str) -> Option<String> {
    // The plan builder escapes a literal quote as '\'' (four shell bytes:
    // quote, backslash, quote, quote) inside the single-quoted path.
    const ESCAPED_QUOTE: &str = "'\\''";
    let rest = shell_cmd.strip_prefix("cd '")?;
    let mut out = String::new();
    let mut index = 0usize;
    while index < rest.len() {
        if rest[index..].starts_with(ESCAPED_QUOTE) {
            out.push('\'');
            index += ESCAPED_QUOTE.len();
            continue;
        }
        if rest.as_bytes()[index] == b'\'' {
            rest[index + 1..].strip_prefix(" && ")?;
            return Some(out);
        }
        let ch = rest[index..].chars().next()?;
        out.push(ch);
        index += ch.len_utf8();
    }
    None
}
/// Mount the run workdir at its host path when the plan folded a `cd` guard
/// into the command (single-node / shared-filesystem clusters; the documented
/// Kubernetes deployment mode). Multus/PVC deployments pin their own mounts.
fn attach_workdir_volume(container: &mut Value, shell_cmd: &str) {
    let Some(workdir) = workdir_from_shell_cmd(shell_cmd) else {
        return;
    };
    container["volumeMounts"] = json!([
        { "name": "oxoflow-workdir", "mountPath": workdir }
    ]);
}

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
    cluster_config: &ClusterJobConfig,
) -> Result<String> {
    // A run/submission scope (the run directory or `cluster submit` output
    // directory) is folded into the name so two runs of the same workflow do
    // not collide on AlreadyExists (issue #1158 live campaign: the second
    // `run --profile k8s` failed with `jobs.batch "oxoflow-make" already
    // exists`). No scope keeps the historical stable name.
    let base = sanitize_cloud_name(&rule.name, 40, true, "");
    let job_name = match cluster_config.name_scope.as_deref() {
        Some(scope) if !scope.is_empty() => {
            use sha2::{Digest, Sha256};
            let suffix = hex::encode(Sha256::digest(format!("{scope}|{}", rule.name).as_bytes()))
                [..8]
                .to_string();
            format!("oxoflow-{base}-{suffix}")
        }
        _ => format!("oxoflow-{base}"),
    };

    let container_image = rule
        .environment
        .docker
        .as_deref()
        .unwrap_or("ubuntu:latest");

    let mut container = json!({
        "name": sanitize_k8s_name(&rule.name),
        "image": container_image,
        // Local/single-node clusters preload images (kind/k3s `load`); an
        // implicit `Always` pull on `:latest` would need a registry the
        // compute node may not reach.
        "imagePullPolicy": "IfNotPresent",
        "command": ["/bin/bash", "-c"],
        "args": [shell_cmd],
        "resources": container_resources(rule),
    });
    attach_sensitive_env(&mut container, &sensitive_env_refs(shell_cmd));
    attach_workdir_volume(&mut container, shell_cmd);
    let workdir = workdir_from_shell_cmd(shell_cmd);

    let mut pod_spec = json!({
        "restartPolicy": "Never",
        "containers": [container]
    });
    if let Some(workdir) = &workdir {
        pod_spec["volumes"] = json!([
            { "name": "oxoflow-workdir", "hostPath": { "path": workdir, "type": "Directory" } }
        ]);
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
                "spec": pod_spec
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
        "imagePullPolicy": "IfNotPresent",
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

/// Parse a Kubernetes index list — the `status.completedIndexes` /
/// `status.failedIndexes` format — into the index set it names.
///
/// The wire format is a comma-separated list of single indexes and
/// inclusive ranges (`"0-2,4"`). Malformed segments are ignored rather
/// than guessed: a garbage status field must leave the element unknown,
/// never invent a completed index (issue #1132 / ledger RB-06).
pub fn parse_index_list(spec: &str) -> BTreeSet<u32> {
    let mut indexes = BTreeSet::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.split_once('-') {
            Some((start, end)) => {
                if let (Ok(start), Ok(end)) =
                    (start.trim().parse::<u32>(), end.trim().parse::<u32>())
                    && start <= end
                {
                    indexes.extend(start..=end);
                }
            }
            None => {
                if let Ok(index) = part.parse::<u32>() {
                    indexes.insert(index);
                }
            }
        }
    }
    indexes
}

/// The terminal phase of an Indexed Job, read from
/// `status.conditions[*].type` (`Failed` / `Complete`).
///
/// With `backoffLimit: 0` a single pod failure terminally fails the Job and
/// the controller never creates the remaining pods (`parallelism` caps the
/// in-flight count), so indexes that appear in neither status list on a
/// terminal-Failed Job will never run — they must settle, or the driver
/// polls a dead Job forever (an element verdict of `Running` resets the
/// driver's blind-settlement guard). A `Complete` Job with an unrecorded
/// index is contradictory wire data, not a provable outcome: the index is
/// [`BackendJobStatus::Unknown`] — never a fabricated success or failure —
/// and the driver's terminal-unknown guard settles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedJobTerminal {
    /// `status.conditions` contains `type: Failed`.
    Failed,
    /// `status.conditions` contains `type: Complete`.
    Complete,
}

/// The verdict an Indexed Job's status proves for one completion index.
///
/// `completed` / `failed` come from
/// [`parse_index_list`] over `status.completedIndexes` /
/// `status.failedIndexes`; `failed_count` is `status.failed`. A failed
/// count with no `failedIndexes` data (the field is Kubernetes 1.31+)
/// proves the Job is terminal-failed but not WHICH element failed, so
/// that element is [`BackendJobStatus::Unknown`] — never a success and
/// never a fabricated failure. An element that is neither recorded
/// completed nor recorded failed, while the Job is not in that blind
/// state, is still live — unless the Job itself is terminal
/// ([`IndexedJobTerminal`]): an unrecorded index can then never start, so
/// it settles instead of reporting live forever.
pub fn indexed_element_status(
    completed: &BTreeSet<u32>,
    failed: &BTreeSet<u32>,
    failed_count: u64,
    index: u32,
    job_terminal: Option<IndexedJobTerminal>,
) -> BackendJobStatus {
    if completed.contains(&index) {
        return BackendJobStatus::Completed;
    }
    if failed.contains(&index) {
        return BackendJobStatus::Failed;
    }
    if failed_count > 0 && failed.is_empty() {
        return BackendJobStatus::Unknown;
    }
    match job_terminal {
        // backoffLimit: 0 — the controller will never create or restart
        // this index; the work will not be done, so the element fails.
        Some(IndexedJobTerminal::Failed) => BackendJobStatus::Failed,
        // A Complete Job must have run every index; a missing index on a
        // Complete Job is wire data we cannot interpret — settle as
        // unknown, never fabricate the outcome.
        Some(IndexedJobTerminal::Complete) => BackendJobStatus::Unknown,
        None => BackendJobStatus::Running,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::ClusterBackend;
    use crate::rule::Rule;
    use serde_json::Value;

    fn k8s_test_config() -> ClusterJobConfig {
        ClusterJobConfig {
            backend: crate::cluster::ClusterBackend::Kubernetes,
            queue: None,
            account: None,
            walltime: None,
            extra_args: Vec::new(),
            name_scope: None,
        }
    }

    #[test]
    fn job_manifest_mounts_the_folded_workdir_at_its_host_path() {
        let rule = Rule {
            name: "hello".to_string(),
            output: crate::rule::FilePatterns::List(vec!["hello.txt".to_string()]),
            shell: Some("echo hi > hello.txt".to_string()),
            ..Default::default()
        };
        let cmd = "cd '/tmp/run work' && echo hi > hello.txt";
        let manifest: Value = serde_json::from_str(
            &generate_k8s_job_manifest(&rule, cmd, &k8s_test_config()).unwrap(),
        )
        .unwrap();
        let container = &manifest["spec"]["template"]["spec"]["containers"][0];
        assert_eq!(container["volumeMounts"][0]["mountPath"], "/tmp/run work");
        assert_eq!(
            manifest["spec"]["template"]["spec"]["volumes"][0]["hostPath"]["path"],
            "/tmp/run work"
        );
        assert_eq!(
            manifest["spec"]["template"]["spec"]["volumes"][0]["hostPath"]["type"],
            "Directory"
        );

        // A command without the folded `cd` keeps the previous manifest
        // shape (no volumes) — never an empty hostPath.
        let bare: Value = serde_json::from_str(
            &generate_k8s_job_manifest(&rule, "echo hi > hello.txt", &k8s_test_config()).unwrap(),
        )
        .unwrap();
        assert!(
            bare["spec"]["template"]["spec"].get("volumes").is_none(),
            "{bare}"
        );
    }

    #[test]
    fn job_names_are_scoped_per_run_so_reruns_do_not_collide() {
        let rule = Rule {
            name: "make".to_string(),
            output: crate::rule::FilePatterns::List(vec!["a.txt".to_string()]),
            shell: Some("true".to_string()),
            ..Default::default()
        };
        let cmd = "cd '/tmp/w' && true";
        let name_for = |scope: Option<&str>| -> String {
            let mut config = k8s_test_config();
            config.name_scope = scope.map(str::to_string);
            let manifest: Value =
                serde_json::from_str(&generate_k8s_job_manifest(&rule, cmd, &config).unwrap())
                    .unwrap();
            manifest["metadata"]["name"].as_str().unwrap().to_string()
        };
        assert_eq!(name_for(None), "oxoflow-make");
        let first = name_for(Some("/runs/2026-11-24T15-00"));
        let second = name_for(Some("/runs/2026-11-24T15-05"));
        assert_ne!(first, second, "two run scopes must not share a Job name");
        assert!(first.starts_with("oxoflow-make-"), "{first}");
        assert!(first.len() <= 63, "{first}");
    }

    #[test]
    fn workdir_parser_handles_single_quote_escapes() {
        assert_eq!(
            workdir_from_shell_cmd("cd '/tmp/a' && true").as_deref(),
            Some("/tmp/a")
        );
        assert_eq!(
            workdir_from_shell_cmd("cd '/tmp/it'\\''s' && true").as_deref(),
            Some("/tmp/it's")
        );
        assert_eq!(workdir_from_shell_cmd("true"), None);
    }

    fn test_config() -> ClusterJobConfig {
        ClusterJobConfig {
            backend: ClusterBackend::Kubernetes,
            queue: None,
            account: None,
            walltime: None,
            extra_args: Vec::new(),
            name_scope: None,
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

    /// Ledger RB-06: the Indexed Job status fields the driver's per-element
    /// poller reads must parse into the completion-index set they name.
    #[test]
    fn parse_index_list_reads_the_indexed_job_status_format() {
        assert_eq!(
            parse_index_list("0-2,4"),
            BTreeSet::from([0, 1, 2, 4]),
            "ranges are inclusive and comma-separated"
        );
        assert_eq!(parse_index_list("3"), BTreeSet::from([3]));
        assert_eq!(parse_index_list(""), BTreeSet::new());
        // Defensive: junk never fabricates an index.
        assert_eq!(parse_index_list("4-2"), BTreeSet::new());
        assert_eq!(parse_index_list("x,1,,2-y"), BTreeSet::from([1]));
    }

    /// Ledger RB-06: per-element verdicts stay independent, and missing
    /// failedIndexes data is Unknown — not success, and not copied onto
    /// every element.
    #[test]
    fn indexed_element_status_is_per_element_and_never_fabricates() {
        let completed = parse_index_list("0,2");
        let failed = parse_index_list("1");
        for (index, expected) in [
            (0, BackendJobStatus::Completed),
            (1, BackendJobStatus::Failed),
            (2, BackendJobStatus::Completed),
            // Index 3 is neither finished nor failed: still live.
            (3, BackendJobStatus::Running),
        ] {
            assert_eq!(
                indexed_element_status(&completed, &failed, 1, index, None),
                expected,
                "index {index} must have its own verdict"
            );
        }
        // Kubernetes < 1.31 has status.failed but no failedIndexes: the
        // Job is terminal-failed and the failing element is unknowable.
        // It must be Unknown, never success and never assumed to be index 0.
        let unknown = indexed_element_status(&BTreeSet::new(), &BTreeSet::new(), 2, 1, None);
        assert_eq!(
            unknown,
            BackendJobStatus::Unknown,
            "missing failedIndexes must not pin the failure on a guessed element"
        );
    }

    /// backoffLimit: 0 makes a terminal-Failed Indexed Job final: the
    /// controller never creates the remaining pods (`parallelism` caps
    /// in-flight elements), so indexes recorded in NEITHER status list
    /// would previously report `Running` forever — resetting the driver's
    /// blind-settlement guard and polling a dead Job with no wall-clock
    /// budget. A terminal-Failed Job settles the unrecorded index as
    /// Failed; a Complete Job with an unrecorded index is contradictory
    /// wire data, so it settles as Unknown (never fabricated success).
    #[test]
    fn unrecorded_index_settles_when_the_job_is_terminal() {
        let completed = parse_index_list("0");
        let failed = parse_index_list("1");
        // Indexes 2..63 never got a pod (parallelism capped at 2): the
        // recorded sets prove nothing about them, the Job does.
        for index in [2, 63] {
            assert_eq!(
                indexed_element_status(
                    &completed,
                    &failed,
                    1,
                    index,
                    Some(IndexedJobTerminal::Failed)
                ),
                BackendJobStatus::Failed,
                "an unrecorded index on a terminal-Failed Job can never run"
            );
        }
        // The recorded verdicts are untouched by the terminal phase.
        assert_eq!(
            indexed_element_status(&completed, &failed, 1, 0, Some(IndexedJobTerminal::Failed)),
            BackendJobStatus::Completed
        );
        assert_eq!(
            indexed_element_status(&completed, &failed, 1, 1, Some(IndexedJobTerminal::Failed)),
            BackendJobStatus::Failed
        );
        // A live Job keeps unrecorded indexes live — the fix must not
        // settle anything on a Job that is still running.
        assert_eq!(
            indexed_element_status(&completed, &failed, 1, 2, None),
            BackendJobStatus::Running
        );
        // Complete-but-unrecorded is contradictory wire data: Unknown, so
        // the driver's terminal-unknown guard can settle it, never a
        // fabricated success.
        assert_eq!(
            indexed_element_status(
                &completed,
                &failed,
                1,
                2,
                Some(IndexedJobTerminal::Complete)
            ),
            BackendJobStatus::Unknown
        );
        // The blind state (no failedIndexes field) keeps its documented
        // Unknown contract even on a terminal-Failed Job.
        assert_eq!(
            indexed_element_status(
                &BTreeSet::new(),
                &BTreeSet::new(),
                2,
                1,
                Some(IndexedJobTerminal::Failed)
            ),
            BackendJobStatus::Unknown
        );
    }
}
