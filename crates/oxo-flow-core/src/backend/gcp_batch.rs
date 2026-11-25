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

//! # Sensitive config values (issue #1120 / ledger RB-12)
//!
//! A `sensitive = true` config value is routed out of `shell_cmd` into an
//! `OXO_FLOW_SENSITIVE_<n>` reference before this generator runs
//! (`ScheduledPlan::build_with_sensitive`, called from `run_cluster.rs`), so a
//! generated job body carries only the reference.
//!
//! GCP Batch resolves the value at submission time through
//! `taskSpec.environment.secretVariables`, whose value is a Secret Manager
//! resource path (`projects/PROJECT/secrets/SECRET/versions/VERSION`). The
//! engine cannot invent that path, so the operator supplies the mapping in
//! `OXO_FLOW_SECRET_REFS` (`NAME=projects/.../versions/...` pairs); the
//! generator emits an entry only for routed names that job actually uses —
//! unused mappings never reach a manifest. Without a mapping the reference
//! expands empty and the rule fails: loud rather than a silent credential
//! leak. The Batch service account needs
//! `roles/secretmanager.secretAccessor` on each referenced secret.

use super::{BackendJobStatus, sanitize_cloud_name, scheduled_resources};
use crate::cluster::ClusterJobConfig;
use crate::error::Result;
use crate::rule::Rule;
use serde_json::json;
use std::collections::BTreeMap;

/// Sanitise a string for use as a GCP Batch job name (lowercase alphanumeric
/// and hyphens, max 63 chars).
pub fn sanitize_gcp_batch_name(name: &str) -> String {
    sanitize_cloud_name(name, 63, true, "")
}

/// `taskSpec.environment` for the routed names that have an operator-supplied
/// reference. `None` when no used name is mapped — the historical, mapping-free
/// body (the operator injects out of band).
///
/// Only `secretVariables` is emitted: a plaintext `environment.variables`
/// entry would put the value (or a useless reference string) in the job body,
/// which is exactly what RB-12 forbids.
fn secret_variables(
    used: &[String],
    secret_refs: &BTreeMap<String, String>,
) -> Option<serde_json::Value> {
    let vars: serde_json::Map<String, serde_json::Value> = used
        .iter()
        .filter_map(|name| {
            secret_refs
                .get(name)
                .map(|reference| (name.clone(), json!(reference)))
        })
        .collect();
    if vars.is_empty() {
        None
    } else {
        Some(json!({ "secretVariables": vars }))
    }
}

/// Generate a Google Cloud Batch job JSON configuration.
pub fn generate_gcp_batch_job_spec(
    rule: &Rule,
    shell_cmd: &str,
    cluster_config: &ClusterJobConfig,
) -> Result<String> {
    generate_gcp_batch_job_spec_with_secrets(rule, shell_cmd, &BTreeMap::new(), cluster_config)
}

