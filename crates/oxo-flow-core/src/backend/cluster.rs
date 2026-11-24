//! Cluster (SLURM/PBS/SGE/LSF) executor + shared scheduler-output parsing.
//!
//! Directive generation lives in `crate::cluster` and stays as-is (issue #74);
//! this module is the submission/tracking layer above it. Job-id and
//! status-line parsing is shared (issue #74 comment 5): tracking and
//! array-index mapping need the same logic.

use super::{BackendJobStatus, ScheduledRule, TerminalRecord};
use crate::cluster::{
    ClusterBackend, ClusterJobConfig, generate_array_submit_script_with_secrets,
    generate_submit_script_with_secrets,
};
use crate::error::{OxoFlowError, Result};
use chrono::Utc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Parse the scheduler-assigned job id from a submission's output.
///
/// SLURM submissions use `--parsable` (bare id on stdout) with a sentence
/// fallback; PBS prints a bare id; SGE/LSF print sentences (issue #74
/// phase-1 item 1 — the old wrapper script chained `Submitted batch job N`
/// into `--dependency`, which silently broke).
pub fn parse_job_id(backend: &ClusterBackend, stdout: &str, stderr: &str) -> Result<String> {
    let out = stdout.trim();
    match backend {
        ClusterBackend::Slurm => {
            if !out.is_empty() && out.chars().all(|c| c.is_ascii_digit()) {
                return Ok(out.to_string()); // --parsable
            }
            parse_with_regex(r"Submitted batch job (\d+)", stderr.trim())
        }
        ClusterBackend::Pbs => {
            if !out.is_empty() && !out.contains(char::is_whitespace) {
                return Ok(out.to_string());
            }
            Err(unparseable("PBS", out, stderr.trim()))
        }
        ClusterBackend::Sge => parse_with_regex(r"Your job(?:-array)? (\d+)", out),
        ClusterBackend::Lsf => parse_with_regex(r"Job <(\d+)> is submitted", out),
        ClusterBackend::Kubernetes => {
            if !out.is_empty() {
                if let Some(name) = out
                    .strip_prefix("job.batch/")
                    .and_then(|s| s.strip_suffix(" created"))
                {
                    return Ok(name.trim().to_string());
                }
                return Ok(out.to_string());
            }
            Err(unparseable("Kubernetes", out, stderr.trim()))
        }
        ClusterBackend::AwsBatch => {
            if out.is_empty() {
                return Err(unparseable("AWS Batch", out, stderr.trim()));
            }
            // `--query jobId --output text` returns the bare id, but a
            // wrapped/older CLI may ignore the query and print the whole
            // SubmitJob response. Key on the id it carries instead of
            // returning the JSON document as a job id (which would also
            // corrupt array child handles `{id}:{n}`).
            if let Ok(doc) = serde_json::from_str::<serde_json::Value>(out)
                && let Some(id) = doc.get("jobId").and_then(|v| v.as_str())
                && !id.is_empty()
            {
                return Ok(id.to_string());
            }
            Ok(out.to_string())
        }
        ClusterBackend::GcpBatch => {
            if !out.is_empty() {
                return Ok(out.to_string());
            }
            Err(unparseable("GCP Batch", out, stderr.trim()))
        }
    }
}

fn parse_with_regex(pattern: &str, text: &str) -> Result<String> {
    let re = regex::Regex::new(pattern).expect("static regex compiles");
    re.captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| unparseable("job id", text, ""))
}

/// Mint a GCP Batch job id from a rule/job directory name. GCP caps ids
/// at 63 chars (`^[a-z]([a-z0-9-]{0,61}[a-z0-9])?$`): "oxoflow-" (8) +
/// "-" (1) + millis (13) leaves 41 for the name — clamp the sanitized
/// dir or long rule names fail gcloud's id validation on every submit.
fn mint_gcp_job_id(dir: &str, millis: i64) -> String {
    let name = super::gcp_batch::sanitize_gcp_batch_name(dir);
    format!("oxoflow-{}-{}", &name[..name.len().min(41)], millis)
}

fn unparseable(what: &str, stdout: &str, stderr: &str) -> OxoFlowError {
    OxoFlowError::Config {
        message: format!("cannot parse {what} from '{stdout}' / '{stderr}'"),
    }
}

/// SGE state word → backend status. One table for all three read paths
/// (live-queue table rows, `qstat -j` positional pairing, the driver poll) —
/// the hand-copied versions had already drifted apart (`dr` was Completed in
/// the table but Unknown in `cluster status`; `Eqw` Failed in two of three).
fn sge_state(state: &str) -> BackendJobStatus {
    match state {
        "r" | "t" => BackendJobStatus::Running,
        "qw" | "hqw" | "h" | "Rq" | "P" => BackendJobStatus::Pending,
        // Deletion states: the job left the queue (d = deleting; dr/dt/dR =
        // deleting after running). The accounting probe refines the verdict
        // when reachable.
        "d" | "dr" | "dt" | "dR" => BackendJobStatus::Completed,
        "Eqw" | "E" => BackendJobStatus::Failed,
        _ => BackendJobStatus::Unknown,
    }
}

/// Parse one scheduler status line into `(job id, status)`; `None` for
/// unrecognised shapes. SLURM lines use the driver's `%i|%T` format (array
/// elements like `12345_7` are preserved verbatim); PBS/LSF lines are the
/// default `qstat`/`bjobs` table rows.
pub fn parse_status_line(
    backend: &ClusterBackend,
    line: &str,
) -> Option<(String, BackendJobStatus)> {
    match backend {
        ClusterBackend::Slurm => {
            let (id, state) = line.split_once('|')?;
            let status = match state {
                // Long forms (`%T` — what the engine's squeue calls emit)…
                "PENDING" | "CONFIGURING" => BackendJobStatus::Pending,
                "RUNNING" | "COMPLETING" => BackendJobStatus::Running,
                "COMPLETED" => BackendJobStatus::Completed,
                "FAILED" | "TIMEOUT" | "OUT_OF_MEMORY" | "NODE_FAIL" | "BOOT_FAIL" => {
                    BackendJobStatus::Failed
                }
                "CANCELLED" | "PREEMPTED" => BackendJobStatus::Cancelled,
                // …and compact forms (`%t`): `squeue -o %t` prints `R`,
                // `PD`, `CG`, … Without these every live job parses as
                // `Unknown` (seen live: RUNNING jobs reported as
                // "finished, or an unknown id" by `cluster status`).
                "PD" => BackendJobStatus::Pending,
                "R" | "CG" => BackendJobStatus::Running,
                "CD" => BackendJobStatus::Completed,
                "F" | "NF" | "TO" | "OOM" | "BF" => BackendJobStatus::Failed,
                "CA" | "PR" => BackendJobStatus::Cancelled,
                _ => BackendJobStatus::Unknown,
            };
            Some((id.to_string(), status))
        }
        ClusterBackend::Pbs => {
            // "Job id  Name  User  Time Use  S  Queue"
            let fields: Vec<&str> = line.split_whitespace().collect();
            let id = (*fields.first()?).to_string();
            let status = match *fields.get(4)? {
                "Q" | "H" | "W" => BackendJobStatus::Pending,
                // "B" is a live array container (elements staged/running).
                "R" | "B" => BackendJobStatus::Running,
                "C" => BackendJobStatus::Completed,
                // F (finished) says nothing about the exit code at table
                // level — leave Unknown so the accounting probe settles it
                // with the real exit code.
                "F" => BackendJobStatus::Unknown,
                _ => BackendJobStatus::Unknown,
            };
            Some((id, status))
        }
        ClusterBackend::Lsf => {
            // "JOBID  USER  STAT  QUEUE  ..." — STAT follows IBM's bjobs
            // states. Suspend states split by WHEN the job was held:
            // USUSP held it while still pending (never started), while
            // PSUSP/SSUSP held a started job; LSF 10.x merged all three
            // into SUSP, which in practice means "started, then held".
            let fields: Vec<&str> = line.split_whitespace().collect();
            let id = (*fields.first()?).to_string();
            let status = match *fields.get(2)? {
                "PEND" | "USUSP" => BackendJobStatus::Pending,
                "RUN" | "PSUSP" | "SSUSP" | "SUSP" => BackendJobStatus::Running,
                "DONE" => BackendJobStatus::Completed,
                "EXIT" | "ZOMBI" => BackendJobStatus::Failed,
                _ => BackendJobStatus::Unknown,
            };
            Some((id, status))
        }
        ClusterBackend::Sge => {
            // The bare `qstat` table has whitespace columns with the state in
            // field 4 ("job-ID prior qname ... state"), matching the shape
            // PBS/LSF rows take above.
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() > 4 && fields[0].chars().all(|c| c.is_ascii_digit()) {
                return Some((fields[0].to_string(), sge_state(fields[4])));
            }
            // qstat -j output pairs "job_number: N" with "state: r"; the
            // executor collects both lines — here we extract the number only.
            Some((
                line.strip_prefix("job_number:")?.trim().to_string(),
                BackendJobStatus::Unknown,
            ))
        }
        ClusterBackend::Kubernetes | ClusterBackend::AwsBatch | ClusterBackend::GcpBatch => {
            // Two real shapes: pipe rows from the jsonpath/query formats
            // this module asks for, and tab rows from gcloud's
            // `--format=value(name,state)`. `split_once` handles the pipe;
            // whitespace rows read id-first (gcloud prints name then
            // state).
            let (id, state) = match line.split_once('|') {
                Some(pair) => pair,
                None => {
                    let mut fields = line.split_whitespace();
                    let id = fields.next()?;
                    (id, fields.next()?)
                }
            };
            let status = match state.trim().to_uppercase().as_str() {
                "PENDING" | "SUBMITTED" | "RUNNABLE" | "QUEUED" => BackendJobStatus::Pending,
                "RUNNING" | "STARTING" => BackendJobStatus::Running,
                "COMPLETE" | "COMPLETED" | "SUCCEEDED" => BackendJobStatus::Completed,
                "FAILED" | "DELETION_IN_PROGRESS" => BackendJobStatus::Failed,
                "CANCELLED" => BackendJobStatus::Cancelled,
                _ => BackendJobStatus::Unknown,
            };
            let mut id = id.trim();
            // gcloud's --format=value(name,…) prints the FULLY QUALIFIED
            // resource name (projects/p/locations/l/jobs/<id>) on both
            // describe and list — users (and this module's own submit path)
            // always speak the bare id, so slice the last segment or a
            // by-id `cluster status` query never matches.
            if matches!(backend, ClusterBackend::GcpBatch)
                && let Some(pos) = id.rfind("/jobs/")
            {
                id = &id[pos + "/jobs/".len()..];
            }
            Some((id.to_string(), status))
        }
    }
}

/// Split a Kubernetes array-element handle (`{jobName}_{ordinal}`) into the
/// base Job name and the ZERO-based completion index it names.
///
/// The base is what the driver submitted (`array_element_id` composes the
/// ordinal, 1-based, onto the base), and Kubernetes Job names are sanitized
/// to lowercase alphanumerics and dashes — so the last underscore is
/// unambiguous. `None` means the id is a plain Job name.
fn k8s_element_index(element_id: &str) -> Option<(&str, u32)> {
    let (base, ordinal) = element_id.rsplit_once('_')?;
    if base.is_empty() || ordinal.is_empty() || !ordinal.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let ordinal: u32 = ordinal.parse().ok()?;
    Some((base, ordinal.checked_sub(1)?))
}

/// Split a GCP Batch array-element handle (`{job}:{taskIndex}`) into the
/// base job id and the ZERO-based task index. `None` means the id is a
/// plain job id.
fn gcp_element_index(element_id: &str) -> Option<(&str, u32)> {
    let (base, index) = element_id.rsplit_once(':')?;
    if base.is_empty() || index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((base, index.parse().ok()?))
}

/// AWS Batch `DescribeJobs` accepts at most 100 job ids per call — a longer
/// `--jobs` list is rejected outright as a validation error, not answered
/// partially. The driver can legitimately exceed that cap: `max_submitted`
/// has no 100 ceiling and every array element carries its own child id, so
/// a poll of 101+ in-flight elements must split the list and merge the
/// per-call answers (review of #1222, ledger RB-06).
pub const AWS_DESCRIBE_JOBS_MAX_IDS: usize = 100;

/// The scheduler invocations that report `job_ids`, in the form each
/// scheduler actually accepts: `(program, arguments)`, one entry per call.
///
/// SLURM's `-j` takes ONE comma-separated list — ids as separate arguments
/// answer "Invalid job id specified" on a real cluster (the driver's own
/// poller always joined them; the standalone `cluster status` command did
/// not). PBS and LSF take ids positionally, SGE wants one `-j` per job.
/// SLURM is asked for the same `%i|%T` shape the poller reads, so both
/// paths share one parser.
pub fn status_invocations(
    backend: &ClusterBackend,
    job_ids: &[String],
) -> Vec<(&'static str, Vec<String>)> {
    match backend {
        ClusterBackend::Slurm => vec![(
            "squeue",
            vec![
                "-j".to_string(),
                job_ids.join(","),
                "--noheader".to_string(),
                "-o".to_string(),
                "%i|%T".to_string(),
            ],
        )],
        // `-x` includes finished (history) jobs: an OpenPBS by-id listing
        // without it refuses finished array elements outright (exit 35),
        // which would read as "left the queue" mid-flight.
        ClusterBackend::Pbs => {
            let mut args = vec!["-x".to_string()];
            args.extend(job_ids.iter().cloned());
            vec![("qstat", args)]
        }
        ClusterBackend::Lsf => {
            // `-a`: plain bjobs lists only PEND/RUN, so a job that finished
            // between polls would vanish and settlement would depend on a
            // working `bacct` store. With `-a` the DONE/EXIT rows stay
            // visible directly in the poll answer.
            let mut args = vec!["-a".to_string()];
            args.extend(job_ids.iter().cloned());
            vec![("bjobs", args)]
        }
        ClusterBackend::Sge => job_ids
            .iter()
            .map(|id| ("qstat", vec!["-j".to_string(), id.clone()]))
            .collect(),
        ClusterBackend::Kubernetes => job_ids
            .iter()
            .map(|id| {
                (
                    "kubectl",
                    vec![
                        "get".to_string(),
                        "job".to_string(),
                        id.clone(),
                        // `name|conditions` rows — the same pipe shape the
                        // shared parser reads. A job with no conditions
                        // yet yields `name|` (Unknown), which `cluster
                        // status` reports verbatim.
                        "-o".to_string(),
                        "jsonpath={.metadata.name}|{.status.conditions[*].type}".to_string(),
                    ],
                )
            })
            .collect(),
        ClusterBackend::AwsBatch => {
            // `--jobs` is a LIST parameter: each id must be its own argv
            // entry (a single space-joined argument becomes one invalid
            // id). Array elements are real child job ids (`{parent}:{n}`),
            // so this path answers per element too — and the 100-id
            // DescribeJobs cap applies per call, so long id lists become
            // one invocation per chunk.
            job_ids
                .chunks(AWS_DESCRIBE_JOBS_MAX_IDS)
                .map(|group| {
                    let mut args = vec![
                        "batch".to_string(),
                        "describe-jobs".to_string(),
                        "--jobs".to_string(),
                    ];
                    args.extend(group.iter().cloned());
                    // The driver poller reads the same `id|STATE` rows —
                    // one query shape, one parser.
                    args.extend([
                        "--query".to_string(),
                        "jobs[].join(|,[jobId,status])".to_string(),
                        "--output".to_string(),
                        "text".to_string(),
                    ]);
                    ("aws", args)
                })
                .collect()
        }
        ClusterBackend::GcpBatch => job_ids
            .iter()
            .map(|id| {
                (
                    "gcloud",
                    vec![
                        "batch".to_string(),
                        "jobs".to_string(),
                        "describe".to_string(),
                        id.clone(),
                        // `name<TAB>state` — the tab shape the shared
                        // parser reads alongside the pipe form.
                        "--format=value(name,status.state)".to_string(),
                    ],
                )
            })
            .collect(),
    }
}

/// The invocations that list the CURRENT USER's jobs — the no-arguments
/// `cluster status` view. `std::process::Command` performs no shell
/// expansion, so the conventional `-u $USER` is resolved from the
/// environment here; SGE and LSF scope a bare call to the invoking user on
/// their own.
pub fn my_jobs_invocations(backend: &ClusterBackend) -> Vec<(&'static str, Vec<String>)> {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_default();
    match backend {
        ClusterBackend::Slurm => vec![(
            "squeue",
            vec![
                "-u".to_string(),
                user,
                "--noheader".to_string(),
                "-o".to_string(),
                "%i|%T".to_string(),
            ],
        )],
        ClusterBackend::Pbs => vec![("qstat", vec!["-u".to_string(), user])],
        ClusterBackend::Sge => vec![("qstat", Vec::new())],
        ClusterBackend::Lsf => vec![("bjobs", Vec::new())],
        ClusterBackend::Kubernetes => vec![(
            "kubectl",
            vec![
                "get".to_string(),
                "jobs".to_string(),
                "-o".to_string(),
                "jsonpath={range .items[*]}{.metadata.name}|{.status.conditions[*].type}{\"\n\"}{end}"
                    .to_string(),
            ],
        )],
        ClusterBackend::AwsBatch => vec![(
            "aws",
            vec![
                "batch".to_string(),
                "list-jobs".to_string(),
                // list-jobs requires a queue; sites configure it via the
                // AWS CLI defaults. The query keeps the parse shape the
                // same as describe-jobs.
                "--query".to_string(),
                "jobSummaryList[].join(|,[jobId,status])".to_string(),
                "--output".to_string(),
                "text".to_string(),
            ],
        )],
        ClusterBackend::GcpBatch => vec![(
            "gcloud",
            vec![
                "batch".to_string(),
                "jobs".to_string(),
                "list".to_string(),
                "--format=value(name,status.state)".to_string(),
            ],
        )],
    }
}

