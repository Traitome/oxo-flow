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
//!
//! # Sensitive config values (issue #1120 / ledger RB-12)
//!
//! A `sensitive = true` config value must never be inlined in the job body.
//! The cluster plan routes each such value out of `shell_cmd` into an
//! `OXO_FLOW_SENSITIVE_<n>` reference before this generator runs
//! (`ScheduledPlan::route_sensitive_values`, called from `run_cluster.rs`), so
//! `containerOverrides.command` carries only the *name*.
//!
//! The value itself cannot be attached here: the SubmitJob `ContainerOverrides`
//! structure has **no `secrets` field** (only `environment` — plaintext), and
//! this generator receives neither the value nor a path to it. The faithful
//! AWS mechanism is the **job definition's** `containerProperties.secrets`
//! (`[{ "name": "OXO_FLOW_SENSITIVE_0", "valueFrom": "<arn>" }]`, or
//! `environment`), which is operator-managed and never part of a SubmitJob
//! call. The operator must therefore map each routed `OXO_FLOW_SENSITIVE_<n>`
//! name on the job definition (or via `environment` in
//! `containerOverrides`, added by whoever holds the value); the container then
//! resolves the reference from its own environment.

use super::{sanitize_cloud_name, scheduled_resources};
use crate::cluster::ClusterJobConfig;
use crate::error::Result;
use crate::rule::Rule;
use serde_json::json;

/// AWS Batch job names: at most 128 chars, first char alphanumeric,
/// letters/numbers/hyphens/underscores (`SubmitJobRequest.jobName`).
pub const AWS_JOB_NAME_MAX: usize = 128;

/// Engine prefix on single-job names; counted against the 128-char cap.
pub const AWS_JOB_NAME_PREFIX: &str = "oxoflow-";

/// Engine prefix on array-job names; counted against the 128-char cap.
pub const AWS_ARRAY_JOB_NAME_PREFIX: &str = "oxoflow-array-";

/// Default job queue when the profile's `queue` key is unset. Shared with
/// the `cluster status` listing (`my_jobs_invocations`) so submit and list
/// never disagree about which queue the engine uses.
pub const AWS_DEFAULT_JOB_QUEUE: &str = "oxoflow-queue";

/// Default job definition when the profile's `account` key is unset (the
/// shared `ClusterJobConfig.account` key doubles as the AWS job
/// definition).
pub const AWS_DEFAULT_JOB_DEFINITION: &str = "oxoflow-job-def";

/// Sanitise a rule name into the portion of an AWS Batch job name that
/// follows [`AWS_JOB_NAME_PREFIX`]: the budget is
/// [`AWS_JOB_NAME_MAX`] minus the prefix, so prefixing can never overflow
/// the vendor's 128-char cap (the old 128-budget-plus-prefix produced 136).
pub fn sanitize_aws_batch_name(name: &str) -> String {
    sanitize_cloud_name(
        name,
        AWS_JOB_NAME_MAX - AWS_JOB_NAME_PREFIX.len(),
        false,
        "_",
    )
}

/// [`sanitize_aws_batch_name`] for the longer array-job prefix.
fn sanitize_aws_batch_array_name(name: &str) -> String {
    sanitize_cloud_name(
        name,
        AWS_JOB_NAME_MAX - AWS_ARRAY_JOB_NAME_PREFIX.len(),
        false,
        "_",
    )
}