/// [`generate_gcp_batch_job_spec`] with the operator's `OXO_FLOW_SECRET_REFS`
/// mapping (issue #1120 / ledger RB-12).
pub fn generate_gcp_batch_job_spec_with_secrets(
    rule: &Rule,
    shell_cmd: &str,
    secret_refs: &BTreeMap<String, String>,
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

    let mut task_spec = json!({
        "runnables": [runnable],
        "computeResource": {
            "cpuMilli": threads * 1000,
            "memoryMib": mem_mb
        },
        "maxRetryCount": 0
    });
    if let Some(environment) = secret_variables(
        &crate::executor::process::sensitive_env_refs(shell_cmd),
        secret_refs,
    ) {
        task_spec["environment"] = environment;
    }

    let payload = json!({
        "taskGroups": [
            {
                "taskSpec": task_spec
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
    cluster_config: &ClusterJobConfig,
) -> Result<String> {
    generate_gcp_batch_array_spec_with_secrets(
        rule,
        cmd_dir,
        count,
        &[],
        &BTreeMap::new(),
        cluster_config,
    )
}

/// [`generate_gcp_batch_array_spec`] with the per-index command files'
/// routed names and the operator's `OXO_FLOW_SECRET_REFS` mapping (issue
/// #1120 / ledger RB-12). The dispatch line carries no reference itself, so
/// the driver passes the union of the command files' names in.
pub fn generate_gcp_batch_array_spec_with_secrets(
    rule: &Rule,
    cmd_dir: &str,
    count: usize,
    sensitive_refs: &[String],
    secret_refs: &BTreeMap<String, String>,
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

    let mut task_spec = json!({
        "runnables": [runnable],
        "computeResource": {
            "cpuMilli": threads * 1000,
            "memoryMib": mem_mb
        },
        "maxRetryCount": 0
    });
    if let Some(environment) = secret_variables(sensitive_refs, secret_refs) {
        task_spec["environment"] = environment;
    }

    let payload = json!({
        "taskGroups": [
            {
                "taskCount": count,
                "parallelism": count.min(64),
                "taskSpec": task_spec
            }
        ],
        "logsPolicy": {
            "destination": "CLOUD_LOGGING"
        }
    });

    Ok(serde_json::to_string_pretty(&payload)?)
}

/// Handle of one array element: a Batch task's `BATCH_TASK_INDEX` is
/// ZERO-based, while the driver's element ordinal is 1-based (element 1
/// runs task 0, matching the generated `cmd.$((index + 1)).sh` dispatch).
///
/// `gcloud` addresses tasks only inside a job (`gcloud batch tasks`), so
/// the `{job}:{taskIndex}` handle is engine-owned: `ClusterExecutor::poll`
/// resolves it by listing the job's tasks. It is not passed to the
/// scheduler as a job id (cancellation still targets the base job).
pub fn task_element_id(base: &str, ordinal: usize) -> String {
    format!("{base}:{}", ordinal.saturating_sub(1))
}

/// Parse `gcloud batch tasks list --format=value(name,status.state)` rows
/// into `task index -> status`.
///
/// Task names are fully qualified
/// (`projects/P/locations/L/jobs/J/taskGroups/G/tasks/<n>`); the trailing
/// index is what the handle from [`task_element_id`] names. Unrecognised
/// states map to [`BackendJobStatus::Unknown`] — missing status data is
/// never reported as success (issue #1132 / ledger RB-06). `UNEXECUTED` is
/// the task.proto terminal state for a task the Job finished without
/// running; it is a failure of that element, not missing data, so it
/// settles instead of waiting for the driver's blind-settlement guard.
pub fn parse_task_states(output: &str) -> BTreeMap<u32, BackendJobStatus> {
    let mut states = BTreeMap::new();
    for line in output.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let mut fields = line.split_whitespace();
        let (Some(name), Some(state)) = (fields.next(), fields.next()) else {
            continue;
        };
        let Some((_, index)) = name.rsplit_once("/tasks/") else {
            continue;
        };
        let Ok(index) = index.parse::<u32>() else {
            continue;
        };
        let status = match state.trim().to_uppercase().as_str() {
            "PENDING" | "SCHEDULED" => BackendJobStatus::Pending,
            "ASSIGNED" | "RUNNING" => BackendJobStatus::Running,
            "SUCCEEDED" => BackendJobStatus::Completed,
            // TaskStatus.State has no CANCELLED member; a task of a
            // cancelled/deleted job either failed or was never executed.
            "FAILED" | "UNEXECUTED" => BackendJobStatus::Failed,
            _ => BackendJobStatus::Unknown,
        };
        states.insert(index, status);
    }
    states
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
            name_scope: None,
            volume: None,
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

    /// Ledger RB-06: element handles are zero-based task indexes, matching
    /// `BATCH_TASK_INDEX`.
    #[test]
    fn task_element_ids_are_zero_based() {
        assert_eq!(
            task_element_id("oxoflow-array-trim-1", 1),
            "oxoflow-array-trim-1:0"
        );
        assert_eq!(
            task_element_id("oxoflow-array-trim-1", 6),
            "oxoflow-array-trim-1:5"
        );
        assert_eq!(
            task_element_id("oxoflow-array-trim-1", 0),
            "oxoflow-array-trim-1:0"
        );
    }

    /// Ledger RB-06: `gcloud batch tasks list` rows resolve to independent
    /// per-element states; unknown or malformed rows never read as success.
    #[test]
    fn parse_task_states_reads_per_element_outcomes() {
        let output = "\
projects/p/locations/us-central1/jobs/oxoflow-array-trim-1/taskGroups/group0/tasks/0\tSUCCEEDED
projects/p/locations/us-central1/jobs/oxoflow-array-trim-1/taskGroups/group0/tasks/1\tFAILED
projects/p/locations/us-central1/jobs/oxoflow-array-trim-1/taskGroups/group0/tasks/2\tRUNNING
projects/p/locations/us-central1/jobs/oxoflow-array-trim-1/taskGroups/group0/tasks/3\tSOMETHING_NEW
projects/p/locations/us-central1/jobs/oxoflow-array-trim-1/taskGroups/group0/tasks/4\tUNEXECUTED
";
        let states = parse_task_states(output);
        assert_eq!(states.get(&0), Some(&BackendJobStatus::Completed));
        assert_eq!(states.get(&1), Some(&BackendJobStatus::Failed));
        assert_eq!(states.get(&2), Some(&BackendJobStatus::Running));
        assert_eq!(
            states.get(&3),
            Some(&BackendJobStatus::Unknown),
            "an unrecognised state must not be optimistically mapped"
        );
        assert_eq!(
            states.get(&4),
            Some(&BackendJobStatus::Failed),
            "UNEXECUTED is a terminal non-success task state (task.proto), \
             not missing data"
        );
        // Malformed rows are skipped, not guessed.
        assert!(parse_task_states("garbage\n\n").is_empty());
    }

    /// Issue #1171: a submit-host ambient credential must never reach a GCP
    /// Batch job body. The job config is built from the rule and the rendered
    /// command only; the task's environment is the VM/container image plus
    /// `taskSpec.environment`, which this generator leaves to the operator.
    /// The probe runs in a child process that carries sentinel credentials
    /// (the crate forbids `std::env::set_var`), so a future change that
    /// forwards the submit host's environment or inlines its values fails
    /// here.
    #[test]
    fn gcp_batch_job_body_carries_no_submit_host_environment() {
        crate::backend::assert_cloud_body_ignores_submit_host_env("gcp-batch");
    }

    /// Sentinel regression (ledger RB-12): after the plan routes a
    /// `sensitive = true` value out of the command, no GCP Batch body carries
    /// the literal — only the `OXO_FLOW_SENSITIVE_<n>` reference, which the
    /// operator maps in `taskSpec.environment.secretVariables` at submission
    /// (the engine cannot know the project, so it does not guess a path).
    #[test]
    fn test_sensitive_value_never_in_gcp_batch_body() {
        let sentinel = "SENTINEL_SECRET_VALUE_E5";
        let (routed, env) = crate::executor::process::route_sensitive_values_through_env(
            &format!("tool --api-key {sentinel} --run"),
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
        let single = generate_gcp_batch_job_spec(&rule, &routed, &test_config()).unwrap();
        assert!(
            !single.contains(sentinel),
            "sentinel leaked into the GCP Batch body: {single}"
        );
        assert!(
            single.contains("OXO_FLOW_SENSITIVE_0"),
            "the routed reference must survive into the body: {single}"
        );

        let array = generate_gcp_batch_array_spec(&rule, "runs/cmds", 4, &test_config()).unwrap();
        assert!(
            !array.contains(sentinel),
            "sentinel leaked into the GCP Batch array body: {array}"
        );
    }

    /// Ledger RB-12, injection half: with the operator's
    /// `OXO_FLOW_SECRET_REFS` mapping, the body carries submission-time
    /// `taskSpec.environment.secretVariables` entries — one per routed name
    /// the job actually uses — and never the value. Unused mappings never
    /// reach a manifest (least privilege).
    #[test]
    fn test_gcp_batch_emits_secret_variables_from_the_operator_mapping() {
        let sentinel = "SENTINEL_SECRET_VALUE_G6";
        let (routed, _env) = crate::executor::process::route_sensitive_values_through_env(
            &format!("tool --api-key {sentinel} --run"),
            &[sentinel.to_string()],
        );
        let refs = std::collections::BTreeMap::from([
            (
                "OXO_FLOW_SENSITIVE_0".to_string(),
                "projects/my-proj/secrets/api-key/versions/latest".to_string(),
            ),
            (
                "OXO_FLOW_SENSITIVE_9".to_string(),
                "projects/my-proj/secrets/unused/versions/1".to_string(),
            ),
        ]);
        let rule = Rule {
            name: "call_variants".into(),
            ..Default::default()
        };

        let single =
            generate_gcp_batch_job_spec_with_secrets(&rule, &routed, &refs, &test_config())
                .unwrap();
        assert!(
            !single.contains(sentinel),
            "sentinel leaked into the GCP Batch body: {single}"
        );
        let val: Value = serde_json::from_str(&single).unwrap();
        let vars = &val["taskGroups"][0]["taskSpec"]["environment"]["secretVariables"];
        assert_eq!(
            vars["OXO_FLOW_SENSITIVE_0"],
            "projects/my-proj/secrets/api-key/versions/latest"
        );
        assert!(
            vars["OXO_FLOW_SENSITIVE_9"].is_null(),
            "an unused mapping must not reach the body: {single}"
        );

        // Array bodies dispatch to per-index files; the driver passes the
        // union of their references in.
        let array = generate_gcp_batch_array_spec_with_secrets(
            &rule,
            "runs/cmds",
            4,
            &["OXO_FLOW_SENSITIVE_0".to_string()],
            &refs,
            &test_config(),
        )
        .unwrap();
        assert!(!array.contains(sentinel));
        let val: Value = serde_json::from_str(&array).unwrap();
        assert_eq!(
            val["taskGroups"][0]["taskSpec"]["environment"]["secretVariables"]["OXO_FLOW_SENSITIVE_0"],
            "projects/my-proj/secrets/api-key/versions/latest"
        );

        // Without a mapping the historical body is unchanged: no environment
        // channel at all (the value would fail loudly rather than leak).
        let unmapped = generate_gcp_batch_job_spec(&rule, &routed, &test_config()).unwrap();
        let val: Value = serde_json::from_str(&unmapped).unwrap();
        assert!(val["taskGroups"][0]["taskSpec"]["environment"].is_null());
    }
}