/// Parse listing output — the scheduler's per-user table — into one
/// `(job id, status)` per LIVE row.
///
/// Unlike [`status_report`] this answers per parsed row: the question here
/// is "what is in my queue right now", so header lines are skipped (a PBS
/// header's first field is the literal `Job id`, not a numeric id) and there
/// is no requested-id list to anchor unknowns against.
pub fn parse_job_listing(
    backend: &ClusterBackend,
    stdout: &str,
) -> Vec<(String, BackendJobStatus)> {
    let is_hpc = !matches!(
        backend,
        ClusterBackend::Kubernetes | ClusterBackend::AwsBatch | ClusterBackend::GcpBatch
    );
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(|line| parse_status_line(backend, line))
        // The numeric-first guard drops HPC table headers ("Job id  Name
        // ..."). Cloud rows come from jsonpath/query formats with no
        // headers, and their ids are alphanumeric — the guard filtered out
        // every cloud listing, so it only applies to HPC backends.
        .filter(|(id, _)| !is_hpc || id.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .collect()
}

/// One or more complete cancel invocations for `job_ids` on `backend`.
///
/// The standalone `cluster cancel` command and [`ClusterExecutor::cancel`]
/// share this mapping so the two can never drift (issue #1158 live campaign:
/// the CLI used to pass the bare job id, which `kubectl` rejected as an
/// unknown command). Scheduler-array-style backends cancel every id in one
/// call; per-job APIs (AWS Batch, GCP Batch) get one invocation each.
pub fn cancel_invocations(
    backend: &ClusterBackend,
    job_ids: &[String],
) -> Vec<(String, Vec<String>)> {
    match backend {
        ClusterBackend::Slurm => vec![("scancel".to_string(), job_ids.to_vec())],
        ClusterBackend::Pbs | ClusterBackend::Sge => {
            vec![("qdel".to_string(), job_ids.to_vec())]
        }
        ClusterBackend::Lsf => vec![("bkill".to_string(), job_ids.to_vec())],
        ClusterBackend::Kubernetes => {
            let mut args = vec!["delete".to_string(), "job".to_string()];
            args.extend(job_ids.iter().cloned());
            vec![("kubectl".to_string(), args)]
        }
        ClusterBackend::AwsBatch => job_ids
            .iter()
            .map(|id| {
                (
                    "aws".to_string(),
                    vec![
                        "batch".to_string(),
                        "cancel-job".to_string(),
                        "--job-id".to_string(),
                        id.clone(),
                        "--reason".to_string(),
                        "cancelled".to_string(),
                    ],
                )
            })
            .collect(),
        ClusterBackend::GcpBatch => job_ids
            .iter()
            .map(|id| {
                (
                    "gcloud".to_string(),
                    vec![
                        "batch".to_string(),
                        "jobs".to_string(),
                        "delete".to_string(),
                        id.clone(),
                        "--quiet".to_string(),
                    ],
                )
            })
            .collect(),
    }
}

/// The command that cancels a job, matching [`super::ExecutorBackend::cancel`]
/// so the standalone `cluster cancel` command and the driver agree.
pub fn cancel_command(backend: &ClusterBackend) -> &'static str {
    match backend {
        ClusterBackend::Slurm => "scancel",
        ClusterBackend::Pbs | ClusterBackend::Sge => "qdel",
        ClusterBackend::Lsf => "bkill",
        ClusterBackend::Kubernetes => "kubectl",
        ClusterBackend::AwsBatch => "aws",
        ClusterBackend::GcpBatch => "gcloud",
    }
}

/// One `(job id, status)` per REQUESTED id, read out of scheduler status
/// output.
///
/// Reporting per request (rather than per parsed row) is what makes the
/// command answerable: a job the scheduler no longer lists — finished and
/// gone from the live queue, or a typo — comes back as
/// [`BackendJobStatus::Unknown`] instead of silently vanishing, and column
/// headers or site wrappers never turn into rows of their own.
pub fn status_report(
    backend: &ClusterBackend,
    job_ids: &[String],
    stdout: &str,
) -> Vec<(String, BackendJobStatus)> {
    let mut found: HashMap<String, BackendJobStatus> = HashMap::new();
    for line in stdout.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some((id, status)) = parse_status_line(backend, line)
            && job_ids.contains(&id)
        {
            found.insert(id, status);
        }
    }
    // SGE's `qstat -j` splits an answer across two lines — the number and
    // the state pair up positionally, not line-by-line.
    if matches!(backend, ClusterBackend::Sge) {
        let mut current: Option<&str> = None;
        for line in stdout.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if let Some(id) = line.strip_prefix("job_number:") {
                current = Some(id.trim());
                found.insert(id.trim().to_string(), BackendJobStatus::Unknown);
            } else if let (Some(id), Some(state)) =
                (current, line.strip_prefix("state:").map(str::trim))
            {
                found.insert(id.to_string(), sge_state(state));
            }
        }
    }
    job_ids
        .iter()
        .map(|id| {
            (
                id.clone(),
                found.get(id).copied().unwrap_or(BackendJobStatus::Unknown),
            )
        })
        .collect()
}

/// Absolute form of `path`, best effort. The scheduler resolves the
/// working-directory directive on the EXEC node, where a relative path
/// would mean somewhere else entirely.
fn absolute_workdir(path: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return resolved;
    }
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    }
}

/// Log paths a rendered script directs its scheduler to open, in directive
/// order: `(output, error)` — either may be absent.
///
/// Parsed from the script TEXT rather than re-derived from the rule: array
/// scripts rewrite the directives to per-element patterns (`%A_%a`, `%I`)
/// and `--extra-arg` lines can add more, so the script itself is the only
/// source of truth for what the scheduler will open.
fn script_log_paths(script: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in script.lines() {
        let line = line.trim_start();
        // The four directive prefixes, with the log path either after '='
        // (SLURM `--output=`) or after a space (PBS/SGE/LSF `-o `/`-e `).
        for (prefix, sep) in [
            ("#SBATCH --output=", '='),
            ("#SBATCH --error=", '='),
            ("#PBS -o ", ' '),
            ("#PBS -e ", ' '),
            ("#$ -o ", ' '),
            ("#$ -e ", ' '),
            ("#BSUB -o ", ' '),
            ("#BSUB -e ", ' '),
        ] {
            if let Some(rest) = line.strip_prefix(prefix) {
                // The path runs to the next separator (or end of line) —
                // LSF appends nothing, but extra args on the same directive
                // line must not become part of the path.
                let raw = match rest.find(sep) {
                    Some(idx) => &rest[..idx],
                    None => rest,
                };
                // LSF's `-o` accepts %J-style patterns; whitespace never
                // belongs in the path itself.
                let raw = raw.split_whitespace().next().unwrap_or(raw);
                if !raw.is_empty() {
                    paths.push(PathBuf::from(raw));
                }
            }
        }
    }
    paths
}

/// The array index range a SLURM script declares, parsed from the script
/// TEXT (same philosophy as [`script_log_paths`]: `--extra-arg` lines can
/// add directives the rule never mentioned, so the rendered script is the
/// only source of truth).
///
/// Accepts only one straight `#SBATCH --array=START-END` directive at
/// column 0, with an optional `%throttle` — a throttle bounds concurrency,
/// not the index set, so it does not affect element-wise pairing. Every
/// other shape (comma lists, steps, repeated directives, the short `-a`
/// spelling, indented lines) returns `None`: an unparsed range must fall
/// back to the ready-batch dependency, never guess.
fn parse_array_range(script: &str) -> Option<(u32, u32)> {
    let mut range: Option<(u32, u32)> = None;
    for line in script.lines() {
        let Some(rest) = line.strip_prefix("#SBATCH --array=") else {
            continue;
        };
        let (start, end) = rest.split_once('-')?;
        // A `%throttle` suffix is allowed; anything else after the end
        // (steps, spaces) is a shape this parser does not understand.
        // Trailing whitespace is tolerated — schedulers accept it.
        let end = end.trim_end().split('%').next()?;
        let (start, end) = (start.parse().ok()?, end.parse().ok()?);
        if start > end {
            return None;
        }
        match range {
            // Two directives: which one the scheduler honours is ambiguous.
            Some(existing) if existing != (start, end) => return None,
            Some(_) => {}
            None => range = Some((start, end)),
        }
    }
    range
}

/// Whether a downstream rule may chain element-wise onto its single
/// upstream dependency with `--dependency=aftercorr:` (issue #371).
///
/// Requires both scripts to declare the same straight array range — SLURM
/// pairs `aftercorr` elements by index, so a mismatched start or end would
/// strand indexes silently. Any other shape keeps the ready-batch
/// `afterok:` dependency, which is correct for every fallback case.
pub fn elementwise_aftercorr(upstream_script: &str, downstream_script: &str) -> bool {
    match (
        parse_array_range(upstream_script),
        parse_array_range(downstream_script),
    ) {
        (Some(up), Some(down)) => up == down,
        _ => false,
    }
}

/// Pre-create the directories a scheduler will open log files in.
///
/// Every backend opens the `--output`/`-o`/`-e` files at job LAUNCH, before
/// the script body runs — the body's `mkdir -p logs` is too late (proven on
/// a live SLURM cluster: a job into a fresh run dir failed instantly with
/// "error: can't open ... logs/align.out"; the same job into a pre-made
/// `logs/` completed). Resolve each directive path relative to the pinned
/// working directory — the same resolution the scheduler does for
/// `--chdir`/`-d`/`-wd`/`-cwd` — and create its parent.
fn ensure_log_dirs(script: &str, workdir: &Path) {
    let base = absolute_workdir(workdir);
    for path in script_log_paths(script) {
        let full = if path.is_absolute() {
            path
        } else {
            base.join(path)
        };
        if let Some(parent) = full.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            // Never block submission on housekeeping: the scheduler may
            // itself create the directory, and the old behavior was to
            // create nothing at all.
            eprintln!(
                "warning: could not pre-create log dir {}: {e}",
                parent.display()
            );
        }
    }
}

/// Finish a generated scheduler script for submission.
///
/// Two things the directive generator (`crate::cluster`) does not do. It
/// pins the working directory — PBS/Torque starts a job in `$HOME` and SGE
/// in a queue-configured directory unless told otherwise, so a rule's
/// relative output paths (`out/{sample}.txt`) silently landed outside the
/// run directory. And it drops the `set -e` / `mkdir -p logs` lines the
/// array renderer appends after the command on top of the copies the base
/// script already carries before it.
fn finish_script(script: &str, backend: &ClusterBackend, workdir: &Path) -> String {
    // #534: scheduler directive paths must survive spaces (and quotes) —
    // SLURM/SGE/LSB parse the directive value as one token only when
    // quoted; PBS's shell `cd` needs shell quoting.
    let dir = crate::environment::escape_for_sh_single_quote(
        &absolute_workdir(workdir).display().to_string(),
    );
    let directive = match backend {
        ClusterBackend::Slurm => format!("#SBATCH --chdir='{dir}'"),
        // OpenPBS qsub has no working-directory option at all (no `-d` —
        // that is Torque; no `-w` — that is PBS Pro), so for PBS the pin
        // happens in the script body instead of as a scheduler directive.
        ClusterBackend::Pbs => format!("cd '{dir}' || exit 1"),
        ClusterBackend::Sge => format!("#$ -wd '{dir}'"),
        ClusterBackend::Lsf => format!("#BSUB -cwd '{dir}'"),
        ClusterBackend::Kubernetes | ClusterBackend::AwsBatch | ClusterBackend::GcpBatch => {
            return script.to_string();
        }
    };
    let mut lines = script.lines();
    let mut finished = vec![lines.next().unwrap_or_default().to_string()];
    let mut rest: Vec<String> = lines.map(String::from).collect();
    if backend == &ClusterBackend::Pbs {
        // A PBS body `cd` must come AFTER the whole `#PBS` directive block —
        // qsub stops parsing directives at the first non-directive line, so
        // inserting it at line 2 would silently drop every directive after
        // it.
        let insert_at = rest
            .iter()
            .take_while(|line| line.starts_with("#PBS"))
            .count();
        rest.insert(insert_at, directive);
        finished.extend(rest);
    } else {
        finished.push(directive);
        finished.extend(rest);
    }
    let mut deduped: Vec<String> = Vec::with_capacity(finished.len());
    for line in finished {
        let duplicated = (line == "set -e" || line == "mkdir -p logs")
            && deduped.iter().any(|existing| existing.as_str() == line);
        if !duplicated {
            deduped.push(line);
        }
    }
    // Keep whatever trailing newline the generator emitted.
    let tail = if script.ends_with('\n') { "\n" } else { "" };
    format!("{}{tail}", deduped.join("\n"))
}

/// Cluster executor: renders scripts with the existing directive generator
/// and maps submit/poll/cancel/logs onto each scheduler's CLI.
pub struct ClusterExecutor {
    backend: ClusterBackend,
    cluster: ClusterJobConfig,
    /// Optional directory holding the scheduler binaries (sbatch, squeue,
    /// …) — for nonstandard installations and the mock-scheduler CI harness
    /// (`tests/fixtures/mock-scheduler`).
    bin_dir: Option<PathBuf>,
    /// Extra environment for scheduler commands (e.g. `MOCK_SCHEDULER_DIR`).
    env: Vec<(String, String)>,
    /// Working directory array jobs run in. A per-index command carries its
    /// own `cd` guard, but the scheduler still launches the script somewhere
    /// before reading it — and that somewhere defaults to `$HOME` on PBS.
    workdir: PathBuf,
    /// Operator-supplied secret-reference mapping for cloud bodies
    /// (`OXO_FLOW_SECRET_REFS`, issue #1120 / ledger RB-12): routed
    /// `OXO_FLOW_SENSITIVE_<n>` name → backend-native reference. GCP Batch
    /// projects the used entries into `taskSpec.environment
    /// .secretVariables`; AWS Batch has no submit-time field and ignores it.
    secret_refs: std::collections::BTreeMap<String, String>,
}

impl ClusterExecutor {
    pub fn new(backend: ClusterBackend, cluster: ClusterJobConfig) -> Self {
        Self {
            backend,
            cluster,
            bin_dir: None,
            env: Vec::new(),
            workdir: PathBuf::from("."),
            secret_refs: std::collections::BTreeMap::new(),
        }
    }

    /// Attach the operator's secret-reference mapping (`OXO_FLOW_SECRET_REFS`,
    /// issue #1120 / ledger RB-12) to the cloud body renderers.
    pub fn with_secret_refs(mut self, refs: std::collections::BTreeMap<String, String>) -> Self {
        self.secret_refs = refs;
        self
    }

    /// Set the directory array jobs run in.
    pub fn with_workdir(mut self, dir: PathBuf) -> Self {
        self.workdir = dir;
        self
    }

    /// Resolve scheduler commands from `dir` instead of `PATH`.
    pub fn with_scheduler_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.bin_dir = Some(dir);
        self
    }

    /// Add an environment variable for scheduler commands.
    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
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
        if !out.status.success() {
            return Err(OxoFlowError::Config {
                message: format!(
                    "'{program}' exited {}: {}",
                    out.status.code().unwrap_or(-1),
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            });
        }
        Ok(out)
    }

    /// `--location=…` for gcloud batch invocations (describe/delete/submit
    /// all require it unless the site's gcloud defaults carry one). The
    /// `--queue` cluster option carries the region/location for this
    /// backend, mirroring how AWS Batch reads it as the job queue.
    fn gcloud_location_args(&self) -> Vec<String> {
        match self.cluster.queue.as_deref() {
            Some(loc) => vec![format!("--location={loc}")],
            None => Vec::new(),
        }
    }

    async fn poll_pbs_lsf(
        &self,
        program: &str,
        job_ids: &[String],
    ) -> Result<HashMap<String, BackendJobStatus>> {
        let args: Vec<&str> = job_ids.iter().map(String::as_str).collect();
        // A by-id listing answers for jobs still in the live queue; a job
        // that already finished makes the whole invocation exit non-zero
        // (OpenPBS exits 35 with "use -x or -H", LSF prints "not found").
        // That is expected scheduler behaviour, not a driver failure — read
        // whatever rows came back and let the accounting probe settle the
        // missing ids (same shape as the SGE per-job path above).
        let out = match self.run_cmd(program, &args).await {
            Ok(out) => out,
            Err(err) => {
                tracing::warn!(
                    "{} listing failed, treating ids as absent: {}",
                    program,
                    err
                );
                return Ok(HashMap::new());
            }
        };
        let mut statuses = HashMap::new();
        for line in String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
        {
            if let Some((id, st)) = parse_status_line(&self.backend, line) {
                statuses.insert(id, st);
            }
        }
        Ok(statuses)
    }
}

