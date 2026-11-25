//! Tencent Cloud BatchCompute (批量计算) job specification generator.
//!
//! Maps ScheduledRule specifications onto `tccli batch SubmitJob`
//! `--cli-input-json` payloads (API version `2017-03-12`), with an inline
//! MANAGED compute environment, CVM instance sizing, and array jobs via
//! `Task.TaskInstanceNum`. Submission/polling goes through the shared
//! `ClusterExecutor` (`backend/cluster.rs`).
//!
//! # Profile-key mapping
//!
//! The shared `ClusterJobConfig` keys carry backend-specific payloads, the
//! same way `queue` doubles as the AWS job queue and `--location` rides it
//! for GCP:
//!
//! - `queue` → `Placement.Zone` (**required**, e.g. `ap-shanghai-5` —
//!   SubmitJob's Placement is a required structure and tccli fails without
//!   a zone).
//! - `account` → `ComputeEnv.EnvData.ImageId` (**required** — the CVM image
//!   the batch nodes boot; find ids with `tccli cvm DescribeImages`).
//! - `TENCENTBATCH_INSTANCE_TYPE` env → `ComputeEnv.EnvData.InstanceType`
//!   (default `S5.MEDIUM4`).
//!
//! # Sensitive config values (issue #1120 / ledger RB-12)
//!
//! The cluster plan routes `sensitive = true` values out of `shell_cmd`
//! into `OXO_FLOW_SENSITIVE_<n>` references before this generator runs, and
//! the SubmitJob `Task.EnvVars` structure is PLAINTEXT only — there is no
//! secret-reference field (Tencent's secret channel is COS-mount
//! `Authentications`, an unrelated mechanism). As with AWS Batch, the
//! operator must provide the referenced values through the CVM image's own
//! configuration; this generator never receives or inlines them.
//!
//! # Logs
//!
//! `RedirectInfo` targets must be COS paths; when absent, task logs are
//! collected by the Batch agent and read back through `DescribeTaskLogs`
//! (base64 `StdoutLog`/`StderrLog` — raw per the API reference, a data URI
//! in its example; `ClusterExecutor::logs` accepts both), which the
//! executor's `logs()` decodes.

use super::sanitize_cloud_name;
use crate::cluster::ClusterJobConfig;
use crate::error::{OxoFlowError, Result};
use crate::rule::Rule;
use serde_json::json;

/// Tencent `Job.JobName` length limit (api.json: "字符串长度限制60").
pub const TENCENT_JOB_NAME_MAX: usize = 60;

/// Engine prefix on every Tencent job name; counted against the 60-char
/// cap so prefixing can never overflow it.
pub const TENCENT_JOB_NAME_PREFIX: &str = "oxoflow-";

/// Sanitise a rule name into the portion of a Tencent job name that follows
/// [`TENCENT_JOB_NAME_PREFIX`] — the same string also serves as the task
/// name (one task per rule), for which the API states no separate cap.
pub fn sanitize_tencent_batch_name(name: &str) -> String {
    sanitize_cloud_name(
        name,
        TENCENT_JOB_NAME_MAX - TENCENT_JOB_NAME_PREFIX.len(),
        true,
        "-",
    )
}

/// Engine bookkeeping handle of one array element: `{jobId}:{index}` with a
/// ZERO-based instance index (`TaskInstanceIndex` in `DescribeTask`), while
/// the driver's element ordinal is 1-based (element 1 runs the generated
/// `cmd.1.sh`, reading `BATCH_TASK_INSTANCE_INDEX=0`).
///
/// The handle is engine-owned: Tencent addresses an instance by
/// JobId + TaskName + index, not by this string. `ClusterExecutor::poll`
/// reads the base job's `JobState` and applies it to every element, and the
/// driver records that aggregate provenance in `skip_reason`; per-instance
/// settlement through `DescribeTask` is the RB-06 residual for this backend.
pub fn array_child_id(base: &str, ordinal: usize) -> String {
    format!("{base}:{}", ordinal.saturating_sub(1))
}

/// The zero-based instance-index env var Tencent Batch injects into task
/// commands — the array dispatch line reads it to pick its `cmd.$N.sh`
/// (the same pattern as `AWS_BATCH_JOB_ARRAY_INDEX` / `BATCH_TASK_INDEX`).
pub const INSTANCE_INDEX_ENV: &str = "BATCH_TASK_INSTANCE_INDEX";