/// Scheduler handle of one array element, derived from the id
/// `submit-job` echoed for the array submission.
///
/// AWS Batch gives each array child a real job id of the form
/// `<parentJobId>:<arrayIndex>` with a ZERO-based array index, while the
/// driver's element ordinal is 1-based (element 1 runs
/// `AWS_BATCH_JOB_ARRAY_INDEX=0`, matching the generated
/// `cmd.$((index + 1)).sh` dispatch). The child id is directly
/// describable, so the driver can settle each element on its own status
/// instead of expanding the parent's verdict (issue #1132 / ledger
/// RB-06).
pub fn array_child_id(base: &str, ordinal: usize) -> String {
    format!("{base}:{}", ordinal.saturating_sub(1))
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
    let job_name = format!(
        "{AWS_JOB_NAME_PREFIX}{}",
        sanitize_aws_batch_name(&rule.name)
    );

    let job_queue = cluster_config
        .queue
        .as_deref()
        .unwrap_or(AWS_DEFAULT_JOB_QUEUE);

    let job_definition = cluster_config
        .account
        .as_deref()
        .unwrap_or(AWS_DEFAULT_JOB_DEFINITION);

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
    let job_name = format!(
        "{AWS_ARRAY_JOB_NAME_PREFIX}{}",
        sanitize_aws_batch_array_name(&rule.name)
    );

    let job_queue = cluster_config
        .queue
        .as_deref()
        .unwrap_or(AWS_DEFAULT_JOB_QUEUE);

    let job_definition = cluster_config
        .account
        .as_deref()
        .unwrap_or(AWS_DEFAULT_JOB_DEFINITION);

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
            name_scope: None,
            volume: None,
        }
    }

    #[test]
    fn test_sanitize_aws_batch_name() {
        assert_eq!(sanitize_aws_batch_name("fastqc.sample1"), "fastqc-sample1");
        assert_eq!(sanitize_aws_batch_name("align_bwa-mem"), "align_bwa-mem");
    }

    /// SubmitJob caps jobName at 128 chars. The engine prefix counts
    /// against the cap: the old 128-budget-plus-prefix produced 136 chars
    /// and the service rejected the submission.
    #[test]
    fn generated_job_names_fit_the_128_char_cap() {
        let rule = Rule {
            name: "a".repeat(300),
            ..Default::default()
        };
        let single: Value = serde_json::from_str(
            &generate_aws_batch_job_spec(&rule, "true", &test_config()).unwrap(),
        )
        .unwrap();
        let array: Value = serde_json::from_str(
            &generate_aws_batch_array_spec(&rule, "runs/cmds", 2, &test_config()).unwrap(),
        )
        .unwrap();
        for name in [
            single["jobName"].as_str().unwrap(),
            array["jobName"].as_str().unwrap(),
        ] {
            assert!(
                name.len() <= AWS_JOB_NAME_MAX,
                "AWS caps jobName at {} chars, got {} ({name})",
                AWS_JOB_NAME_MAX,
                name.len()
            );
            assert!(name.starts_with("oxoflow-"), "engine prefix kept: {name}");
            assert!(
                name.chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric()),
                "first char must be alphanumeric: {name}"
            );
        }
    }

    /// A profile without queue/account submits to the engine defaults the
    /// raw `cluster status` listing also queries (one constant, two call
    /// sites).
    #[test]
    fn default_queue_and_job_definition_are_shared_with_the_lister() {
        let config = ClusterJobConfig {
            backend: ClusterBackend::AwsBatch,
            queue: None,
            account: None,
            walltime: None,
            extra_args: Vec::new(),
            name_scope: None,
            volume: None,
        };
        let rule = Rule {
            name: "x".into(),
            ..Default::default()
        };
        let val: Value =
            serde_json::from_str(&generate_aws_batch_job_spec(&rule, "true", &config).unwrap())
                .unwrap();
        assert_eq!(val["jobQueue"], AWS_DEFAULT_JOB_QUEUE);
        assert_eq!(val["jobDefinition"], AWS_DEFAULT_JOB_DEFINITION);
        // The raw listing must query the SAME default queue: the old
        // empty-invocation behaviour and a second hard-coded literal both
        // fail here.
        let invocations =
            crate::backend::cluster::my_jobs_invocations(&ClusterBackend::AwsBatch, None);
        assert!(
            !invocations.is_empty(),
            "the raw listing must be answerable"
        );
        for (_, args) in &invocations {
            let at = args
                .iter()
                .position(|arg| arg == "--job-queue")
                .unwrap_or_else(|| panic!("no --job-queue in {args:?}"));
            assert_eq!(
                args[at + 1],
                AWS_DEFAULT_JOB_QUEUE,
                "the listing must reuse the submit default queue: {args:?}"
            );
        }
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

    /// Ledger RB-06: array elements are addressed by their real AWS Batch
    /// child ids, whose array index is zero-based while the driver's
    /// element ordinal is one-based.
    #[test]
    fn array_child_ids_are_zero_based_real_job_ids() {
        assert_eq!(array_child_id("abc-123", 1), "abc-123:0");
        assert_eq!(array_child_id("abc-123", 4), "abc-123:3");
        // Defensive: ordinal 0 cannot underflow into a bogus negative index.
        assert_eq!(array_child_id("abc-123", 0), "abc-123:0");
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

    /// Issue #1171: a submit-host ambient credential must never reach an AWS
    /// Batch job body. The SubmitJob payload is built from the rule and the
    /// rendered command only; the container's environment is the job
    /// definition's plus image `ENV`, never the submit host's. The probe runs
    /// in a child process that carries sentinel credentials (the crate
    /// forbids `std::env::set_var`), so a future change that forwards the
    /// submit host's environment or inlines its values fails here.
    #[test]
    fn aws_batch_job_body_carries_no_submit_host_environment() {
        crate::backend::assert_cloud_body_ignores_submit_host_env("aws-batch");
    }

    /// Sentinel regression (ledger RB-12): once the cluster plan routes the
    /// sensitive literal out of `shell_cmd`, no AWS Batch job body carries it —
    /// only the `OXO_FLOW_SENSITIVE_<n>` reference the job-definition secret
    /// (or `environment` override) resolves at container start.
    #[test]
    fn test_sensitive_value_never_in_aws_batch_body() {
        let sentinel = "SENTINEL_SECRET_VALUE_C3";
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

        // Single job: the routed reference replaces the literal in the body.
        let single = generate_aws_batch_job_spec(&rule, &routed, &test_config()).unwrap();
        assert!(
            !single.contains(sentinel),
            "sentinel leaked into the AWS Batch body: {single}"
        );
        assert!(
            single.contains("OXO_FLOW_SENSITIVE_0"),
            "the routed reference must survive into the body: {single}"
        );

        // Array job: the body only dispatches to per-index command files, so
        // it never carries either the literal OR the reference — the
        // reference lives in the (private) cmd.N.sh files and resolves from
        // the job definition's `containerProperties.secrets`.
        let array = generate_aws_batch_array_spec(&rule, "runs/cmds", 4, &test_config()).unwrap();
        assert!(
            !array.contains(sentinel),
            "sentinel leaked into the AWS Batch array body: {array}"
        );
    }
}