#[async_trait::async_trait]
impl super::ExecutorBackend for ClusterExecutor {
    fn name(&self) -> &'static str {
        match self.backend {
            ClusterBackend::Slurm => "slurm",
            ClusterBackend::Pbs => "pbs",
            ClusterBackend::Sge => "sge",
            ClusterBackend::Lsf => "lsf",
            ClusterBackend::Kubernetes => "kubernetes",
            ClusterBackend::AwsBatch => "aws-batch",
            ClusterBackend::GcpBatch => "gcp-batch",
        }
    }

    fn render_script(&self, rule: &ScheduledRule) -> Result<String> {
        Ok(finish_script(
            &generate_submit_script_with_secrets(
                &self.backend,
                &rule.rule,
                &rule.shell_cmd,
                &self.secret_refs,
                &self.cluster,
            )?,
            &self.backend,
            &rule.workdir,
        ))
    }

    fn render_array_script(
        &self,
        rule: &crate::rule::Rule,
        cmd_dir: &str,
        count: usize,
        sensitive_refs: &[String],
    ) -> Result<String> {
        Ok(finish_script(
            &generate_array_submit_script_with_secrets(
                &self.backend,
                rule,
                cmd_dir,
                count,
                sensitive_refs,
                &self.secret_refs,
                &self.cluster,
            )?,
            &self.backend,
            &self.workdir,
        ))
    }

    /// The script-based schedulers copy the script into scheduler-visible
    /// storage; their routed sensitive values travel through the private
    /// env file the driver writes beside the script (issue #1120 / RB-12).
    fn uses_secret_env_file(&self) -> bool {
        matches!(
            self.backend,
            ClusterBackend::Slurm | ClusterBackend::Pbs | ClusterBackend::Sge | ClusterBackend::Lsf
        )
    }

    async fn submit(&self, script_path: &Path) -> Result<String> {
        // The scheduler opens the script's log files before the body runs —
        // pre-create their directories or a fresh run dir fails on launch
        // (issue: engine never created `logs/`, sbatch jobs died instantly).
        if let Ok(script) = std::fs::read_to_string(script_path) {
            ensure_log_dirs(&script, &self.workdir);
        }
        let path_str = script_path.to_string_lossy().to_string();
        // Per-backend submission shape. The HPC schedulers take the script
        // positionally (SLURM with --parsable for a bare id); the cloud
        // backends take a generated manifest whose invocations differ from
        // the script form — a batch/v1 Job for Kubernetes, a submit-job CLI
        // input document for AWS (jobName/jobQueue live inside the payload),
        // and a config file plus an id we mint for GCP.
        // GCP accepts the job id positionally but prints the submitted Job
        // resource, not the id; remember what was minted so `submit` can
        // return the scheduler handle every tracking path keys on.
        let mut minted_id: Option<String> = None;
        let (program, args): (&str, Vec<String>) = match self.backend {
            ClusterBackend::Slurm => ("sbatch", vec!["--parsable".into(), path_str.clone()]),
            ClusterBackend::Pbs | ClusterBackend::Sge | ClusterBackend::Lsf => (
                crate::cluster::submit_command(&self.backend),
                vec![path_str.clone()],
            ),
            // `create -f` prints `job.batch/<name> created` — the exact
            // shape parse_job_id's Kubernetes branch reads.
            ClusterBackend::Kubernetes => (
                "kubectl",
                vec!["create".into(), "-f".into(), path_str.clone()],
            ),
            ClusterBackend::AwsBatch => (
                "aws",
                vec![
                    "batch".into(),
                    "submit-job".into(),
                    "--cli-input-json".into(),
                    format!("file://{path_str}"),
                    // The default output is the whole SubmitJob response
                    // JSON — line breaks and braces in every path that
                    // keys on the id (job.id, status.json, and the array
                    // child ids `{id}:{n}`). Ask for the bare id.
                    "--query".into(),
                    "jobId".into(),
                    "--output".into(),
                    "text".into(),
                ],
            ),
            ClusterBackend::GcpBatch => {
                // GCP Batch takes the job id positionally and the Job
                // resource via --config. The manifest carries no id, so one
                // is minted from the job directory (the sanitized rule
                // name) plus the submission millisecond for uniqueness
                // across concurrent runs. `--location` rides the --queue
                // cluster option, mirroring how AWS Batch reads it as the
                // job queue; a missing location fails fast with gcloud's
                // own remediation message.
                let dir = script_path
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("job");
                let job_id = mint_gcp_job_id(dir, Utc::now().timestamp_millis());
                let mut args = vec![
                    "batch".to_string(),
                    "jobs".to_string(),
                    "submit".to_string(),
                    job_id.clone(),
                ];
                args.extend(self.gcloud_location_args());
                args.push("--config".to_string());
                args.push(format!("file://{path_str}"));
                minted_id = Some(job_id);
                ("gcloud", args)
            }
        };
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.run_cmd(program, &arg_refs).await?;
        if let Some(id) = minted_id {
            // `gcloud batch jobs submit <id>` prints the submitted Job
            // resource (name, taskGroups, …) — not the id it accepted
            // positionally. Returning that resource as a job id broke every
            // later describe/cancel and the per-element task handles.
            return Ok(id);
        }
        parse_job_id(
            &self.backend,
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
        )
    }

    async fn poll(&self, job_ids: &[String]) -> Result<HashMap<String, BackendJobStatus>> {
        match self.backend {
            ClusterBackend::Slurm => {
                let list = job_ids.join(",");
                // A transient squeue failure (slurmctld flap, scheduler
                // restart) must not tear down the whole run: one failed
                // poll used to cancel every in-flight job via the driver's
                // cancel-on-error, while the equivalent PBS/LSF listing was
                // deliberately warn-and-continue. Same contract here — the
                // accounting probe and the blind-settlement guard take over.
                let out = match self
                    .run_cmd("squeue", &["-j", &list, "--noheader", "-o", "%i|%T"])
                    .await
                {
                    Ok(out) => out,
                    Err(err) => {
                        tracing::warn!("squeue poll failed, treating ids as absent: {err}");
                        return Ok(HashMap::new());
                    }
                };
                let mut statuses = HashMap::new();
                for line in String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                {
                    if let Some((id, st)) = parse_status_line(&self.backend, line) {
                        statuses.insert(id, st);
                    }
                }
                Ok(statuses)
            }
            ClusterBackend::Pbs => self.poll_pbs_lsf("qstat", job_ids).await,
            ClusterBackend::Lsf => self.poll_pbs_lsf("bjobs", job_ids).await,
            ClusterBackend::Sge => {
                // qstat -j pairs "job_number:" with "state:" lines. For a
                // finished job the command exits non-zero (it left the
                // queue) — settle it from accounting instead of aborting
                // the whole driver on the error.
                let mut statuses = HashMap::new();
                for id in job_ids {
                    let out = match self.run_cmd("qstat", &["-j", id]).await {
                        Ok(o) => o,
                        Err(_) => {
                            let state = self
                                .terminal_status(id)
                                .await
                                .map_or(BackendJobStatus::Unknown, |r| r.status);
                            if state != BackendJobStatus::Unknown {
                                statuses.insert(id.clone(), state);
                            }
                            continue;
                        }
                    };
                    let mut number = None;
                    let mut state = BackendJobStatus::Unknown;
                    for line in String::from_utf8_lossy(&out.stdout).lines().map(str::trim) {
                        if let Some((n, _)) = parse_status_line(&self.backend, line) {
                            number = Some(n);
                        }
                        if let Some(s) = line.strip_prefix("state:") {
                            state = sge_state(s.trim());
                        }
                    }
                    if let Some(n) = number {
                        statuses.insert(n, state);
                    }
                }
                Ok(statuses)
            }
            ClusterBackend::Kubernetes => {
                let mut statuses = HashMap::new();
                // Element handles (`{jobName}_{ordinal}`, minted by
                // `array_element_id`) resolve against the Indexed Job's
                // per-index status; every other id is a plain Job name.
                let mut by_base: std::collections::BTreeMap<&str, Vec<(&String, u32)>> =
                    std::collections::BTreeMap::new();
                for id in job_ids {
                    let Some((base, index)) = k8s_element_index(id) else {
                        // succeeded + failed + spec.completions in one
                        // probe: with `backoffLimit: 0` a failed pod fails
                        // the Job while `succeeded` stays 0, so reading
                        // succeeded alone polled a dead job forever.
                        // Pipe-separated so a missing field leaves an empty
                        // cell instead of shifting the columns.
                        let out = match self
                            .run_cmd(
                                "kubectl",
                                &[
                                    "get",
                                    "job",
                                    id,
                                    "-o",
                                    "jsonpath={.status.succeeded}|{.status.failed}|{.spec.completions}",
                                ],
                            )
                            .await
                        {
                            Ok(o) => o,
                            Err(_) => continue,
                        };
                        if !out.status.success() {
                            continue;
                        }
                        let text = String::from_utf8_lossy(&out.stdout);
                        let trimmed = text.trim();
                        // With the pipe template the three cells always
                        // print (absent fields leave EMPTY cells). An empty
                        // answer is not a status at all → Unknown. The pipe
                        // shape is what stops an absent `succeeded` from
                        // shifting `spec.completions` into its cell and
                        // fabricating success (ledger RB-06).
                        if trimmed.is_empty() {
                            continue;
                        }
                        let cells: Vec<&str> = trimmed.split('|').collect();
                        if cells.len() < 3 {
                            continue;
                        }
                        // Kubernetes omits zero-valued counters, so an empty
                        // cell is 0, not missing data.
                        let succeeded = cells[0].trim().parse::<u64>().unwrap_or(0);
                        let failed = cells[1].trim().parse::<u64>().unwrap_or(0);
                        let completions = cells[2].trim().parse::<u64>().unwrap_or(0);
                        let st = if failed > 0 {
                            BackendJobStatus::Failed
                        } else if succeeded > 0 && (completions == 0 || succeeded >= completions) {
                            // No completions field: a plain (non-indexed)
                            // Job completes at 1.
                            BackendJobStatus::Completed
                        } else {
                            BackendJobStatus::Running
                        };
                        statuses.insert(id.clone(), st);
                        continue;
                    };
                    by_base.entry(base).or_default().push((id, index));
                }
                for (base, elements) in by_base {
                    // One probe per array base: the Indexed Job keeps every
                    // element's outcome in its own status fields, so no
                    // element inherits a sibling's verdict (ledger RB-06).
                    let out = match self
                        .run_cmd(
                            "kubectl",
                            &[
                                "get",
                                "job",
                                base,
                                "-o",
                                // completedIndexes/failedIndexes are the
                                // per-index record; the counts are the
                                // fallback for clusters predating them;
                                // the terminal condition is what lets an
                                // unrecorded index settle on a dead Job
                                // (backoffLimit: 0 never starts the rest).
                                "jsonpath={.status.succeeded}|{.status.failed}|{.spec.completions}|{.status.completedIndexes}|{.status.failedIndexes}|{.status.conditions[*].type}",
                            ],
                        )
                        .await
                    {
                        Ok(o) => o,
                        Err(_) => continue,
                    };
                    if !out.status.success() {
                        continue;
                    }
                    let text = String::from_utf8_lossy(&out.stdout);
                    let trimmed = text.trim();
                    // The template prints six cells (the literal pipes) even
                    // when every field is absent; anything shorter is not a
                    // status answer → Unknown.
                    if trimmed.is_empty() {
                        continue;
                    }
                    let cells: Vec<&str> = trimmed.split('|').collect();
                    if cells.len() < 6 {
                        continue;
                    }
                    let number = |i: usize| cells.get(i).and_then(|v| v.trim().parse::<u64>().ok());
                    // `status.failed` is omitempty: an empty cell is zero
                    // failures, not missing data. Requiring a value here
                    // would keep every healthy array in flight until the
                    // blind-settlement guard failed it.
                    let failed_count = number(1).unwrap_or(0);
                    let succeeded = number(0);
                    let completions = number(2);
                    let completed =
                        super::k8s::parse_index_list(cells.get(3).copied().unwrap_or_default());
                    let failed_indexes =
                        super::k8s::parse_index_list(cells.get(4).copied().unwrap_or_default());
                    // The conditions cell is the space-separated list of
                    // `status.conditions[*].type` values; a missing cell (an
                    // older probe output) leaves the Job non-terminal, and a
                    // Job still running has no terminal condition at all.
                    let conditions = cells.get(5).copied().unwrap_or_default();
                    let job_terminal = conditions.split_whitespace().fold(
                        None,
                        |acc: Option<super::k8s::IndexedJobTerminal>, t| match t {
                            "Failed" => Some(super::k8s::IndexedJobTerminal::Failed),
                            "Complete" if acc != Some(super::k8s::IndexedJobTerminal::Failed) => {
                                Some(super::k8s::IndexedJobTerminal::Complete)
                            }
                            _ => acc,
                        },
                    );
                    for (element_id, index) in elements {
                        let mut verdict = super::k8s::indexed_element_status(
                            &completed,
                            &failed_indexes,
                            failed_count,
                            index,
                            job_terminal,
                        );
                        // Clusters that do not publish completedIndexes leave
                        // the set empty even for a fully successful Job; the
                        // aggregate counts still prove every index completed.
                        if verdict == BackendJobStatus::Running
                            && completed.is_empty()
                            && let (Some(succeeded), Some(completions)) = (succeeded, completions)
                            && completions > 0
                            && succeeded >= completions
                        {
                            verdict = BackendJobStatus::Completed;
                        }
                        statuses.insert(element_id.clone(), verdict);
                    }
                }
                Ok(statuses)
            }
            ClusterBackend::AwsBatch => {
                // Batched describe per chunk (describe-jobs caps `--jobs` at
                // [`AWS_DESCRIBE_JOBS_MAX_IDS`] ids per call); the
                // join(|,…) query renders the same `id|STATE` rows the
                // shared parser reads, replacing the former per-job serial
                // describe calls.
                if job_ids.is_empty() {
                    return Ok(HashMap::new());
                }
                let mut statuses = HashMap::new();
                for group in job_ids.chunks(AWS_DESCRIBE_JOBS_MAX_IDS) {
                    // `--jobs` takes a list: each id is its own argv entry
                    // (a single space-joined argument would be one invalid
                    // id). Element ids are real AWS Batch child job ids
                    // (`{parent}:{arrayIndex}`), so one describe answers
                    // per element — no base-verdict expansion (ledger
                    // RB-06).
                    let mut args: Vec<String> =
                        vec!["batch".into(), "describe-jobs".into(), "--jobs".into()];
                    args.extend(group.iter().cloned());
                    args.push("--query".into());
                    args.push(r"jobs[].join(|,[jobId,status])".into());
                    args.push("--output".into());
                    args.push("text".into());
                    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                    let out = match self.run_cmd("aws", &arg_refs).await {
                        Ok(out) => out,
                        Err(err) => {
                            // Same contract as the PBS/LSF listing: a
                            // transient CLI failure is scheduler noise, not
                            // a driver failure — report nothing for THIS
                            // chunk and let the accounting probe settle its
                            // ids; the other chunks still answer.
                            tracing::warn!(
                                "aws batch describe-jobs failed, treating ids as absent: {err}"
                            );
                            continue;
                        }
                    };
                    for line in String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                    {
                        if let Some((id, st)) = parse_status_line(&self.backend, line) {
                            statuses.insert(id, st);
                        }
                    }
                }
                Ok(statuses)
            }
            ClusterBackend::GcpBatch => {
                let mut statuses = HashMap::new();
                // Element handles (`{job}:{taskIndex}`, minted by
                // `array_element_id`) resolve through the job's tasks; every
                // other id is a plain job.
                let mut by_base: std::collections::BTreeMap<&str, Vec<(&String, u32)>> =
                    std::collections::BTreeMap::new();
                for id in job_ids {
                    let Some((base, index)) = gcp_element_index(id) else {
                        let mut args = vec!["batch".to_string(), "jobs".to_string()];
                        args.extend(self.gcloud_location_args());
                        args.push("describe".to_string());
                        args.push(id.clone());
                        args.push("--format=value(status.state)".to_string());
                        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                        let out = match self.run_cmd("gcloud", &arg_refs).await {
                            Ok(o) => o,
                            Err(_) => continue,
                        };
                        if !out.status.success() {
                            continue;
                        }
                        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                        // Empty or unrecognized output is missing status
                        // data: UNKNOWN, never a live state (which would
                        // poll forever) and never success (ledger RB-06).
                        let st = match text.as_str() {
                            "SUCCEEDED" => BackendJobStatus::Completed,
                            "FAILED" | "DELETION_IN_PROGRESS" => BackendJobStatus::Failed,
                            "RUNNING" => BackendJobStatus::Running,
                            "QUEUED" | "SCHEDULED" => BackendJobStatus::Pending,
                            _ => BackendJobStatus::Unknown,
                        };
                        statuses.insert(id.clone(), st);
                        continue;
                    };
                    by_base.entry(base).or_default().push((id, index));
                }
                for (base, elements) in by_base {
                    let mut args = vec!["batch".to_string(), "tasks".to_string()];
                    args.extend(self.gcloud_location_args());
                    args.push("list".to_string());
                    args.push(format!("--job={base}"));
                    args.push("--format=value(name,status.state)".to_string());
                    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                    let out = match self.run_cmd("gcloud", &arg_refs).await {
                        Ok(o) => o,
                        Err(_) => continue,
                    };
                    if !out.status.success() {
                        continue;
                    }
                    let states =
                        super::gcp_batch::parse_task_states(&String::from_utf8_lossy(&out.stdout));
                    for (element_id, index) in elements {
                        // A task the store does not describe is unknown —
                        // the driver's blind-settlement guard ends it, and
                        // no sibling's verdict is copied onto it.
                        let state = states.get(&index).copied().unwrap_or_default();
                        statuses.insert(element_id.clone(), state);
                    }
                }
                Ok(statuses)
            }
        }
    }

    async fn cancel(&self, job_id: &str) -> Result<()> {
        // One shared mapping with the standalone `cluster cancel` command.
        let invocations = cancel_invocations(&self.backend, &[job_id.to_string()]);
        for (program, args) in invocations {
            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            self.run_cmd(&program, &arg_refs).await?;
        }
        Ok(())
    }

    async fn logs(&self, job_id: &str) -> Result<String> {
        let (program, args): (&str, Vec<String>) = match self.backend {
            ClusterBackend::Slurm => (
                "sacct",
                vec![
                    "-j".to_string(),
                    job_id.to_string(),
                    "--format=JobID,State,ExitCode,Elapsed,MaxRSS,TotalCPU".to_string(),
                ],
            ),
            ClusterBackend::Pbs => (
                "qstat",
                vec!["-x".to_string(), "-f".to_string(), job_id.to_string()],
            ),
            ClusterBackend::Sge => ("qacct", vec!["-j".to_string(), job_id.to_string()]),
            ClusterBackend::Lsf => ("bacct", vec![job_id.to_string()]),
            // kubectl resolves a bare name as a POD; Job resources need the
            // `job/` prefix or the call always fails.
            ClusterBackend::Kubernetes => {
                ("kubectl", vec!["logs".to_string(), format!("job/{job_id}")])
            }
            ClusterBackend::AwsBatch => (
                "aws",
                vec![
                    "batch".to_string(),
                    "describe-jobs".to_string(),
                    "--jobs".to_string(),
                    job_id.to_string(),
                ],
            ),
            ClusterBackend::GcpBatch => {
                let mut args = vec!["batch".to_string(), "jobs".to_string()];
                args.extend(self.gcloud_location_args());
                args.push("describe".to_string());
                args.push(job_id.to_string());
                ("gcloud", args)
            }
        };
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.run_cmd(program, &arg_refs).await?;
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    /// Terminal record for a job that has left the live queue, read from the
    /// scheduler's accounting store (sacct / qstat -x / qacct / bacct).
    ///
    /// SLURM fallback (issue #244): on clusters WITHOUT slurmdbd, sacct
    /// errors ("accounting storage is disabled") and returns nothing —
    /// settlement would then poll forever. `scontrol show job <id>` reads
    /// the controller's in-memory record, which survives briefly after a
    /// job completes even without accounting; its JobState/ExitCode parse
    /// into the same TerminalRecord (Elapsed/MaxRSS/TotalCPU unavailable —
    /// the controller does not report them).
    async fn terminal_status(&self, job_id: &str) -> Option<TerminalRecord> {
        if let Ok(text) = self.logs(job_id).await
            && let Some(rec) = parse_accounting(&self.backend, &text)
        {
            return Some(rec);
        }
        if self.backend == ClusterBackend::Slurm {
            let out = self
                .run_cmd("scontrol", &["show", "job", job_id])
                .await
                .ok()?;
            return parse_scontrol(&String::from_utf8_lossy(&out.stdout));
        }
        None
    }

    fn polls_elements_directly(&self) -> bool {
        // squeue lists array elements as `{jobid}_{index}`; a Kubernetes
        // Indexed Job reports completed/failed indexes; AWS Batch child
        // jobs and GCP Batch tasks are individually addressable. qstat /
        // bjobs report only the array base id in the invocations this
        // adapter issues (issue #136 H4, live-verified for PBS/SGE), so
        // those three keep base-level polling with explicit aggregate
        // provenance (issue #1132 / ledger RB-06).
        matches!(
            self.backend,
            ClusterBackend::Slurm
                | ClusterBackend::Kubernetes
                | ClusterBackend::AwsBatch
                | ClusterBackend::GcpBatch
        )
    }

    fn array_element_id(&self, base: &str, index: usize) -> String {
        // OpenPBS echoes an array submission as `38[]` and addresses its
        // elements as `38[1]` … ; LSF echoes the bare array id and
        // addresses elements `44[2]` — the SLURM `{base}_{index}` form is
        // not a queryable id on either. AWS Batch child ids and GCP Batch
        // task indexes are zero-based and both shaped `{base}:{index}`.
        match self.backend {
            ClusterBackend::Pbs if base.contains("[]") => {
                base.replacen("[]", &format!("[{index}]"), 1)
            }
            ClusterBackend::Pbs | ClusterBackend::Lsf => format!("{base}[{index}]"),
            ClusterBackend::AwsBatch => super::aws_batch::array_child_id(base, index),
            ClusterBackend::GcpBatch => super::gcp_batch::task_element_id(base, index),
            _ => format!("{base}_{index}"),
        }
    }
}