/// Derive the tccli `--region` from the placement zone: `ap-shanghai-5` →
/// `ap-shanghai`. tccli hard-fails a SubmitJob whose zone's region differs
/// from its region (InvalidParameterValue.InvalidZoneMismatchRegion), so
/// the engine derives it instead of trusting a separately-set env var.
pub fn region_from_zone(zone: &str) -> &str {
    match zone.rsplit_once('-') {
        Some((region, suffix))
            if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) =>
        {
            region
        }
        _ => zone,
    }
}

fn zone(config: &ClusterJobConfig) -> Result<&str> {
    config.queue.as_deref().ok_or_else(|| OxoFlowError::Config {
        message: "tencent-batch requires queue = the availability zone (e.g. \
                      ap-shanghai-5) — SubmitJob's Placement.Zone is a required field"
            .into(),
    })
}

fn image_id(config: &ClusterJobConfig) -> Result<&str> {
    config
        .account
        .as_deref()
        .ok_or_else(|| OxoFlowError::Config {
            message: "tencent-batch requires account = the CVM ImageId the batch nodes boot \
                      (list ids with `tccli cvm DescribeImages`)"
                .into(),
        })
}

fn instance_type() -> String {
    // Reading (not writing) an env var is safe in edition 2024.
    std::env::var("TENCENTBATCH_INSTANCE_TYPE").unwrap_or_else(|_| "S5.MEDIUM4".to_string())
}

fn task_job_payload(
    rule: &Rule,
    command: &str,
    count: usize,
    cluster_config: &ClusterJobConfig,
) -> Result<serde_json::Value> {
    let zone = zone(cluster_config)?;
    let job_name = format!(
        "{TENCENT_JOB_NAME_PREFIX}{}",
        sanitize_tencent_batch_name(&rule.name)
    );
    // TaskName is referenced by Dependences and DescribeTask — one task per
    // oxo-flow rule (per-instance fan-out rides TaskInstanceNum). The API
    // states no separate cap; the job-name budget is a safe bound.
    let task_name = sanitize_tencent_batch_name(&rule.name);

    // Task.ComputeEnv and Task.EnvId are mutually exclusive. The inline
    // MANAGED env is the default, but some accounts fail anonymous env
    // creation while a pre-created named env works (live-verified
    // 2026-11-25) — `TENCENTBATCH_ENV_ID` targets a named environment
    // created with `tccli batch CreateComputeEnv` instead.
    let mut task = json!({
        "TaskName": task_name,
        "TaskInstanceNum": count,
        "Application": {
            "DeliveryForm": "LOCAL",
            "Command": command,
        },
        "MaxRetryCount": 0,
    });
    match std::env::var("TENCENTBATCH_ENV_ID") {
        Ok(env_id) if !env_id.trim().is_empty() => {
            task["EnvId"] = json!(env_id);
        }
        _ => {
            let image = image_id(cluster_config)?;
            task["ComputeEnv"] = json!({
                "EnvType": "MANAGED",
                "EnvData": {
                    "InstanceType": instance_type(),
                    "ImageId": image,
                }
            });
        }
    }

    Ok(json!({
        "Placement": { "Zone": zone },
        "Job": {
            "JobName": job_name,
            "Tasks": [task],
        }
    }))
}

/// Generate a Tencent Batch SubmitJob `--cli-input-json` payload for a
/// single rule.
pub fn generate_tencent_batch_job_spec(
    rule: &Rule,
    shell_cmd: &str,
    cluster_config: &ClusterJobConfig,
) -> Result<String> {
    let payload = task_job_payload(rule, shell_cmd, 1, cluster_config)?;
    Ok(serde_json::to_string_pretty(&payload)?)
}