/// Split an accounting row on `|` when the store emitted the pipe-separated
/// form, on whitespace otherwise. Real `sacct` pads columns with spaces; the
/// mock scheduler and `sacct -P` both emit pipes.
fn accounting_columns(line: &str) -> Vec<&str> {
    if line.contains('|') {
        line.split('|').map(str::trim).collect()
    } else {
        line.split_whitespace().collect()
    }
}

/// Parse a SLURM `ExitCode` field into a process exit code.
///
/// The field is `<exit>:<signal>`. A signalled job reports exit `0`, so
/// reading only the first component would record an OOM kill (`0:9`) as a
/// clean exit — signalled jobs get the shell's `128 + signum` instead. Bare
/// integers (PBS `Exit_status`, SGE `exit_status`) parse as themselves.
fn parse_exit_code(raw: &str) -> Option<i32> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (exit, signal) = match raw.split_once(':') {
        Some((e, s)) => (e.trim().parse::<i32>().ok()?, s.trim().parse::<i32>().ok()?),
        None => (raw.parse::<i32>().ok()?, 0),
    };
    Some(if signal != 0 { 128 + signal } else { exit })
}

/// Parse a scheduler **accounting** duration (`[DD-]HH:MM:SS`, `MM:SS`, or
/// bare seconds) into seconds.
///
/// Deliberately looser than [`crate::rule::parse_walltime_secs`], which parses
/// *user* walltime input: accounting output is a fixed machine format, so
/// bare seconds (PBS) and `MM:SS` (LSF) are unambiguous here while the same
/// strings are ambiguous in a user config. Sub-second precision
/// (`00:00:05.004`, common in SLURM's `TotalCPU`) truncates. Keep this name
/// distinct from the config parser — a future caller must choose the grammar
/// it actually wants instead of silently inheriting the wrong one.
fn parse_accounting_duration_secs(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    if raw.is_empty() || raw == "UNLIMITED" || raw == "INVALID" {
        return None;
    }
    let (days, rest) = match raw.split_once('-') {
        Some((d, r)) => (d.trim().parse::<u64>().ok()?, r),
        None => (0, raw),
    };
    let mut secs = days * 86_400;
    let parts: Vec<&str> = rest.split(':').collect();
    // Trailing fractional seconds are dropped, not rounded: accounting
    // resolution is not the point of this number.
    let whole = |p: &str| p.split('.').next().unwrap_or(p).trim().parse::<u64>().ok();
    match parts.as_slice() {
        [h, m, s] => secs += whole(h)? * 3600 + whole(m)? * 60 + whole(s)?,
        [m, s] => secs += whole(m)? * 60 + whole(s)?,
        [s] => secs += whole(s)?,
        _ => return None,
    }
    Some(secs)
}

/// Parse an accounting memory figure into MB. SLURM suffixes `K`/`M`/`G`/`T`
/// (optionally `Kn`/`Kc` for per-node/per-core), PBS writes `1234kb`, SGE's
/// `ru_maxrss` is bare kilobytes. An unsuffixed value is read as kilobytes,
/// which is what every one of these stores means by a bare number.
fn parse_rss_mb(raw: &str) -> Option<u64> {
    let raw = raw.trim().trim_end_matches(['n', 'c']);
    if raw.is_empty() || raw == "0" {
        return None;
    }
    let lower = raw.to_ascii_lowercase();
    let (digits, per_mb) = if let Some(v) = lower.strip_suffix("kb") {
        (v, 1024.0)
    } else if let Some(v) = lower.strip_suffix("mb") {
        (v, 1.0)
    } else if let Some(v) = lower.strip_suffix("gb") {
        (v, 1.0 / 1024.0)
    } else if let Some(v) = lower.strip_suffix('k') {
        (v, 1024.0)
    } else if let Some(v) = lower.strip_suffix('m') {
        (v, 1.0)
    } else if let Some(v) = lower.strip_suffix('g') {
        (v, 1.0 / 1024.0)
    } else if let Some(v) = lower.strip_suffix('t') {
        (v, 1.0 / (1024.0 * 1024.0))
    } else {
        (lower.as_str(), 1024.0)
    };
    let value: f64 = digits.trim().parse().ok()?;
    let mb = value / per_mb;
    if mb <= 0.0 {
        None
    } else {
        Some(mb.round() as u64)
    }
}

/// Parse scheduler accounting output into a terminal record, or `None` when
/// the record is absent or the job is not terminal.
fn parse_accounting(backend: &ClusterBackend, text: &str) -> Option<TerminalRecord> {
    match backend {
        ClusterBackend::Slurm => parse_sacct(text),
        ClusterBackend::Pbs => {
            // qstat -x -f: "    job_state = C" + "    Exit_status = N", with
            // measurements under "resources_used.<field>". OpenPBS 23 marks
            // history jobs "F" (finished) rather than "C" — the exit code
            // still says which.
            let state = text
                .lines()
                .map(str::trim)
                .find_map(|l| l.strip_prefix("job_state = "))
                .unwrap_or_default()
                .to_string();
            if state != "C" && state != "F" && state != "E" {
                return None;
            }
            let field = |key: &str| {
                text.lines()
                    .map(str::trim)
                    .find_map(|l| l.strip_prefix(key)?.strip_prefix(" = "))
            };
            let exit_code = field("Exit_status").and_then(parse_exit_code);
            // "E" (exiting) is transient: the job's process is done but the
            // exit status may not have landed yet. No status, no settlement.
            if state == "E" && exit_code.is_none() {
                return None;
            }
            Some(TerminalRecord {
                status: if exit_code == Some(0) {
                    BackendJobStatus::Completed
                } else {
                    BackendJobStatus::Failed
                },
                exit_code,
                elapsed_secs: field("resources_used.walltime")
                    .and_then(parse_accounting_duration_secs),
                max_rss_mb: field("resources_used.mem").and_then(parse_rss_mb),
                cpu_seconds: field("resources_used.cput").and_then(parse_accounting_duration_secs),
            })
        }
        ClusterBackend::Sge => {
            // qacct emits "<key><padding><value>" pairs, one per line.
            let field = |key: &str| {
                text.lines()
                    .map(str::trim)
                    .find_map(|l| Some(l.strip_prefix(key)?.trim()))
                    .filter(|v| !v.is_empty())
            };
            let exit_code = field("exit_status").and_then(parse_exit_code)?;
            Some(TerminalRecord {
                status: if exit_code == 0 {
                    BackendJobStatus::Completed
                } else {
                    BackendJobStatus::Failed
                },
                exit_code: Some(exit_code),
                elapsed_secs: field("ru_wallclock").and_then(parse_accounting_duration_secs),
                max_rss_mb: field("ru_maxrss").and_then(parse_rss_mb),
                cpu_seconds: field("cpu").and_then(parse_accounting_duration_secs),
            })
        }
        ClusterBackend::Lsf => {
            // bacct: "Job <id>, ..., Status <DONE|EXIT|RUN>, ...". The
            // measurement columns vary too much between LSF versions to
            // parse blind, so this stays state-only.
            if text.contains("Status <DONE>") {
                Some(TerminalRecord::status_only(BackendJobStatus::Completed))
            } else if text.contains("Status <EXIT>") {
                Some(TerminalRecord::status_only(BackendJobStatus::Failed))
            } else {
                None
            }
        }
        ClusterBackend::Kubernetes | ClusterBackend::AwsBatch | ClusterBackend::GcpBatch => None,
    }
}

/// Parse `sacct --format=JobID,State,ExitCode,Elapsed,MaxRSS,TotalCPU`.
///
/// A job is several rows: the allocation itself, plus `.batch`, `.extern`,
/// and one per step. State and exit code come from the allocation row, but
/// `MaxRSS` is only populated on the step rows, so peak memory is the max
/// over every row — reading the first row alone (as this did) always
/// reported no memory at all.
fn parse_sacct(text: &str) -> Option<TerminalRecord> {
    // Column meanings come from the header row when the store echoes one
    // (real `sacct --format=…` prints the requested names); without a
    // header the fixed `--format` order — State,ExitCode,Elapsed,MaxRSS,
    // TotalCPU at 1..5 — is the fallback. Assuming the requested order
    // outright broke against stores that print their own fixed layout: the
    // tx-ubuntu mock scheduler emits JobID,Elapsed,State,ExitCode,MaxRSS
    // regardless of the request, so no state ever matched and every
    // finished job stayed Unknown in `cluster status` even though the
    // accounting overlay ran.
    let mut layout = [1usize, 2, 3, 4, 5]; // state, exit, elapsed, maxrss, cpu
    let mut rows: Vec<Vec<&str>> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('-') {
            continue;
        }
        let cells = accounting_columns(trimmed);
        // Reviewer catch (release-audit round): the first cell may carry a
        // sacct format-name variant — `JobIDRaw` on raw stores, `JobIDOK` on
        // completed-only views. Prefix-matching (not exact equality) keeps
        // every JobID* header recognized; main's blanket
        // `starts_with("JobID")` filter treated all of them as headers, and
        // the tighter check here would have regressed those stores.
        if cells.len() > 1 && cells[0].to_ascii_lowercase().starts_with("jobid") {
            // Header row: derive the column layout from the names actually
            // present; a name outside the known set keeps its positional
            // fallback slot.
            for (i, name) in cells.iter().enumerate() {
                match name.trim().to_ascii_lowercase().as_str() {
                    "state" => layout[0] = i,
                    "exitcode" => layout[1] = i,
                    "elapsed" => layout[2] = i,
                    "maxrss" => layout[3] = i,
                    "totalcpu" => layout[4] = i,
                    _ => {}
                }
            }
            continue;
        }
        rows.push(cells);
    }
    let allocation = rows.first()?;
    let state = allocation.get(layout[0])?.trim();
    // The same state vocabulary the live poller maps in `parse_status_line`:
    // a job must not settle as Failed through one path and Unknown through
    // the other depending on whether squeue still had it.
    let status = match state.split_whitespace().next().unwrap_or(state) {
        "COMPLETED" => BackendJobStatus::Completed,
        "FAILED" | "TIMEOUT" | "OUT_OF_MEMORY" | "NODE_FAIL" | "BOOT_FAIL" => {
            BackendJobStatus::Failed
        }
        // "CANCELLED by <uid>" / "PREEMPTED" — sacct appends the reason.
        "CANCELLED" | "PREEMPTED" => BackendJobStatus::Cancelled,
        _ => return None,
    };
    let max_rss_mb = rows
        .iter()
        .filter_map(|r| r.get(layout[3]).copied().and_then(parse_rss_mb))
        .max();
    // TotalCPU is likewise per-step; the allocation row leaves it blank.
    let cpu_seconds = rows
        .iter()
        .filter_map(|r| {
            r.get(layout[4])
                .copied()
                .and_then(parse_accounting_duration_secs)
        })
        .max();
    Some(TerminalRecord {
        status,
        exit_code: allocation.get(layout[1]).copied().and_then(parse_exit_code),
        elapsed_secs: allocation
            .get(layout[2])
            .copied()
            .and_then(parse_accounting_duration_secs),
        max_rss_mb,
        cpu_seconds,
    })
}

/// Parse `scontrol show job <id>` output (issue #244): the controller's
/// in-memory record, available on clusters WITHOUT slurmdbd while the job
/// stays in the controller's cache after completion. Only state and exit
/// code are recoverable — the controller does not report Elapsed/MaxRSS/
/// TotalCPU.
fn parse_scontrol(text: &str) -> Option<TerminalRecord> {
    let mut job_state = None;
    let mut exit_code = None;
    for token in text.split_whitespace() {
        if let Some(state) = token.strip_prefix("JobState=") {
            job_state = Some(state.trim_end_matches(['(', '+']).to_string());
        }
        if let Some(code) = token.strip_prefix("ExitCode=") {
            exit_code = Some(code.to_string());
        }
    }
    // A pending/running record from the controller is not terminal — the
    // live poller owns those states; only settle on terminal vocabulary.
    // "CANCELLED by <uid>"/"CANCELLED+(Reason)" — sacct/scontrol append
    // reasons; the alphabetic head is the state word.
    let state_word: String = job_state
        .as_deref()?
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    let status = match state_word.as_str() {
        "COMPLETED" => BackendJobStatus::Completed,
        "FAILED" | "TIMEOUT" | "OUT_OF_MEMORY" | "NODE_FAIL" | "BOOT_FAIL" => {
            BackendJobStatus::Failed
        }
        "CANCELLED" | "PREEMPTED" => BackendJobStatus::Cancelled,
        _ => return None,
    };
    Some(TerminalRecord {
        status,
        exit_code: exit_code.as_deref().and_then(parse_exit_code),
        ..TerminalRecord::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ExecutorBackend;
    use crate::cluster::ClusterBackend;
    use crate::config::WorkflowConfig;
    use crate::dag::WorkflowDag;
    use crate::environment::EnvironmentResolver;
    use std::collections::HashMap;

    fn single_rule_plan() -> crate::backend::ScheduledPlan {
        let dir = tempfile::tempdir().unwrap();
        let toml = r#"
            [workflow]
            name = "single"
            [[rules]]
            name = "align"
            shell = "bwa mem ref.fa in.fq > out.bam"
            output = ["out.bam"]
        "#;
        let path = dir.path().join("wf.oxoflow");
        std::fs::write(&path, toml).unwrap();
        let mut config = WorkflowConfig::from_file(&path).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        crate::backend::ScheduledPlan::build(
            &config,
            &dag,
            std::path::Path::new("."),
            &EnvironmentResolver::new(),
            &HashMap::new(),
        )
        .unwrap()
    }

    fn cluster_config() -> ClusterJobConfig {
        ClusterJobConfig {
            backend: ClusterBackend::Slurm,
            queue: Some("compute".into()),
            account: None,
            walltime: None,
            extra_args: vec![],
        }
    }

    #[test]
    fn render_parity_with_cluster_rs() {
        let plan = single_rule_plan();
        let exec = ClusterExecutor::new(ClusterBackend::Slurm, cluster_config());
        let via_trait = exec.render_script(&plan.rules["align"]).unwrap();
        // The executor is a thin render layer over the directive generator,
        // plus the working-directory pin and the duplicate-line cleanup.
        let direct = finish_script(
            &generate_submit_script_with_secrets(
                &ClusterBackend::Slurm,
                &plan.rules["align"].rule,
                &plan.rules["align"].shell_cmd,
                &std::collections::BTreeMap::new(),
                &cluster_config(),
            )
            .expect("script generation must succeed"),
            &ClusterBackend::Slurm,
            &plan.rules["align"].workdir,
        );
        assert_eq!(via_trait, direct);
    }

    #[test]
    fn parse_job_id_slurm_parsable() {
        assert_eq!(
            parse_job_id(&ClusterBackend::Slurm, "12345\n", "").unwrap(),
            "12345"
        );
    }

    #[test]
    fn parse_job_id_slurm_sentence_fallback() {
        assert_eq!(
            parse_job_id(&ClusterBackend::Slurm, "", "Submitted batch job 67890\n").unwrap(),
            "67890"
        );
    }

    #[test]
    fn parse_job_id_pbs_bare() {
        assert_eq!(
            parse_job_id(&ClusterBackend::Pbs, "777.queue\n", "").unwrap(),
            "777.queue"
        );
    }

    #[test]
    fn parse_job_id_sge_sentence() {
        assert_eq!(
            parse_job_id(
                &ClusterBackend::Sge,
                "Your job 4242 (\"align.sh\") has been submitted\n",
                ""
            )
            .unwrap(),
            "4242"
        );
    }

    #[test]
    fn parse_job_id_lsf_sentence() {
        assert_eq!(
            parse_job_id(
                &ClusterBackend::Lsf,
                "Job <9999> is submitted to queue <normal>.\n",
                ""
            )
            .unwrap(),
            "9999"
        );
    }

    #[test]
    fn parse_job_id_slurm_unparseable_is_error() {
        assert!(parse_job_id(&ClusterBackend::Slurm, "garbage", "also garbage").is_err());
    }

    /// Ledger RB-06: a SubmitJob response (default AWS CLI output) must
    /// yield the BARE job id — the whole JSON document is not a job id and
    /// would corrupt every describe/cancel and array child handle.
    #[test]
    fn parse_job_id_aws_extracts_the_json_job_id() {
        let response = "{\n  \"jobArn\": \"arn:aws:batch:job/abc\",\n  \
                        \"jobId\": \"abc-123\",\n  \"jobName\": \"oxoflow-x\"\n}";
        assert_eq!(
            parse_job_id(&ClusterBackend::AwsBatch, response, "").unwrap(),
            "abc-123"
        );
        // The bare `--query jobId --output text` shape still works.
        assert_eq!(
            parse_job_id(&ClusterBackend::AwsBatch, "abc-123\n", "").unwrap(),
            "abc-123"
        );
    }

    #[test]
    fn parse_status_line_slurm() {
        assert_eq!(
            parse_status_line(&ClusterBackend::Slurm, "12345|RUNNING"),
            Some(("12345".into(), BackendJobStatus::Running))
        );
        assert_eq!(
            parse_status_line(&ClusterBackend::Slurm, "12345_7|COMPLETED"),
            Some(("12345_7".into(), BackendJobStatus::Completed))
        );
        assert_eq!(
            parse_status_line(&ClusterBackend::Slurm, "12345|OUT_OF_MEMORY"),
            Some(("12345".into(), BackendJobStatus::Failed))
        );
        assert_eq!(
            parse_status_line(&ClusterBackend::Slurm, "12345|CANCELLED"),
            Some(("12345".into(), BackendJobStatus::Cancelled))
        );
        // Compact forms (%t): a hand-written  query must
        // not read live jobs as Unknown (cluster status used to report
        // RUNNING jobs as "finished, or an unknown id").
        assert_eq!(
            parse_status_line(&ClusterBackend::Slurm, "12345|R"),
            Some(("12345".into(), BackendJobStatus::Running))
        );
        assert_eq!(
            parse_status_line(&ClusterBackend::Slurm, "12345|PD"),
            Some(("12345".into(), BackendJobStatus::Pending))
        );
        assert_eq!(
            parse_status_line(&ClusterBackend::Slurm, "12345|CG"),
            Some(("12345".into(), BackendJobStatus::Running))
        );
        assert_eq!(
            parse_status_line(&ClusterBackend::Slurm, "12345|CD"),
            Some(("12345".into(), BackendJobStatus::Completed))
        );
        assert_eq!(
            parse_status_line(&ClusterBackend::Slurm, "12345|TO"),
            Some(("12345".into(), BackendJobStatus::Failed))
        );
        assert_eq!(
            parse_status_line(&ClusterBackend::Slurm, "12345|CA"),
            Some(("12345".into(), BackendJobStatus::Cancelled))
        );
        assert_eq!(
            parse_status_line(&ClusterBackend::Slurm, "12345|WEIRD"),
            Some(("12345".into(), BackendJobStatus::Unknown))
        );
        assert_eq!(
            parse_status_line(&ClusterBackend::Slurm, "no-pipe-here"),
            None
        );
    }

    #[test]
    fn parse_status_line_pbs() {
        // "Job id  Name  User  Time Use S Queue"
        let line = "777.queue   align.sh   me   0:00:00   R   batch";
        assert_eq!(
            parse_status_line(&ClusterBackend::Pbs, line),
            Some(("777.queue".into(), BackendJobStatus::Running))
        );
        let q_line = "778.queue   align.sh   me   0:00:00   Q   batch";
        assert_eq!(
            parse_status_line(&ClusterBackend::Pbs, q_line),
            Some(("778.queue".into(), BackendJobStatus::Pending))
        );
        // A live array container reports B while its elements run — the
        // job is alive, so it must not count toward blind settlement.
        let b_line = "779.queue   chunk      me   0:00:00   B   batch";
        assert_eq!(
            parse_status_line(&ClusterBackend::Pbs, b_line),
            Some(("779.queue".into(), BackendJobStatus::Running))
        );
    }

    #[test]
    fn parse_status_line_sge() {
        // The bare `qstat` table row (state in field 4) — what the
        // no-arguments `cluster status` listing reads.
        assert_eq!(
            parse_status_line(&ClusterBackend::Sge, "77  0.55500  align  me  r  batch"),
            Some(("77".into(), BackendJobStatus::Running))
        );
        assert_eq!(
            parse_status_line(&ClusterBackend::Sge, "78  0.55500  call  me  qw  batch"),
            Some(("78".into(), BackendJobStatus::Pending))
        );
        // A `qstat -j` job_number line stays (id, unknown); the state pairs
        // up in status_report's positional handling.
        assert_eq!(
            parse_status_line(&ClusterBackend::Sge, "job_number: 99"),
            Some(("99".into(), BackendJobStatus::Unknown))
        );
        // Neither shape.
        assert_eq!(parse_status_line(&ClusterBackend::Sge, "garbage"), None);
    }

    #[test]
    fn parse_status_line_lsf() {
        let line = "12345  me  RUN  batch  1  1  8  Aug 14 12:00";
        assert_eq!(
            parse_status_line(&ClusterBackend::Lsf, line),
            Some(("12345".into(), BackendJobStatus::Running))
        );
        let done = "12345  me  DONE  batch  1  1  8  Aug 14 12:00";
        assert_eq!(
            parse_status_line(&ClusterBackend::Lsf, done),
            Some(("12345".into(), BackendJobStatus::Completed))
        );
        // Suspend states follow IBM's definitions: USUSP suspended the job
        // while it was still pending (never started), PSUSP/SSUSP suspended
        // it after it started, and LSF 10.x merged all three into SUSP —
        // which in practice means "started, then held".
        for (stat, expected) in [
            ("PEND", BackendJobStatus::Pending),
            ("USUSP", BackendJobStatus::Pending),
            ("SSUSP", BackendJobStatus::Running),
            ("PSUSP", BackendJobStatus::Running),
            ("SUSP", BackendJobStatus::Running),
        ] {
            let line = format!("12345  me  {stat}  batch  1  1  8  Aug 14 12:00");
            assert_eq!(
                parse_status_line(&ClusterBackend::Lsf, &line),
                Some(("12345".into(), expected)),
                "{stat}: wrong mapping"
            );
        }
    }

    // ─── cluster-path audit findings ───────────────────────────────────────

    #[test]
    fn status_invocations_match_each_scheduler_cli() {
        // G1: ids went to squeue as separate arguments, which real SLURM
        // rejects — `-j` takes one comma-separated list.
        let ids = ["101".to_string(), "202".to_string()];
        assert_eq!(
            status_invocations(&ClusterBackend::Slurm, &ids),
            vec![(
                "squeue",
                vec![
                    "-j".to_string(),
                    "101,202".to_string(),
                    "--noheader".to_string(),
                    "-o".to_string(),
                    "%i|%T".to_string(),
                ]
            )]
        );
        // -x includes history: without it a finished job (esp. array
        // elements) makes the whole by-id listing exit 35.
        assert_eq!(
            status_invocations(&ClusterBackend::Pbs, &ids),
            vec![(
                "qstat",
                vec!["-x".to_string(), "101".to_string(), "202".to_string()]
            )]
        );
        // LSF polls with -a so DONE/EXIT rows stay visible; plain bjobs
        // answers only for PEND/RUN jobs and a finished job would vanish.
        assert_eq!(
            status_invocations(&ClusterBackend::Lsf, &ids),
            vec![(
                "bjobs",
                vec!["-a".to_string(), "101".to_string(), "202".to_string()]
            )]
        );
        // SGE answers one job per -j.
        assert_eq!(
            status_invocations(&ClusterBackend::Sge, &ids),
            vec![
                ("qstat", vec!["-j".to_string(), "101".to_string()]),
                ("qstat", vec!["-j".to_string(), "202".to_string()]),
            ]
        );
        // AWS Batch takes `--jobs` as a list: each id is its own argv
        // entry (a single space-joined argument is one invalid id).
        assert_eq!(
            status_invocations(&ClusterBackend::AwsBatch, &ids),
            vec![(
                "aws",
                vec![
                    "batch".to_string(),
                    "describe-jobs".to_string(),
                    "--jobs".to_string(),
                    "101".to_string(),
                    "202".to_string(),
                    "--query".to_string(),
                    "jobs[].join(|,[jobId,status])".to_string(),
                    "--output".to_string(),
                    "text".to_string(),
                ]
            )]
        );
    }

    #[test]
    fn my_jobs_invocations_scope_to_the_current_user() {
        // std::process::Command does no shell expansion, so `-u $USER` is
        // resolved from the environment at invocation-build time; SGE and
        // LSF answer a bare call for the invoking user.
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_default();
        assert!(!user.is_empty(), "test environment must have USER/LOGNAME");
        assert_eq!(
            my_jobs_invocations(&ClusterBackend::Slurm),
            vec![(
                "squeue",
                vec![
                    "-u".to_string(),
                    user.clone(),
                    "--noheader".to_string(),
                    "-o".to_string(),
                    "%i|%T".to_string(),
                ]
            )]
        );
        assert_eq!(
            my_jobs_invocations(&ClusterBackend::Pbs),
            vec![("qstat", vec!["-u".to_string(), user])]
        );
        assert_eq!(
            my_jobs_invocations(&ClusterBackend::Sge),
            vec![("qstat", Vec::new())]
        );
        assert_eq!(
            my_jobs_invocations(&ClusterBackend::Lsf),
            vec![("bjobs", Vec::new())]
        );
    }

    #[test]
    fn parse_job_listing_reads_rows_and_skips_headers() {
        let squeue = "101|PENDING\n202|RUNNING\n";
        assert_eq!(
            parse_job_listing(&ClusterBackend::Slurm, squeue),
            vec![
                ("101".to_string(), BackendJobStatus::Pending),
                ("202".to_string(), BackendJobStatus::Running),
            ]
        );
        // The qstat header's first field is the literal "Job id" — a
        // non-numeric id, so the header never becomes a job row.
        let qstat = "Job id      Name    User  Time Use S Queue\n\
                      404.queue   align   me    0:00   R batch\n\
                      405.queue   call    me    0:00   Q batch\n";
        assert_eq!(
            parse_job_listing(&ClusterBackend::Pbs, qstat),
            vec![
                ("404.queue".to_string(), BackendJobStatus::Running),
                ("405.queue".to_string(), BackendJobStatus::Pending),
            ]
        );
        // SGE's bare `qstat` table puts the state in field 4, like PBS.
        let qstat_sge = "job-ID  prior   name       user         state\n\
                         77      0.55500 align      me           r\n\
                         78      0.55500 call       me           qw\n";
        assert_eq!(
            parse_job_listing(&ClusterBackend::Sge, qstat_sge),
            vec![
                ("77".to_string(), BackendJobStatus::Running),
                ("78".to_string(), BackendJobStatus::Pending),
            ]
        );
        // And a `qstat -j` pairing stays unknown-per-number as before — the
        // table arm must not swallow it.
        assert_eq!(
            parse_job_listing(&ClusterBackend::Sge, "job_number: 99\n"),
            vec![("99".to_string(), BackendJobStatus::Unknown)]
        );
        let bjobs = "JOBID   USER    STAT  QUEUE\n\
                     12345   me      RUN   normal\n";
        assert_eq!(
            parse_job_listing(&ClusterBackend::Lsf, bjobs),
            vec![("12345".to_string(), BackendJobStatus::Running)]
        );
    }

    #[test]
    fn status_report_answers_every_requested_id() {
        // G2: the scheduler's own table came back raw. Parsed per request, a
        // job that already left the queue reads as unknown rather than
        // disappearing, and the qstat header row never becomes a job.
        let squeue = "202|RUNNING\n303|COMPLETED\n";
        assert_eq!(
            status_report(
                &ClusterBackend::Slurm,
                &["101".into(), "202".into()],
                squeue
            ),
            vec![
                ("101".to_string(), BackendJobStatus::Unknown),
                ("202".to_string(), BackendJobStatus::Running),
            ]
        );
        // A qstat header row parses to a bogus id; it must not be reported.
        let qstat = "Job id      Name    User  Time Use S Queue\n\
                      404.queue   align   me    0:00   R batch\n";
        assert_eq!(
            status_report(&ClusterBackend::Pbs, &["404.queue".to_string()], qstat),
            vec![("404.queue".to_string(), BackendJobStatus::Running)]
        );
    }

    #[test]
    fn finish_script_pins_the_working_directory_per_backend() {
        // G9: no script ever said where it runs. PBS starts jobs in $HOME and
        // SGE in a queue-configured directory, so relative outputs landed
        // outside the run directory.
        let cases = [
            (ClusterBackend::Slurm, "#SBATCH --chdir='/wf'"),
            // OpenPBS qsub has NO working-directory option (no -d — that is
            // Torque; no -w — that is PBS Pro), so the pin happens in the
            // script body (found live, issue #356).
            (ClusterBackend::Pbs, "cd '/wf' || exit 1"),
            (ClusterBackend::Sge, "#$ -wd '/wf'"),
            (ClusterBackend::Lsf, "#BSUB -cwd '/wf'"),
        ];
        for (backend, pin) in cases {
            let script = finish_script("#!/bin/bash\nset -e\ntrue\n", &backend, Path::new("/wf"));
            assert_eq!(
                script,
                format!("#!/bin/bash\n{pin}\nset -e\ntrue\n"),
                "{backend}: the working directory must be pinned"
            );
        }
    }

    #[test]
    fn array_element_ids_match_each_scheduler() {
        let exec = |backend| ClusterExecutor::new(backend, cluster_config());
        // SLURM composes `{base}_{index}` (squeue lists elements that way).
        assert_eq!(
            exec(ClusterBackend::Slurm).array_element_id("123", 2),
            "123_2"
        );
        // OpenPBS echoes the array as `38[]` and addresses elements `38[1]`.
        assert_eq!(
            exec(ClusterBackend::Pbs).array_element_id("38[].pbs-master", 1),
            "38[1].pbs-master"
        );
        // LSF echoes the bare array id and addresses elements `44[2]`.
        assert_eq!(exec(ClusterBackend::Lsf).array_element_id("44", 2), "44[2]");
        // AWS Batch child jobs are real ids with a zero-based array index.
        assert_eq!(
            exec(ClusterBackend::AwsBatch).array_element_id("abc-123", 2),
            "abc-123:1"
        );
        // GCP Batch tasks are zero-based; the engine-owned `{job}:{task}`
        // handle is resolved through `gcloud batch tasks list`.
        assert_eq!(
            exec(ClusterBackend::GcpBatch).array_element_id("oxoflow-array-qc-7", 3),
            "oxoflow-array-qc-7:2"
        );
    }

    /// Write an executable scheduler shim (`kubectl`, `aws`, `gcloud`)
    /// into `dir` so `ClusterExecutor::with_scheduler_dir` resolves it
    /// without touching the real CLIs (ledger RB-06 wire tests).
    #[cfg(unix)]
    fn write_scheduler_shim(dir: &Path, program: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(program);
        std::fs::write(&path, format!("#!/bin/bash\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Ledger RB-06: a Kubernetes Indexed Job's completedIndexes /
    /// failedIndexes status is settled per element — one element failing
    /// does not turn its successful siblings into failures. The single
    /// base probe is what makes this per-element evidence, not a copy.
    #[tokio::test]
    #[cfg(unix)]
    async fn kubernetes_indexed_elements_report_independent_statuses() {
        let runs = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let calls = runs.path().join("calls");
        write_scheduler_shim(
            bin.path(),
            "kubectl",
            &format!(
                "printf '%s\\n' \"$@\" >> {calls}\n\
                     if [ \"$1\" = get ] && [ \"$2\" = job ]; then\n\
                     echo '1|1|2|0|1|'\n\
                     fi\n\
                     exit 0",
                calls = calls.display()
            ),
        );
        let exec = ClusterExecutor::new(ClusterBackend::Kubernetes, cluster_config())
            .with_scheduler_dir(bin.path().to_path_buf());
        let statuses = exec
            .poll(&["arr_1".to_string(), "arr_2".to_string()])
            .await
            .unwrap();
        assert_eq!(
            statuses.get("arr_1"),
            Some(&BackendJobStatus::Completed),
            "completion index 0 is recorded completed: {statuses:?}"
        );
        assert_eq!(
            statuses.get("arr_2"),
            Some(&BackendJobStatus::Failed),
            "completion index 1 is recorded failed: {statuses:?}"
        );
        // Both verdicts come from ONE probe of the base Job, and it asks
        // for the per-index fields.
        let calls = std::fs::read_to_string(&calls).unwrap();
        let probes: Vec<&str> = calls
            .lines()
            .filter(|l| l.contains("jsonpath={.status.succeeded}"))
            .collect();
        assert_eq!(
            probes.len(),
            1,
            "both elements resolve from ONE base-Job probe: {calls}"
        );
        assert!(
            calls.contains("completedIndexes"),
            "the probe must read the per-index status: {calls}"
        );
    }

    /// Ledger RB-06: when a cluster does not publish failedIndexes, the
    /// failed element cannot be identified — it must be Unknown, never a
    /// fabricated failure pinned on an arbitrary index.
    #[tokio::test]
    #[cfg(unix)]
    async fn kubernetes_missing_failed_indexes_are_unknown_not_guessed() {
        let bin = tempfile::tempdir().unwrap();
        write_scheduler_shim(
            bin.path(),
            "kubectl",
            "if [ \"$1\" = get ] && [ \"$2\" = job ]; then echo '0|1|2|||'; fi\nexit 0",
        );
        let exec = ClusterExecutor::new(ClusterBackend::Kubernetes, cluster_config())
            .with_scheduler_dir(bin.path().to_path_buf());
        let statuses = exec
            .poll(&["arr_1".to_string(), "arr_2".to_string()])
            .await
            .unwrap();
        assert_eq!(
            statuses.get("arr_1"),
            Some(&BackendJobStatus::Unknown),
            "missing failedIndexes must not call element 1 failed: {statuses:?}"
        );
        assert_eq!(statuses.get("arr_2"), Some(&BackendJobStatus::Unknown));
    }

    /// A dead Indexed Job must not hold the driver hostage: with
    /// `backoffLimit: 0` and `parallelism` < count, one pod failure
    /// terminally fails the Job and the controller never creates the
    /// remaining pods, so their indexes appear in NEITHER status list.
    /// The probe reads the Job's terminal condition and settles those
    /// indexes as Failed — a `Running` verdict resets the driver's
    /// blind-settlement guard and real cluster runs have no poll budget.
    #[tokio::test]
    #[cfg(unix)]
    async fn kubernetes_dead_job_settles_its_unrecorded_elements() {
        let bin = tempfile::tempdir().unwrap();
        // Element 1 completed, element 2 failed, elements 3 and 4 never
        // started; the Job carries its terminal Failed condition.
        write_scheduler_shim(
            bin.path(),
            "kubectl",
            "if [ \"$1\" = get ] && [ \"$2\" = job ]; then echo '1|1|4|0|1|Failed'; fi\nexit 0",
        );
        let exec = ClusterExecutor::new(ClusterBackend::Kubernetes, cluster_config())
            .with_scheduler_dir(bin.path().to_path_buf());
        let statuses = exec
            .poll(&[
                "arr_1".to_string(),
                "arr_2".to_string(),
                "arr_3".to_string(),
                "arr_4".to_string(),
            ])
            .await
            .unwrap();
        assert_eq!(statuses.get("arr_1"), Some(&BackendJobStatus::Completed));
        assert_eq!(statuses.get("arr_2"), Some(&BackendJobStatus::Failed));
        assert_eq!(
            statuses.get("arr_3"),
            Some(&BackendJobStatus::Failed),
            "an unstarted element on a dead Job must settle, not report Running forever: {statuses:?}"
        );
        assert_eq!(statuses.get("arr_4"), Some(&BackendJobStatus::Failed));
    }

    /// Ledger RB-06: Kubernetes omits zero-valued counters, so they print
    /// as EMPTY fields. The fixture feeds the OLD space-delimited shape the
    /// pre-fix parser read — `succeeded` absent, `spec.completions` = 3 —
    /// so the old `split_whitespace()` column walk shifted `spec.completions`
    /// into the `succeeded` slot and fabricated success for a just-created
    /// Job. Fail-first: pre-fix this reads Completed; post-fix the pipe
    /// parser sees fewer than three cells and keeps the Job in flight.
    #[tokio::test]
    #[cfg(unix)]
    async fn kubernetes_unset_counters_do_not_shift_columns() {
        let bin = tempfile::tempdir().unwrap();
        write_scheduler_shim(
            bin.path(),
            "kubectl",
            "if [ \"$1\" = get ] && [ \"$2\" = job ]; then echo ' 3'; fi\nexit 0",
        );
        let exec = ClusterExecutor::new(ClusterBackend::Kubernetes, cluster_config())
            .with_scheduler_dir(bin.path().to_path_buf());
        let statuses = exec.poll(&["single-job".to_string()]).await.unwrap();
        assert_ne!(
            statuses.get("single-job"),
            Some(&BackendJobStatus::Completed),
            "an absent succeeded counter must not shift spec.completions into it \
             and fabricate success: {statuses:?}"
        );
        assert!(
            !statuses.contains_key("single-job"),
            "with no pipe cells the answer is not a status — the Job stays in flight: {statuses:?}"
        );
    }

    /// Ledger RB-06: an empty status answer is not a verdict — the job
    /// stays in flight (Unknown) instead of being fabricated as Running.
    #[tokio::test]
    #[cfg(unix)]
    async fn kubernetes_empty_status_answer_stays_unknown() {
        let bin = tempfile::tempdir().unwrap();
        write_scheduler_shim(
            bin.path(),
            "kubectl",
            "if [ \"$1\" = get ] && [ \"$2\" = job ]; then echo ''; fi\nexit 0",
        );
        let exec = ClusterExecutor::new(ClusterBackend::Kubernetes, cluster_config())
            .with_scheduler_dir(bin.path().to_path_buf());
        let statuses = exec.poll(&["single-job".to_string()]).await.unwrap();
        assert!(
            !statuses.contains_key("single-job"),
            "an empty status answer must stay absent (Unknown): {statuses:?}"
        );
    }

    /// Ledger RB-06: AWS Batch array elements are real child job ids and
    /// settle on their own rows. The `--jobs` list is passed as separate
    /// argv entries (one space-joined argument is a single invalid id).
    #[tokio::test]
    #[cfg(unix)]
    async fn aws_batch_child_jobs_report_independent_statuses() {
        let runs = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let args_log = runs.path().join("args");
        write_scheduler_shim(
            bin.path(),
            "aws",
            &format!(
                "printf '%s\\n' \"$@\" >> {args_log}\n\
                 echo 'arr:0|SUCCEEDED'\n\
                 echo 'arr:1|FAILED'\n\
                 exit 0",
                args_log = args_log.display()
            ),
        );
        let exec = ClusterExecutor::new(ClusterBackend::AwsBatch, cluster_config())
            .with_scheduler_dir(bin.path().to_path_buf());
        let statuses = exec
            .poll(&["arr:0".to_string(), "arr:1".to_string()])
            .await
            .unwrap();
        assert_eq!(statuses.get("arr:0"), Some(&BackendJobStatus::Completed));
        assert_eq!(statuses.get("arr:1"), Some(&BackendJobStatus::Failed));
        let argv: Vec<String> = std::fs::read_to_string(&args_log)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        let jobs_at = argv.iter().position(|a| a == "--jobs").unwrap();
        assert_eq!(
            &argv[jobs_at + 1..jobs_at + 3],
            &["arr:0".to_string(), "arr:1".to_string()],
            "each job id is its own argv entry: {argv:?}"
        );
    }

    /// The `--jobs` groups of one `aws` argv log, in call order. Shared by
    /// the wire tests that pin the DescribeJobs chunking contract.
    #[cfg(unix)]
    fn aws_jobs_groups(argv: &[String]) -> Vec<Vec<String>> {
        let mut groups = Vec::new();
        let mut i = 0;
        while i < argv.len() {
            if argv[i] == "--jobs" {
                let mut group = Vec::new();
                i += 1;
                while i < argv.len() && argv[i] != "--query" {
                    group.push(argv[i].clone());
                    i += 1;
                }
                groups.push(group);
            } else {
                i += 1;
            }
        }
        groups
    }

    /// Review of #1222 (ledger RB-06): `DescribeJobs` rejects a `--jobs`
    /// list over 100 ids outright, and the driver can legitimately hold
    /// 101+ elements in flight (`max_submitted` has no 100 ceiling and
    /// every element is its own child id). The poller must split the list
    /// into ≤100-id calls and merge the answers — one oversized call would
    /// fail, warn "treating ids as absent", and (AWS having no accounting
    /// store) leave every element to blind-settle as Failed despite
    /// AWS-side success.
    #[tokio::test]
    #[cfg(unix)]
    async fn aws_batch_poll_chunks_past_the_describe_jobs_100_id_cap() {
        const IDS: usize = 150; // two calls: 100 + 50
        let runs = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let args_log = runs.path().join("args");
        // Answer every id the call asked for, so merged results must cover
        // all 150 ids.
        write_scheduler_shim(
            bin.path(),
            "aws",
            &format!(
                "printf '%s\\n' \"$@\" >> {args_log}\n\
                 mode=0\n\
                 for a in \"$@\"; do\n\
                 if [ \"$a\" = \"--jobs\" ]; then mode=1; continue; fi\n\
                 if [ \"$a\" = \"--query\" ]; then mode=0; continue; fi\n\
                 if [ \"$mode\" = 1 ]; then echo \"$a|SUCCEEDED\"; fi\n\
                 done\n\
                 exit 0",
                args_log = args_log.display()
            ),
        );
        let exec = ClusterExecutor::new(ClusterBackend::AwsBatch, cluster_config())
            .with_scheduler_dir(bin.path().to_path_buf());
        let ids: Vec<String> = (0..IDS).map(|i| format!("elem:{i}")).collect();
        let statuses = exec.poll(&ids).await.unwrap();
        assert_eq!(
            statuses.len(),
            IDS,
            "every id must resolve across the merged chunk answers"
        );
        for id in &ids {
            assert_eq!(
                statuses.get(id),
                Some(&BackendJobStatus::Completed),
                "{id} must settle on its own row despite the chunking"
            );
        }
        let argv: Vec<String> = std::fs::read_to_string(&args_log)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        let groups = aws_jobs_groups(&argv);
        assert_eq!(
            groups.len(),
            2,
            "150 ids must split into two describe calls: {groups:?}"
        );
        assert_eq!(groups[0].len(), AWS_DESCRIBE_JOBS_MAX_IDS);
        assert_eq!(groups[1].len(), IDS - AWS_DESCRIBE_JOBS_MAX_IDS);
        for group in &groups {
            assert!(
                group.len() <= AWS_DESCRIBE_JOBS_MAX_IDS,
                "no describe call may exceed the 100-id cap: {}",
                group.len()
            );
        }
    }

    /// The standalone `cluster status` command builds its invocations from
    /// the same id list — the chunking contract must hold there too.
    #[test]
    fn aws_status_invocations_chunk_past_the_describe_jobs_cap() {
        let ids: Vec<String> = (0..AWS_DESCRIBE_JOBS_MAX_IDS + 1)
            .map(|i| format!("elem:{i}"))
            .collect();
        let invocations = status_invocations(&ClusterBackend::AwsBatch, &ids);
        assert_eq!(
            invocations.len(),
            2,
            "101 ids must produce two describe calls: {}",
            invocations.len()
        );
        let (program, first) = &invocations[0];
        assert_eq!(*program, "aws");
        let groups: Vec<Vec<String>> = aws_jobs_groups(first);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), AWS_DESCRIBE_JOBS_MAX_IDS);
        let (_, second) = &invocations[1];
        let groups: Vec<Vec<String>> = aws_jobs_groups(second);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 1, "the remainder id rides its own call");
    }

    /// Ledger RB-06: GCP Batch array elements resolve through the job's
    /// task list, one status per task index.
    #[tokio::test]
    #[cfg(unix)]
    async fn gcp_batch_tasks_report_independent_statuses() {
        let bin = tempfile::tempdir().unwrap();
        write_scheduler_shim(
            bin.path(),
            "gcloud",
            "echo 'projects/p/locations/l/jobs/arr/taskGroups/group0/tasks/0\tSUCCEEDED'\n\
             echo 'projects/p/locations/l/jobs/arr/taskGroups/group0/tasks/1\tFAILED'\n\
             exit 0",
        );
        let exec = ClusterExecutor::new(ClusterBackend::GcpBatch, cluster_config())
            .with_scheduler_dir(bin.path().to_path_buf());
        let statuses = exec
            .poll(&["arr:0".to_string(), "arr:1".to_string()])
            .await
            .unwrap();
        assert_eq!(statuses.get("arr:0"), Some(&BackendJobStatus::Completed));
        assert_eq!(statuses.get("arr:1"), Some(&BackendJobStatus::Failed));
    }

    /// Ledger RB-06: `aws batch submit-job` must return the BARE job id —
    /// it is the handle the driver keys tracking on and the array child
    /// ids `{id}:{n}` extend. The CLI is asked for exactly that shape.
    #[tokio::test]
    #[cfg(unix)]
    async fn aws_batch_submit_returns_the_bare_job_id() {
        let runs = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let args_log = runs.path().join("args");
        write_scheduler_shim(
            bin.path(),
            "aws",
            &format!(
                "printf '%s\\n' \"$@\" >> {}\necho 'abc-123'\nexit 0",
                args_log.display()
            ),
        );
        let spec = runs.path().join("job.json");
        std::fs::write(&spec, "{}").unwrap();
        let exec = ClusterExecutor::new(ClusterBackend::AwsBatch, cluster_config())
            .with_scheduler_dir(bin.path().to_path_buf());
        let id = exec.submit(&spec).await.unwrap();
        assert_eq!(id, "abc-123");
        let argv = std::fs::read_to_string(&args_log).unwrap();
        assert!(
            argv.contains("--query\njobId\n--output\ntext"),
            "the submit must ask for the bare id, not the response JSON: {argv:?}"
        );
    }

    /// Ledger RB-06: `gcloud batch jobs submit <id>` prints the submitted
    /// Job resource, not the id; `submit` must return the minted id it
    /// passed positionally, or every describe/cancel and per-element task
    /// handle is keyed on YAML.
    #[tokio::test]
    #[cfg(unix)]
    async fn gcp_batch_submit_returns_the_minted_job_id() {
        let runs = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        write_scheduler_shim(
            bin.path(),
            "gcloud",
            "printf 'name: projects/p/locations/l/jobs/whatever\\nstatus:\\n  state: QUEUED\\n'\n\
             exit 0",
        );
        let job_dir = runs.path().join("jobs/trim");
        std::fs::create_dir_all(&job_dir).unwrap();
        let spec = job_dir.join("job.json");
        std::fs::write(&spec, "{}").unwrap();
        let exec = ClusterExecutor::new(ClusterBackend::GcpBatch, cluster_config())
            .with_scheduler_dir(bin.path().to_path_buf());
        let id = exec.submit(&spec).await.unwrap();
        assert!(id.starts_with("oxoflow-trim-"), "minted id expected: {id}");
        assert!(
            !id.contains('\n') && !id.contains("status:"),
            "the job resource must never become the job id: {id}"
        );
    }

    /// Ledger RB-06: an empty / unrecognised `gcloud` state is Unknown,
    /// never Pending (which polls forever) and never success.
    #[tokio::test]
    #[cfg(unix)]
    async fn gcp_batch_missing_state_is_unknown_not_pending() {
        let bin = tempfile::tempdir().unwrap();
        write_scheduler_shim(
            bin.path(),
            "gcloud",
            "if [ \"$1\" = batch ] && [ \"$2\" = jobs ]; then echo 'SOMETHING_NEW'; fi\nexit 0",
        );
        let exec = ClusterExecutor::new(ClusterBackend::GcpBatch, cluster_config())
            .with_scheduler_dir(bin.path().to_path_buf());
        let statuses = exec.poll(&["solo-job".to_string()]).await.unwrap();
        assert_eq!(
            statuses.get("solo-job"),
            Some(&BackendJobStatus::Unknown),
            "an unrecognised state is missing data, not a live job: {statuses:?}"
        );
    }

    #[test]
    fn pbs_body_cd_lands_after_the_directive_block() {
        // qsub stops parsing `#PBS` directives at the first non-directive
        // line, so the body `cd` pin must follow — not precede — them.
        let script = finish_script(
            "#!/bin/bash\n#PBS -N prep\n#PBS -l nodes=1:ppn=1\nset -e\ntrue\n",
            &ClusterBackend::Pbs,
            Path::new("/wf"),
        );
        assert_eq!(
            script,
            "#!/bin/bash\n#PBS -N prep\n#PBS -l nodes=1:ppn=1\ncd '/wf' || exit 1\nset -e\ntrue\n",
            "{script}"
        );
    }

    #[test]
    fn finish_script_resolves_a_relative_workdir() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let script = finish_script(
            "#!/bin/bash\ntrue\n",
            &ClusterBackend::Slurm,
            nested.as_path(),
        );
        let expected = format!(
            "#!/bin/bash\n#SBATCH --chdir='{}'\ntrue\n",
            nested.canonicalize().unwrap().display()
        );
        assert_eq!(script, expected);
    }

    #[test]
    fn array_script_pins_workdir_without_duplicating_body_lines() {
        // G9 + P7-13: the array renderer re-appends `set -e` and
        // `mkdir -p logs` after the base script already emitted them.
        let exec = ClusterExecutor::new(ClusterBackend::Slurm, cluster_config())
            .with_workdir(PathBuf::from("/wf"));
        let rule = crate::rule::Rule {
            name: "align".to_string(),
            ..crate::rule::Rule::default()
        };
        let script = exec
            .render_array_script(&rule, "/run/jobs/align/chunk-1", 2, &[])
            .unwrap();
        assert_eq!(
            script.matches("set -e").count(),
            1,
            "one set -e only: {script}"
        );
        assert_eq!(
            script.matches("mkdir -p logs").count(),
            1,
            "one mkdir -p logs only: {script}"
        );
        assert!(
            script.contains("#SBATCH --chdir='/wf'\n"),
            "the array pins the working directory too: {script}"
        );
        assert!(
            script.starts_with("#!/bin/bash\n#SBATCH --chdir='/wf'\n#SBATCH --array=1-2\n"),
            "the array range stays right after the new chdir directive: {script}"
        );
    }

    #[test]
    fn script_log_paths_reads_every_backend_directive() {
        let slurm = "#!/bin/bash\n\
                     #SBATCH --output=logs/align.out\n\
                     #SBATCH --error=logs/align.err\n";
        assert_eq!(
            script_log_paths(slurm),
            vec![
                PathBuf::from("logs/align.out"),
                PathBuf::from("logs/align.err")
            ]
        );
        // Array forms: the per-element pattern is one shared parent dir.
        let array = "#SBATCH --output=logs/align.%A_%a.out\n#SBATCH --error=logs/align.%A_%a.err\n";
        assert_eq!(script_log_paths(array).len(), 2);

        let pbs = "#PBS -o logs/fastqc.out\n#PBS -e logs/fastqc.err\n";
        assert_eq!(script_log_paths(pbs).len(), 2);
        let sge = "#$ -o logs/align.out\n#$ -e logs/align.err\n";
        assert_eq!(script_log_paths(sge).len(), 2);
        let lsf = "#BSUB -o logs/sort.%I.out\n#BSUB -e logs/sort.%I.err\n";
        assert_eq!(script_log_paths(lsf).len(), 2);

        // Body lines that merely mention the directives stay untouched.
        assert!(script_log_paths("# echo #SBATCH --output=x\ntrue\n").is_empty());
    }

    #[test]
    fn parse_array_range_reads_the_straight_slurm_range() {
        assert_eq!(
            parse_array_range("#!/bin/bash\n#SBATCH --array=1-3\ntrue\n"),
            Some((1, 3))
        );
        // A %throttle bounds concurrency, not the index set: it must not
        // block a 1:1 match with an unthrottled downstream.
        assert_eq!(parse_array_range("#SBATCH --array=1-20%4\n"), Some((1, 20)));
        assert_eq!(parse_array_range("#SBATCH --array=0-9 \n"), Some((0, 9)));
    }

    #[test]
    fn parse_array_range_rejects_every_non_straight_range() {
        // No array directive at all.
        assert_eq!(parse_array_range("#!/bin/bash\necho hi\n"), None);
        // Comma lists and step forms are not one straight range.
        assert_eq!(parse_array_range("#SBATCH --array=1,3,5\n"), None);
        assert_eq!(parse_array_range("#SBATCH --array=0-15:4\n"), None);
        // Reversed or malformed ranges.
        assert_eq!(parse_array_range("#SBATCH --array=3-1\n"), None);
        assert_eq!(parse_array_range("#SBATCH --array=1-\n"), None);
        // Two directives: which one the scheduler honours is ambiguous —
        // refuse rather than guess.
        assert_eq!(
            parse_array_range("#SBATCH --array=1-3\n#SBATCH --array=4-6\n"),
            None
        );
        // sbatch honours directives only at column 0; an indented line is a
        // shell comment and must NOT read as an array declaration.
        assert_eq!(parse_array_range("  #SBATCH --array=1-3\n"), None);
        // The short -a spelling and other backends' directives are not
        // recognised (they conservatively fall back to afterok downstream).
        assert_eq!(parse_array_range("#SBATCH -a 1-3\n"), None);
        assert_eq!(parse_array_range("#PBS -J 1-3\n"), None);
    }

    #[test]
    fn elementwise_aftercorr_requires_matching_ranges_on_both_ends() {
        let up = "#!/bin/bash\n#SBATCH --array=1-3\n";
        // The straight chain: same start, same end.
        assert!(elementwise_aftercorr(up, "#SBATCH --array=1-3\n"));
        // Throttles differ but the index ranges match.
        assert!(elementwise_aftercorr("#SBATCH --array=1-3%2\n", up));
        // Mismatched end or start: the element-wise pairing would strand
        // indexes — fall back.
        assert!(!elementwise_aftercorr(up, "#SBATCH --array=1-4\n"));
        assert!(!elementwise_aftercorr(up, "#SBATCH --array=2-3\n"));
        // Scalar↔array mixes, both directions.
        assert!(!elementwise_aftercorr("true\n", up));
        assert!(!elementwise_aftercorr(up, "true\n"));
        // Both non-array: nothing to pair.
        assert!(!elementwise_aftercorr("true\n", "true\n"));
    }

    #[test]
    fn ensure_log_dirs_creates_the_referenced_directory_before_submit() {
        // Regression: a fresh run dir with no `logs/` made every backend's
        // job die at launch — the scheduler opens the output file before the
        // script body's `mkdir -p logs` runs.
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("fresh-run");
        std::fs::create_dir(&run_dir).unwrap();
        let script =
            "#!/bin/bash\n#SBATCH --output=logs/align.out\n#SBATCH --error=logs/align.err\n";
        ensure_log_dirs(script, &run_dir);
        assert!(
            run_dir.join("logs").is_dir(),
            "logs/ must exist before the scheduler opens the file"
        );

        // Relative workdir resolves against the process cwd, matching the
        // scheduler's --chdir handling.
        ensure_log_dirs(script, Path::new("."));
        // no return value; must not panic on a relative workdir
    }

    #[test]
    fn ensure_log_dirs_tolerates_an_unreadable_or_missing_script() {
        // submit() reads the script best-effort: a missing file must not
        // block submission (the scheduler's own error is the truthful one).
        let dir = tempfile::tempdir().unwrap();
        ensure_log_dirs("", dir.path()); // empty script → no dirs
        std::fs::set_permissions(
            dir.path(),
            std::os::unix::fs::PermissionsExt::from_mode(0o555),
        )
        .unwrap();
        ensure_log_dirs("#SBATCH --output=logs/x.out\n", dir.path()); // read-only base
        std::fs::set_permissions(
            dir.path(),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
    }

    /// Sentinel regression (ledger RB-12), per HPC backend: a rendered
    /// submit script for a routed command carries the
    /// `OXO_FLOW_SENSITIVE_<n>` reference, never the value — the value
    /// travels in the driver's private env file, and the script-based
    /// backends declare that channel (`uses_secret_env_file`).
    #[test]
    fn hpc_scripts_render_the_reference_not_the_sentinel_for_every_scheduler() {
        let sentinel = "SENTINEL_RENDER_VALUE_Q4";
        let mut plan = single_rule_plan();
        // Put the sentinel into the command, then route it through the
        // shared builder so the shell_cmd carries the reference (the
        // production ordering: route, then render the script).
        plan.rules.get_mut("align").unwrap().shell_cmd =
            format!("bwa mem ref.fa in.fq -R {sentinel} > out.bam");
        let bindings = crate::executor::process::sensitive_bindings(&[sentinel.to_string()]);
        let routed = crate::executor::process::route_sensitive_values_with_bindings(
            &plan.rules["align"].shell_cmd,
            &bindings,
        )
        .0;
        assert!(
            !routed.contains(sentinel) && routed.contains("${OXO_FLOW_SENSITIVE_0}"),
            "precondition: routing must swap the value for a reference: {routed}"
        );
        plan.rules.get_mut("align").unwrap().shell_cmd = routed;

        for backend in [
            ClusterBackend::Slurm,
            ClusterBackend::Pbs,
            ClusterBackend::Sge,
            ClusterBackend::Lsf,
        ] {
            let exec = ClusterExecutor::new(backend, cluster_config());
            assert!(
                exec.uses_secret_env_file(),
                "{backend} must declare the private env-file channel"
            );
            let script = exec.render_script(&plan.rules["align"]).unwrap();
            assert!(
                !script.contains(sentinel),
                "{backend} script leaked the sentinel: {script}"
            );
            assert!(
                script.contains("${OXO_FLOW_SENSITIVE_0}"),
                "{backend} script must carry the routed reference: {script}"
            );
        }

        // Cloud backends do not use the env file: their bodies carry the
        // reference into the backend's own secret channel.
        for backend in [
            ClusterBackend::Kubernetes,
            ClusterBackend::AwsBatch,
            ClusterBackend::GcpBatch,
        ] {
            let exec = ClusterExecutor::new(backend, cluster_config());
            assert!(
                !exec.uses_secret_env_file(),
                "{backend} must not declare the env-file channel"
            );
        }
    }
}

#[cfg(test)]
mod accounting_tests {
    use super::*;

    #[test]
    fn parses_scontrol_completed_record() {
        // Issue #244: the no-slurmdbd fallback. Real `scontrol show job`
        // output (whitespace-separated key=value pairs).
        let text = "JobId=12345 JobName=align\n\
   UserId=ops(1000) GroupId=ops(1000) MCS_label=N/A\n\
   Priority=4294901759 Nice=0 Account=(null) QOS=normal\n\
   JobState=COMPLETED Reason=(None) Dependency=(null)\n\
   ExitCode=0:0 RunTime=00:01:22 TimeLimit=02:00:00\n";
        assert_eq!(
            parse_scontrol(text),
            Some(TerminalRecord {
                status: BackendJobStatus::Completed,
                exit_code: Some(0),
                ..TerminalRecord::default()
            })
        );

        // A failed job reports the signal-aware exit code.
        let failed = "JobId=12346 JobName=align\n   JobState=FAILED Reason=NonZeroExitCode\n   ExitCode=1:0\n";
        assert_eq!(
            parse_scontrol(failed),
            Some(TerminalRecord {
                status: BackendJobStatus::Failed,
                exit_code: Some(1),
                ..TerminalRecord::default()
            })
        );

        // A CANCELLED job (state may carry the reason in parentheses).
        let cancelled = "JobId=12347 JobName=x\n   JobState=CANCELLED+(Reason)\n   ExitCode=0:15\n";
        assert_eq!(
            parse_scontrol(cancelled),
            Some(TerminalRecord {
                status: BackendJobStatus::Cancelled,
                exit_code: Some(143),
                ..TerminalRecord::default()
            })
        );

        // A RUNNING record is NOT terminal — the live poller owns those.
        let running = "JobId=12348 JobName=x\n   JobState=RUNNING\n   ExitCode=0:0\n";
        assert_eq!(parse_scontrol(running), None);
    }

    #[test]
    fn parses_sacct_terminal_lines() {
        // Arrange — real sacct
        // --format=JobID,State,ExitCode,Elapsed,MaxRSS,TotalCPU
        // (State is the second column in that format order).
        let text = "JobID    State      ExitCode     Elapsed    MaxRSS  TotalCPU\n\
------------ ---------- -------- ---------- ---------- ----------\n\
12345    COMPLETED      0:0        00:01:22    128.2M   00:02:44\n";

        // Act / Assert
        assert_eq!(
            parse_accounting(&ClusterBackend::Slurm, text),
            Some(TerminalRecord {
                status: BackendJobStatus::Completed,
                exit_code: Some(0),
                elapsed_secs: Some(82),
                max_rss_mb: Some(128),
                cpu_seconds: Some(164),
            })
        );
        // A non-zero exit and a multi-day walltime, written out rather than
        // substituted: "0:0" is a substring of "00:01:22".
        let failed = "JobID    State      ExitCode     Elapsed    MaxRSS  TotalCPU\n\
------------ ---------- -------- ---------- ---------- ----------\n\
12345    FAILED         7:0        1-02:00:00  128.2M   00:02:44\n";
        assert_eq!(
            parse_accounting(&ClusterBackend::Slurm, failed),
            Some(TerminalRecord {
                status: BackendJobStatus::Failed,
                exit_code: Some(7),
                elapsed_secs: Some(93_600),
                max_rss_mb: Some(128),
                cpu_seconds: Some(164),
            })
        );
        // The mock scheduler pipes the same columns.
        let piped = "JobID|State|ExitCode|Elapsed|MaxRSS|TotalCPU\n\
12345|COMPLETED|0:0|00:00:05|1234K|00:00:04\n";
        assert_eq!(
            parse_accounting(&ClusterBackend::Slurm, piped),
            Some(TerminalRecord {
                status: BackendJobStatus::Completed,
                exit_code: Some(0),
                elapsed_secs: Some(5),
                max_rss_mb: Some(1),
                cpu_seconds: Some(4),
            })
        );
    }

    #[test]
    fn sacct_layout_comes_from_the_header_row() {
        // The tx-ubuntu mock scheduler prints its own fixed column order —
        // JobID,Elapsed,State,ExitCode,MaxRSS — regardless of the requested
        // `--format`. Positional parsing read Elapsed as the state, matched
        // nothing, and left every finished job Unknown in `cluster status`.
        // Verbatim rows from that store.
        let text = "       JobID    Elapsed      State ExitCode    MaxRSS\n\
4813        00:01:22  COMPLETED      0:0    128.2M\n";
        assert_eq!(
            parse_accounting(&ClusterBackend::Slurm, text),
            Some(TerminalRecord {
                status: BackendJobStatus::Completed,
                exit_code: Some(0),
                elapsed_secs: Some(82),
                max_rss_mb: Some(128),
                cpu_seconds: None,
            })
        );
        // Sacct format-name variants (`JobIDRaw` on raw stores, `JobIDOK` on
        // completed-only views) must keep matching as header rows — main's
        // blanket `starts_with("JobID")` filter treated them as headers, and
        // an exact-equality check here would regress those stores.
        let raw = "       JobIDRaw    Elapsed      State ExitCode    MaxRSS\n\
4813        00:01:22  COMPLETED      0:0    128.2M\n";
        assert_eq!(
            parse_accounting(&ClusterBackend::Slurm, raw),
            Some(TerminalRecord {
                status: BackendJobStatus::Completed,
                exit_code: Some(0),
                elapsed_secs: Some(82),
                max_rss_mb: Some(128),
                cpu_seconds: None,
            })
        );
    }

    #[test]
    fn sacct_max_rss_comes_from_the_step_rows() {
        // Arrange — real sacct reports MaxRSS only on the step rows; the
        // allocation row leaves it blank. Reading the first row alone (what
        // this did) always reported no memory at all.
        let text = "JobID|State|ExitCode|Elapsed|MaxRSS|TotalCPU\n\
12345|COMPLETED|0:0|00:01:00||\n\
12345.batch|COMPLETED|0:0|00:01:00|2G|00:00:30\n\
12345.extern|COMPLETED|0:0|00:01:00|4K|00:00:00\n";

        // Act
        let rec = parse_accounting(&ClusterBackend::Slurm, text).unwrap();

        // Assert — state and exit code from the allocation row, peak memory
        // as the max over every row (not the first, and not the last).
        assert_eq!(rec.status, BackendJobStatus::Completed);
        assert_eq!(rec.elapsed_secs, Some(60));
        assert_eq!(rec.max_rss_mb, Some(2048));
    }

    #[test]
    fn sacct_states_match_the_live_poller() {
        // A job must settle the same way whether squeue still had it or it
        // had to come from sacct — same vocabulary, same mapping.
        let row = |state: &str| format!("12345|{state}|0:0|00:00:05||\n");
        for state in [
            "FAILED",
            "TIMEOUT",
            "OUT_OF_MEMORY",
            "NODE_FAIL",
            "BOOT_FAIL",
        ] {
            let rec = parse_accounting(&ClusterBackend::Slurm, &row(state)).unwrap();
            assert_eq!(rec.status, BackendJobStatus::Failed, "{state}");
        }
        for state in ["CANCELLED by 1001", "PREEMPTED"] {
            let rec = parse_accounting(&ClusterBackend::Slurm, &row(state)).unwrap();
            assert_eq!(rec.status, BackendJobStatus::Cancelled, "{state}");
        }
        // Non-terminal states keep the job in flight.
        for state in ["RUNNING", "PENDING"] {
            assert_eq!(parse_accounting(&ClusterBackend::Slurm, &row(state)), None);
        }
    }

    #[test]
    fn sacct_signalled_job_does_not_report_a_clean_exit() {
        // Arrange — an OOM kill reports exit component 0 with signal 9.
        // Reading only the exit component records a kill as success.
        let text = "JobID|State|ExitCode|Elapsed|MaxRSS|TotalCPU\n\
12345|OUT_OF_MEMORY|0:9|00:00:30|32000M|00:00:25\n";

        // Act
        let rec = parse_accounting(&ClusterBackend::Slurm, text).unwrap();

        // Assert — the shell's 128 + signum, and a failed state.
        assert_eq!(rec.status, BackendJobStatus::Failed);
        assert_eq!(rec.exit_code, Some(137));
    }

    #[test]
    fn parses_scheduler_durations_and_memory_figures() {
        // Arrange / Act / Assert — the formats these four stores emit.
        assert_eq!(parse_accounting_duration_secs("00:00:05"), Some(5));
        assert_eq!(parse_accounting_duration_secs("01:02:03"), Some(3723));
        assert_eq!(parse_accounting_duration_secs("2-00:00:00"), Some(172_800));
        assert_eq!(parse_accounting_duration_secs("04:30"), Some(270));
        assert_eq!(parse_accounting_duration_secs("82"), Some(82));
        // SLURM TotalCPU carries sub-second precision; it truncates.
        assert_eq!(parse_accounting_duration_secs("00:00:04.500"), Some(4));
        assert_eq!(parse_accounting_duration_secs(""), None);
        assert_eq!(parse_accounting_duration_secs("UNLIMITED"), None);

        assert_eq!(parse_rss_mb("1048576K"), Some(1024));
        assert_eq!(parse_rss_mb("128.2M"), Some(128));
        assert_eq!(parse_rss_mb("2G"), Some(2048));
        // SLURM's per-node / per-core suffixes.
        assert_eq!(parse_rss_mb("512Mn"), Some(512));
        // PBS writes kilobytes with a unit; SGE's ru_maxrss is bare KB.
        assert_eq!(parse_rss_mb("2048kb"), Some(2));
        assert_eq!(parse_rss_mb("4096"), Some(4));
        assert_eq!(parse_rss_mb(""), None);
        assert_eq!(parse_rss_mb("0"), None);
    }

    #[test]
    fn parses_pbs_qstat_x_terminal_blocks() {
        // Arrange — qstat -x -f job block with the resources_used fields a
        // completed job carries.
        let ok = "Job Id: 12345.vm\n    Job_Name = fastqc\n    job_state = C\n    \
Exit_status = 0\n    resources_used.walltime = 00:02:00\n    \
resources_used.mem = 65536kb\n    resources_used.cput = 00:03:30\n";
        assert_eq!(
            parse_accounting(&ClusterBackend::Pbs, ok),
            Some(TerminalRecord {
                status: BackendJobStatus::Completed,
                exit_code: Some(0),
                elapsed_secs: Some(120),
                max_rss_mb: Some(64),
                cpu_seconds: Some(210),
            })
        );
        let bad = ok.replace("Exit_status = 0", "Exit_status = 1");
        let rec = parse_accounting(&ClusterBackend::Pbs, &bad).unwrap();
        assert_eq!(rec.status, BackendJobStatus::Failed);
        assert_eq!(rec.exit_code, Some(1));
        // OpenPBS 23 history marks finished jobs "F" instead of "C".
        let f = ok.replace("job_state = C", "job_state = F");
        let rec = parse_accounting(&ClusterBackend::Pbs, &f).unwrap();
        assert_eq!(rec.status, BackendJobStatus::Completed);
        // "E" without an Exit_status yet is transient — not terminal.
        let mut exiting_lines: Vec<String> = ok
            .lines()
            .filter(|l| !l.contains("Exit_status"))
            .map(String::from)
            .collect();
        exiting_lines[2] = "    job_state = E".into();
        let exiting = exiting_lines.join("\n");
        assert_eq!(parse_accounting(&ClusterBackend::Pbs, &exiting), None);
        // Still running: no terminal state.
        let running = ok.replace("job_state = C", "job_state = R");
        assert_eq!(parse_accounting(&ClusterBackend::Pbs, &running), None);
    }

    #[test]
    fn parses_qacct_exit_status() {
        // Arrange — qacct -j output
        let ok = "qname        all.q\njobnumber    12345\nexit_status  0\n\
ru_wallclock 82\nru_maxrss    262144\ncpu          164\n";
        assert_eq!(
            parse_accounting(&ClusterBackend::Sge, ok),
            Some(TerminalRecord {
                status: BackendJobStatus::Completed,
                exit_code: Some(0),
                elapsed_secs: Some(82),
                max_rss_mb: Some(256),
                cpu_seconds: Some(164),
            })
        );
        let bad = ok.replace("exit_status  0", "exit_status  137");
        let rec = parse_accounting(&ClusterBackend::Sge, &bad).unwrap();
        assert_eq!(rec.status, BackendJobStatus::Failed);
        assert_eq!(rec.exit_code, Some(137));
    }

    #[test]
    fn parses_bacct_status_lines() {
        // Arrange — bacct output carries a Status <...> line. LSF's
        // measurement columns vary between versions, so this backend stays
        // state-only rather than guessing.
        let done = "Job <12345>, User <bioinf>, Status <DONE>, Queue <normal>, Command <fastqc>\n";
        assert_eq!(
            parse_accounting(&ClusterBackend::Lsf, done),
            Some(TerminalRecord::status_only(BackendJobStatus::Completed))
        );
        let exited = done.replace("DONE", "EXIT");
        assert_eq!(
            parse_accounting(&ClusterBackend::Lsf, &exited),
            Some(TerminalRecord::status_only(BackendJobStatus::Failed))
        );
        let running = done.replace("Status <DONE>", "Status <RUN>");
        assert_eq!(parse_accounting(&ClusterBackend::Lsf, &running), None);
    }

    #[test]
    fn sge_state_table_is_shared_across_read_paths() {
        // The three SGE read paths (live table, `qstat -j` positional
        // pairing, driver poll) all route through one `sge_state` table —
        // the hand-copied versions had already drifted apart (`dr` was
        // Completed in the table, Unknown in `cluster status`).
        assert_eq!(sge_state("r"), BackendJobStatus::Running);
        assert_eq!(sge_state("t"), BackendJobStatus::Running);
        assert_eq!(sge_state("qw"), BackendJobStatus::Pending);
        assert_eq!(sge_state("h"), BackendJobStatus::Pending);
        assert_eq!(sge_state("Rq"), BackendJobStatus::Pending);
        assert_eq!(sge_state("P"), BackendJobStatus::Pending);
        assert_eq!(sge_state("d"), BackendJobStatus::Completed);
        assert_eq!(sge_state("dr"), BackendJobStatus::Completed);
        assert_eq!(sge_state("dt"), BackendJobStatus::Completed);
        assert_eq!(sge_state("dR"), BackendJobStatus::Completed);
        assert_eq!(sge_state("Eqw"), BackendJobStatus::Failed);
        assert_eq!(sge_state("E"), BackendJobStatus::Failed);
        assert_eq!(sge_state("weird"), BackendJobStatus::Unknown);

        // The `qstat -j` positional pairing path reports the same verdicts
        // as the table path for the same state word.
        let report = status_report(
            &ClusterBackend::Sge,
            &["42".to_string()],
            "job_number: 42\nstate: dr\n",
        );
        assert_eq!(report[0].1, BackendJobStatus::Completed);
    }

    #[test]
    fn parse_status_line_reads_batched_aws_describe_rows() {
        // The driver poll renders `id|STATE` rows via
        // `--query jobs[].join(|,[jobId,status])`; the shared parser must
        // read the same shape the standalone `cluster status` path emits.
        let (id, st) = parse_status_line(&ClusterBackend::AwsBatch, "abc-123|SUCCEEDED").unwrap();
        assert_eq!(id, "abc-123");
        assert_eq!(st, BackendJobStatus::Completed);
        assert_eq!(
            parse_status_line(&ClusterBackend::AwsBatch, "abc-123|FAILED")
                .unwrap()
                .1,
            BackendJobStatus::Failed
        );
        assert_eq!(
            parse_status_line(&ClusterBackend::AwsBatch, "abc-123|RUNNABLE")
                .unwrap()
                .1,
            BackendJobStatus::Pending
        );
    }

    #[test]
    fn parse_status_line_reads_k8s_jsonpath_and_gcp_tab_rows() {
        // `kubectl get job -o jsonpath={.metadata.name}|{...type}` emits
        // pipe rows with a condition word, and gcloud's
        // --format=value(name,state) emits tab rows — both must route
        // through the shared parser.
        let (id, st) =
            parse_status_line(&ClusterBackend::Kubernetes, "oxoflow-align|Complete").unwrap();
        assert_eq!(id, "oxoflow-align");
        assert_eq!(st, BackendJobStatus::Completed);
        assert_eq!(
            parse_status_line(&ClusterBackend::Kubernetes, "oxoflow-align|Failed")
                .unwrap()
                .1,
            BackendJobStatus::Failed
        );
        // No conditions yet: empty state parses as Unknown, not garbage.
        assert_eq!(
            parse_status_line(&ClusterBackend::Kubernetes, "oxoflow-align|")
                .unwrap()
                .1,
            BackendJobStatus::Unknown
        );
        let (id, st) =
            parse_status_line(&ClusterBackend::GcpBatch, "oxoflow-x-1\tSUCCEEDED").unwrap();
        assert_eq!(id, "oxoflow-x-1");
        assert_eq!(st, BackendJobStatus::Completed);
    }

    #[test]
    fn minted_gcp_job_ids_respect_the_63_char_cap() {
        let id = mint_gcp_job_id(
            "a-very-long-rule-directory-name-exceeding-gcp-limit-xyz",
            1_700_000_000_000,
        );
        assert!(id.len() <= 63, "id too long: {} ({})", id, id.len());
        assert!(id.starts_with("oxoflow-"));
        assert!(id.ends_with("-1700000000000"));
        // Short names pass through whole.
        let short = mint_gcp_job_id("align", 1);
        assert_eq!(short, "oxoflow-align-1");
    }

    #[test]
    fn gcp_fully_qualified_names_normalize_to_bare_ids() {
        // `gcloud --format=value(name,…)` prints
        // projects/p/locations/l/jobs/<id>; the bare id is what users (and
        // the submit path) speak, and a by-id status query must match.
        let (id, st) = parse_status_line(
            &ClusterBackend::GcpBatch,
            "projects/p/locations/us-central1/jobs/oxoflow-x-1\tSUCCEEDED",
        )
        .unwrap();
        assert_eq!(id, "oxoflow-x-1");
        assert_eq!(st, BackendJobStatus::Completed);
        // Bare ids pass through untouched.
        let (id2, _) = parse_status_line(&ClusterBackend::GcpBatch, "oxoflow-x-1|RUNNING").unwrap();
        assert_eq!(id2, "oxoflow-x-1");
    }

    #[test]
    fn cloud_job_listings_are_not_filtered_by_the_hpc_header_guard() {
        // The numeric-first guard exists to drop HPC table headers; cloud
        // ids are alphanumeric and were ALL filtered out before it was
        // backend-gated.
        let rows = parse_job_listing(
            &ClusterBackend::AwsBatch,
            "job-abc-123|SUCCEEDED\njob-def-456|RUNNING\n",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "job-abc-123");
        assert_eq!(rows[0].1, BackendJobStatus::Completed);
        // HPC keeps its header guard.
        assert!(
            parse_job_listing(
                &ClusterBackend::Pbs,
                "Job id  Name  User  Time Use  S  Queue\n"
            )
            .is_empty()
        );
    }

    #[test]
    fn submit_invocation_hints_match_executor_shapes() {
        // The manual-submit hints must name the same invocation form
        // ClusterExecutor::submit issues per backend (the cloud backends
        // take manifest files, not script arguments).
        let dir = "out";
        assert_eq!(
            crate::cluster::submit_invocation_hint(&ClusterBackend::Slurm, dir),
            "sbatch --parsable out/*.sh"
        );
        assert_eq!(
            crate::cluster::submit_invocation_hint(&ClusterBackend::Kubernetes, dir),
            "kubectl create -f out/*.sh"
        );
        assert!(
            crate::cluster::submit_invocation_hint(&ClusterBackend::AwsBatch, dir)
                .contains("aws batch submit-job --cli-input-json")
        );
        assert!(
            crate::cluster::submit_invocation_hint(&ClusterBackend::GcpBatch, dir)
                .contains("gcloud batch jobs submit")
        );
    }
    #[test]
    fn cancel_invocations_match_each_backends_cli_grammar() {
        let ids = vec!["a".to_string(), "b".to_string()];
        let k8s = cancel_invocations(&ClusterBackend::Kubernetes, &ids);
        assert_eq!(
            k8s,
            vec![(
                "kubectl".to_string(),
                vec![
                    "delete".to_string(),
                    "job".to_string(),
                    "a".to_string(),
                    "b".to_string()
                ]
            )]
        );
        // Scheduler-array CLIs take every id in one call.
        assert_eq!(
            cancel_invocations(&ClusterBackend::Slurm, &ids),
            vec![("scancel".to_string(), ids.clone())]
        );
        // Per-job cloud APIs get one invocation per id.
        let aws = cancel_invocations(&ClusterBackend::AwsBatch, &ids);
        assert_eq!(aws.len(), 2);
        assert!(aws[0].1.contains(&"--job-id".to_string()));
        let gcp = cancel_invocations(&ClusterBackend::GcpBatch, &ids);
        assert_eq!(gcp.len(), 2);
        assert_eq!(gcp[1].1[2], "delete");
    }
}