/// Generate a Tencent Batch array-job payload: the SAME single task with
/// `TaskInstanceNum = count` — BatchCompute's array knob — dispatching
/// through the zero-based `BATCH_TASK_INSTANCE_INDEX`.
pub fn generate_tencent_batch_array_spec(
    rule: &Rule,
    cmd_dir: &str,
    count: usize,
    cluster_config: &ClusterJobConfig,
) -> Result<String> {
    let dispatch_cmd = format!(
        "INDEX=$(({} + 1)); sh \"{cmd_dir}/cmd.${{INDEX}}.sh\"",
        INSTANCE_INDEX_ENV
    );
    let payload = task_job_payload(rule, &dispatch_cmd, count, cluster_config)?;
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
            backend: ClusterBackend::TencentBatch,
            queue: Some("ap-shanghai-5".into()),
            account: Some("img-ubuntu22".into()),
            walltime: None,
            extra_args: Vec::new(),
            name_scope: None,
            volume: None,
        }
    }

    /// Live finding (2026-11-25): anonymous job-level env creation failed
    /// on a working account while a named env (`CreateComputeEnv`) ran the
    /// same payload to SUCCEED — the generator must support an EnvId
    /// reference (`TENCENTBATCH_ENV_ID`), which drops ComputeEnv entirely.
    #[test]
    fn env_id_reference_replaces_the_inline_compute_env() {
        // set_var is unsafe in edition 2024 — probe via the child-process
        // pattern like the ambient-leak test.
        crate::backend::assert_tencent_env_id_probe();
        // Without the var, ComputeEnv is present and EnvId absent.
        let rule = Rule {
            name: "x".into(),
            ..Default::default()
        };
        let plain = generate_tencent_batch_job_spec(&rule, "true", &test_config()).unwrap();
        assert!(plain.contains("ComputeEnv") && !plain.contains("\"EnvId\""));
    }

    #[test]
    fn region_is_derived_from_the_zone() {
        use super::region_from_zone;
        assert_eq!(region_from_zone("ap-shanghai-5"), "ap-shanghai");
        assert_eq!(region_from_zone("ap-guangzhou-2"), "ap-guangzhou");
        assert_eq!(region_from_zone("ap-shanghai"), "ap-shanghai");
    }

    #[test]
    fn test_sanitize_tencent_batch_name() {
        assert_eq!(
            sanitize_tencent_batch_name("fastqc.sample1"),
            "fastqc-sample1"
        );
    }

    /// `Job.JobName` is capped at 60 chars. The engine prefix counts
    /// against the cap — the old 60-budget-plus-prefix produced 68.
    #[test]
    fn generated_job_name_fits_the_60_char_limit() {
        let rule = Rule {
            name: "b".repeat(300),
            ..Default::default()
        };
        let spec = generate_tencent_batch_job_spec(&rule, "true", &test_config())
            .expect("spec generation must succeed");
        let val: Value = serde_json::from_str(&spec).unwrap();
        let name = val["Job"]["JobName"].as_str().unwrap();
        assert!(
            name.len() <= TENCENT_JOB_NAME_MAX,
            "Tencent caps JobName at {} chars, got {} ({name})",
            TENCENT_JOB_NAME_MAX,
            name.len()
        );
        assert!(name.starts_with(TENCENT_JOB_NAME_PREFIX), "{name}");
        assert!(
            val["Job"]["Tasks"][0]["TaskName"].as_str().unwrap().len() <= TENCENT_JOB_NAME_MAX,
            "the task name shares the same bound"
        );
    }

    #[test]
    fn test_generate_tencent_batch_job_spec() {
        let rule = Rule {
            name: "star_align".into(),
            ..Default::default()
        };
        let spec = generate_tencent_batch_job_spec(&rule, "STAR --genomeDir ref", &test_config())
            .expect("spec generation must succeed");
        let val: Value = serde_json::from_str(&spec).expect("spec must be valid json");

        assert_eq!(val["Placement"]["Zone"], "ap-shanghai-5");
        assert_eq!(val["Job"]["JobName"], "oxoflow-star-align");
        let task = &val["Job"]["Tasks"][0];
        assert_eq!(task["TaskName"], "star-align");
        assert_eq!(task["TaskInstanceNum"], 1);
        assert_eq!(task["Application"]["DeliveryForm"], "LOCAL");
        assert_eq!(task["Application"]["Command"], "STAR --genomeDir ref");
        assert_eq!(task["ComputeEnv"]["EnvType"], "MANAGED");
        assert_eq!(task["ComputeEnv"]["EnvData"]["ImageId"], "img-ubuntu22");
    }

    /// Missing zone/image are named at generation time — never a payload
    /// that fails server-side after money is spent.
    #[test]
    fn missing_zone_and_image_are_named_at_generation() {
        let rule = Rule {
            name: "a".into(),
            ..Default::default()
        };
        let bare = ClusterJobConfig {
            backend: ClusterBackend::TencentBatch,
            queue: None,
            account: None,
            walltime: None,
            extra_args: Vec::new(),
            name_scope: None,
            volume: None,
        };
        let err = generate_tencent_batch_job_spec(&rule, "true", &bare).unwrap_err();
        assert!(err.to_string().contains("availability zone"), "{err}");
        let err = generate_tencent_batch_job_spec(
            &rule,
            "true",
            &ClusterJobConfig {
                backend: ClusterBackend::TencentBatch,
                queue: Some("ap-shanghai-5".into()),
                account: None,
                walltime: None,
                extra_args: Vec::new(),
                name_scope: None,
                volume: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("ImageId"), "{err}");
    }

    #[test]
    fn test_generate_tencent_batch_array_spec() {
        let rule = Rule {
            name: "mutect2".into(),
            ..Default::default()
        };
        let spec = generate_tencent_batch_array_spec(&rule, "runs/cmds", 10, &test_config())
            .expect("spec generation must succeed");
        let val: Value = serde_json::from_str(&spec).expect("spec must be valid json");

        // The array knob is TaskInstanceNum on the SAME task (no
        // arrayProperties structure in this API).
        assert_eq!(val["Job"]["Tasks"][0]["TaskInstanceNum"], 10);
        let cmd = val["Job"]["Tasks"][0]["Application"]["Command"]
            .as_str()
            .unwrap();
        assert!(cmd.contains(INSTANCE_INDEX_ENV), "{cmd}");
        assert!(cmd.contains("cmd.${INDEX}.sh"), "{cmd}");
    }

    /// Ledger RB-06: array elements are addressed by their zero-based
    /// TaskInstanceIndex while the driver's ordinal is one-based.
    #[test]
    fn array_child_ids_are_zero_based() {
        assert_eq!(array_child_id("job-abc", 1), "job-abc:0");
        assert_eq!(array_child_id("job-abc", 4), "job-abc:3");
        assert_eq!(array_child_id("job-abc", 0), "job-abc:0");
    }

    /// Issue #1171: no submit-host ambient credential or sentinel secret
    /// may reach a Tencent Batch job body — the payload is built from the
    /// rule and the rendered command only.
    /// The probe child started by
    /// [`crate::backend::assert_tencent_env_id_probe`].
    #[test]
    #[ignore]
    fn tencent_env_id_probe_child() {
        let env_id = std::env::var("TENCENTBATCH_ENV_ID")
            .expect("the probe child must carry TENCENTBATCH_ENV_ID");
        let rule = Rule {
            name: "x".into(),
            ..Default::default()
        };
        let spec = generate_tencent_batch_job_spec(&rule, "true", &test_config())
            .expect("spec must render");
        assert!(
            spec.contains(&format!("\"EnvId\": \"{env_id}\"")),
            "the task must reference the named env: {spec}"
        );
        assert!(
            !spec.contains("ComputeEnv"),
            "an EnvId reference must drop the inline ComputeEnv: {spec}"
        );
        println!("TENCENT_ENV_ID_PROBE_OK");
    }

    #[test]
    fn test_sensitive_value_never_in_tencent_batch_body() {
        let sentinel = "SENTINEL_SECRET_VALUE_T9";
        let (routed, _env) = crate::executor::process::route_sensitive_values_through_env(
            &format!("tool --api-key {sentinel} --run"),
            &[sentinel.to_string()],
        );
        let rule = Rule {
            name: "call_variants".into(),
            ..Default::default()
        };
        let single = generate_tencent_batch_job_spec(&rule, &routed, &test_config()).unwrap();
        assert!(
            !single.contains(sentinel),
            "sentinel leaked into the Tencent Batch body: {single}"
        );
        let array =
            generate_tencent_batch_array_spec(&rule, "runs/cmds", 4, &test_config()).unwrap();
        assert!(
            !array.contains(sentinel),
            "sentinel leaked into the Tencent Batch array body: {array}"
        );
    }
}
