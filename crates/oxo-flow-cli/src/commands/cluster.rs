use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::backend::BackendJobStatus;
use oxo_flow_core::backend::ExecutorBackend;
use oxo_flow_core::backend::cluster::elementwise_aftercorr;
use oxo_flow_core::cluster::{ClusterBackend, ClusterJobConfig};
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::ClusterAction;
use crate::commands::print_banner;

/// Human `cluster status` rows printed before truncation. A production run
/// can hold >10k instances; the machine forms (TSV on a pipe, `--json`)
/// always carry every instance.
const STATUS_ROW_LIMIT: usize = 50;

/// Resolve a `--backend` value, refusing to guess.
///
/// An unrecognised name used to fall back to SLURM, so a typo (`-b slrm`,
/// `-b pbs-torque`) submitted real jobs to, or queried, the wrong scheduler.
fn parse_backend(name: &str) -> Result<ClusterBackend> {
    ClusterBackend::from_str(name).map_err(|_| {
        anyhow::anyhow!(
            "unknown cluster backend '{name}' — expected {}",
            oxo_flow_core::cluster::SUPPORTED_BACKENDS_HINT
        )
    })
}

/// Resolve the backend for a cluster action: the `--backend` flag wins,
/// then `$OXO_FLOW_CLUSTER_BACKEND`, then SLURM — the overwhelmingly
/// common scheduler, and the one `-b` used to be required for. An EXPLICIT
/// value keeps [`parse_backend`]'s strict error, so a typo can never
/// silently route to SLURM; only an omitted flag falls back.
fn resolve_backend(flag: Option<&str>) -> Result<ClusterBackend> {
    let env = std::env::var("OXO_FLOW_CLUSTER_BACKEND").ok();
    resolve_backend_with(flag, env.as_deref())
}

/// The pure decision behind [`resolve_backend`], with the environment value
/// injected — the test crate forbids `unsafe`, which rules out the
/// `set_var` dance, and a pure function pins the precedence table anyway.
fn resolve_backend_with(flag: Option<&str>, env: Option<&str>) -> Result<ClusterBackend> {
    if let Some(name) = flag {
        return parse_backend(name);
    }
    match env.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => parse_backend(name),
        None => Ok(ClusterBackend::Slurm),
    }
}

/// Human one-liner for the parsed status of one job.
fn status_line(id: &str, status: BackendJobStatus) -> String {
    match status {
        BackendJobStatus::Unknown => {
            format!("  {id}: not in the queue (finished, or an unknown id)")
        }
        other => format!("  {id}: {}", other.label()),
    }
}

/// Ask the scheduler after `job_ids` and answer one status per id.
///
/// The scheduler's output is CAPTURED and parsed rather than streamed: a bare
/// passthrough gave the caller a transcript when it asked a question, and
/// nothing a machine could read. `run` is the execution seam — the command
/// hands in the real scheduler binary, tests hand in a stub.
fn query_status(
    backend: &ClusterBackend,
    job_ids: &[String],
    queue: Option<&str>,
    run: impl Fn(&str, &[String]) -> Result<String>,
) -> Result<Vec<(String, BackendJobStatus)>> {
    let mut report: Vec<(String, BackendJobStatus)> = job_ids
        .iter()
        .map(|id| (id.clone(), BackendJobStatus::Unknown))
        .collect();
    for (program, args) in
        oxo_flow_core::backend::cluster::status_invocations(backend, job_ids, queue)
    {
        for (id, status) in
            oxo_flow_core::backend::cluster::status_report(backend, job_ids, &run(program, &args)?)
        {
            // A scheduler that does not know an id stays silent about it:
            // keep the earlier verdict rather than overwriting it with
            // unknown.
            if status != BackendJobStatus::Unknown
                && let Some(entry) = report.iter_mut().find(|(rid, _)| rid == &id)
            {
                entry.1 = status;
            }
        }
    }
    Ok(report)
}

/// Ask the scheduler for the current user's live jobs and answer one
/// `(id, status)` per listed row — the no-arguments `cluster status` view.
/// Same execution seam as [`query_status`].
///
/// A backend may need more than one invocation to answer the live view
/// (AWS Batch's list-jobs returns only RUNNING jobs unless each live
/// status is requested explicitly), so every invocation is run and the rows
/// are merged; the first id wins if a job transitions between calls.
fn query_my_jobs(
    backend: &ClusterBackend,
    queue: Option<&str>,
    run: impl Fn(&str, &[String]) -> Result<String>,
) -> Result<Vec<(String, BackendJobStatus)>> {
    let mut listing: Vec<(String, BackendJobStatus)> = Vec::new();
    for (program, args) in oxo_flow_core::backend::cluster::my_jobs_invocations(backend, queue) {
        eprintln!(
            "{} Executing '{} {}'...",
            "Cluster:".bold().cyan(),
            program,
            args.join(" ")
        );
        // A user listing has no per-requested-id contract: the rows the
        // scheduler returns ARE the answer.
        let rows =
            oxo_flow_core::backend::cluster::parse_job_listing(backend, &run(program, &args)?);
        for row in rows {
            if !listing.iter().any(|(id, _)| id == &row.0) {
                listing.push(row);
            }
        }
    }
    Ok(listing)
}

/// Emit the `oxo_submit` shell helper: submits a script and echoes the bare
/// scheduler job id.
///
/// Only PBS's `qsub` prints an id a dependency flag can consume directly.
/// SLURM needs `--parsable`; SGE and LSF print sentences. Chaining raw
/// submit output into `--dependency=afterok:` produced
/// `afterok:Submitted batch job 12345` and broke silently (issue #74
/// phase-1 item 1). The patterns mirror `parse_job_id` in
/// `oxo_flow_core::backend::cluster` so the wrapper and the live submit
/// path agree on what a job id is.
///
/// `selector` is the profile's `queue` value where the cloud CLI needs it
/// to address the submission (GCP location, Tencent placement zone).
fn generate_submit_helper(backend: &ClusterBackend, selector: Option<&str>) -> String {
    let quote = oxo_flow_core::environment::escape_for_sh_single_quote;
    let body: String = match backend {
        // `--parsable` prints a bare id, or `<id>;<cluster>` on federated
        // clusters — keep the id.
        ClusterBackend::Slurm => {
            "  out=$(sbatch --parsable \"$@\") || return $?\n  id=${out%%;*}\n".to_string()
        }
        // qsub already prints a bare id (`12345.headnode` is accepted
        // verbatim by `-W depend=`). Exactly one token, or fall through to
        // the error path — mashing multi-token output together would feed a
        // garbage id to the next job's `-W depend=`.
        ClusterBackend::Pbs => {
            "  out=$(qsub \"$@\") || return $?\n  set -- $out\n  if [ $# -eq 1 ]; then id=\"$1\"; else id=\"\"; fi\n".to_string()
        }
        // "Your job 12345 (\"align\") has been submitted"
        ClusterBackend::Sge => {
            "  out=$(qsub \"$@\") || return $?\n  id=$(printf '%s' \"$out\" | grep -oE 'Your job(-array)? [0-9]+' | grep -oE '[0-9]+$')\n".to_string()
        }
        // "Job <12345> is submitted to queue <normal>."
        ClusterBackend::Lsf => {
            "  out=$(bsub \"$@\") || return $?\n  id=$(printf '%s' \"$out\" | grep -oE 'Job <[0-9]+>' | grep -oE '[0-9]+')\n".to_string()
        }
        ClusterBackend::Kubernetes => {
            "  out=$(kubectl create -f \"$@\" -o jsonpath='{.metadata.name}') || return $?\n  id=\"$out\"\n".to_string()
        }
        ClusterBackend::AwsBatch => {
            "  out=$(aws batch submit-job --cli-input-json file://\"$@\" --query 'jobId' --output text) || return $?\n  id=\"$out\"\n".to_string()
        }
        ClusterBackend::GcpBatch => {
            // `gcloud batch jobs submit` takes a job id positionally and
            // the manifest via --config as a PLAIN local path (a `file://`
            // prefix fails with "file not found"). No `--query` flag exists
            // in gcloud — the id is minted here, submitted, and echoed.
            let location = selector
                .map(|loc| format!(" --location '{}'", quote(loc)))
                .unwrap_or_default();
            format!(
                "  id=\"oxoflow-$(basename \"$1\" .sh | tr '[:upper:]_' '[:lower:]-' | \
                 sed 's/[^a-z0-9-]//g' | cut -c1-41)-$(date +%s)\"\n\
                 \x20 out=$(gcloud batch jobs submit \"$id\" --config \"$1\"{location}) || return $?\n"
            )
        }
        ClusterBackend::TencentBatch => {
            let region = selector
                .map(|zone| {
                    format!(
                        " --region '{}'",
                        quote(oxo_flow_core::backend::tencent_batch::region_from_zone(zone))
                    )
                })
                .unwrap_or_default();
            // tccli prints the unwrapped response body (`{"JobId": …}`);
            // the grep accepts that and the API envelope's spelling.
            format!(
                "  out=$(tccli batch SubmitJob --cli-input-json file://\"$@\" --output json{region}) || return $?\n\
                 \x20 id=$(printf '%s' \"$out\" | grep -oE '\"JobId\": ?\"[^\"]+\"' | grep -oE 'job-[a-z0-9]+')\n"
            )
        }
    };

    let mut helper = String::new();
    helper.push_str("# Submit one script and echo its scheduler job id.\n");
    helper.push_str("oxo_submit() {\n");
    helper.push_str("  local out id\n");
    helper.push_str(&body);
    helper.push_str("  if [ -z \"$id\" ]; then\n");
    helper.push_str("    echo \"oxo-flow: cannot parse job id from: $out\" >&2\n");
    helper.push_str("    return 1\n");
    helper.push_str("  fi\n");
    helper.push_str("  printf '%s' \"$id\"\n");
    helper.push_str("}\n\n");
    helper
}

/// Read a rendered script for the aftercorr decision. Failure is not an
/// error: the decision is an optimization on top of a correct default, so
/// an unreadable script just means the ready-batch dependency.
fn read_script(output_dir: &Path, rule: &str) -> String {
    std::fs::read_to_string(output_dir.join(format!("{rule}.sh"))).unwrap_or_default()
}

/// Generate a submit wrapper script that handles job dependencies.
/// This script tracks job IDs and sets up proper dependency chains.
fn generate_submit_wrapper(
    backend: &ClusterBackend,
    order: &[String],
    dag: &WorkflowDag,
    output_dir: &Path,
    selector: Option<&str>,
) -> Result<String> {
    let mut script = String::new();
    script.push_str("#!/bin/bash\n");
    script.push_str("# Auto-generated dependency-aware submit script\n");
    script.push_str("# Generated by oxo-flow\n\n");
    script.push_str("set -e\n\n");
    script.push_str("# Track job IDs\ndeclare -A JOB_IDS\n\n");
    script.push_str(&generate_submit_helper(backend, selector));

    // Generate submit commands for each rule in order
    for rule_name in order {
        let script_name = format!("{}.sh", rule_name);
        let script_path = output_dir.join(&script_name);

        // Get dependencies for this rule
        let deps = dag.dependencies(rule_name).unwrap_or_default();
        let dep_job_refs: Vec<String> = deps
            .iter()
            .map(|d| format!("${{JOB_IDS[{}]}}", d))
            .collect();

        script.push_str(&format!("echo 'Submitting {}...'\n", rule_name));

        // Add dependency specification if there are dependencies
        if !dep_job_refs.is_empty() {
            match backend {
                ClusterBackend::Slurm => {
                    // Element-wise chaining (issue #371): when the single
                    // upstream and this rule are both straight arrays with
                    // the same index range, `aftercorr` pairs elements 1:1
                    // and pipelines the chain (measured 1.33× makespan win
                    // in issue #355). The decision reads the WRITTEN
                    // scripts — the only source of truth for `--array`
                    // directives arriving via `--extra-arg`. Any other
                    // shape keeps the known-safe ready-batch `afterok:`.
                    let dep_flag = match deps.as_slice() {
                        [dep]
                            if elementwise_aftercorr(
                                &read_script(output_dir, dep),
                                &read_script(output_dir, rule_name),
                            ) =>
                        {
                            format!("aftercorr:${{JOB_IDS[{dep}]}}")
                        }
                        _ => format!("afterok:{}", dep_job_refs.join(":")),
                    };
                    script.push_str(&format!(
                        "JOB_IDS[{}]=$(oxo_submit --dependency={} {})\n",
                        rule_name,
                        dep_flag,
                        script_path.display()
                    ));
                }
                ClusterBackend::Pbs => {
                    // PBS uses -W depend=afterok:jobid
                    let dep_str = dep_job_refs.join(":");
                    script.push_str(&format!(
                        "JOB_IDS[{}]=$(oxo_submit -W depend=afterok:{} {})\n",
                        rule_name,
                        dep_str,
                        script_path.display()
                    ));
                }
                ClusterBackend::Sge => {
                    // SGE takes one comma-separated -hold_jid list; a repeated
                    // flag would keep only the last dependency.
                    let hold_jid = dep_job_refs.join(",");
                    script.push_str(&format!(
                        "JOB_IDS[{}]=$(oxo_submit -hold_jid {} {})\n",
                        rule_name,
                        hold_jid,
                        script_path.display()
                    ));
                }
                ClusterBackend::Lsf => {
                    // LSF uses -w 'ended(jobid)'. DOUBLE quotes in the
                    // generated script: single quotes would keep the
                    // ${JOB_IDS[..]} reference literal and the dependency
                    // chain would silently never chain (caught live on
                    // tx-ubuntu's mock bsub).
                    let dep_str = dep_job_refs
                        .iter()
                        .map(|d| format!("ended({})", d))
                        .collect::<Vec<_>>()
                        .join(" && ");
                    script.push_str(&format!(
                        "JOB_IDS[{}]=$(oxo_submit -w \"{}\" {})\n",
                        rule_name,
                        dep_str,
                        script_path.display()
                    ));
                }
                ClusterBackend::Kubernetes
                | ClusterBackend::AwsBatch
                | ClusterBackend::GcpBatch
                | ClusterBackend::TencentBatch => {
                    script.push_str(&format!(
                        "JOB_IDS[{}]=\"$(oxo_submit {})\"\n",
                        rule_name,
                        script_path.display()
                    ));
                }
            }
        } else {
            // No dependencies
            script.push_str(&format!(
                "JOB_IDS[{}]=$(oxo_submit {})\n",
                rule_name,
                script_path.display()
            ));
        }

        // Double quotes: the id has to expand, not print literally.
        script.push_str(&format!(
            "echo \"  Submitted {} as job ID: ${{JOB_IDS[{}]}}\"\n\n",
            rule_name, rule_name
        ));
    }

    script.push_str("echo 'All jobs submitted successfully!'\n");
    script.push_str("echo 'Job ID mapping:'\n");
    script.push_str("for name in \"${!JOB_IDS[@]}\"; do\n");
    script.push_str("  echo \"  $name: ${JOB_IDS[$name]}\"\n");
    script.push_str("done\n");

    Ok(script)
}

// ── Run-aware status/logs (issue #1085) ────────────────────────────────
//
// `run --profile <cluster>` records one directory per rule instance under
// `.oxo-flow/runs/<stamp>/jobs/<instance>/`: `job.id` (scheduler id),
// `job.sh`, and `status.json` (state + timestamps + accounting figures).
// The engine also captures each job's stdout/stderr under
// `<workdir>/logs/<instance>.out/.err`. These helpers read that state so
// `cluster status|logs` can answer questions about a RUN, not just about
// raw scheduler ids.

/// One rule instance as recorded in a run directory.
#[derive(Debug, Clone, serde::Serialize)]
struct RunInstance {
    instance: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    job_id: Option<String>,
    state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    submitted_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    elapsed_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cpu_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_rss_mb: Option<u64>,
}

impl RunInstance {
    /// Parse one `jobs/<instance>/` directory. Missing files degrade to
    /// `unknown` rather than failing the whole report: a run directory is
    /// written incrementally, so a half-created instance is expected.
    fn from_dir(name: String, dir: &Path) -> RunInstance {
        let job_id = std::fs::read_to_string(dir.join("job.id"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let status: Option<serde_json::Value> = std::fs::read_to_string(dir.join("status.json"))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok());
        let text = |key: &str| {
            status
                .as_ref()
                .and_then(|v| v.get(key))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        let number = |key: &str| {
            status
                .as_ref()
                .and_then(|v| v.get(key))
                .and_then(|v| v.as_u64())
        };
        RunInstance {
            instance: name,
            job_id: job_id.or_else(|| text("job_id")),
            state: text("state")
                .unwrap_or_else(|| "unknown".to_string())
                .to_ascii_lowercase(),
            submitted_at: text("submitted_at"),
            finished_at: text("finished_at"),
            exit_code: status
                .as_ref()
                .and_then(|v| v.get("exit_code"))
                .and_then(serde_json::Value::as_i64),
            elapsed_secs: number("elapsed_secs"),
            cpu_seconds: status
                .as_ref()
                .and_then(|v| v.get("cpu_seconds"))
                .and_then(serde_json::Value::as_f64),
            max_rss_mb: number("max_rss_mb"),
        }
    }

    /// Human elapsed figure: the accounting store's own when present,
    /// otherwise the recorded submit/finish span. `-` when neither exists
    /// (a live job's `status.json` carries no timestamps until it settles).
    fn elapsed_label(&self) -> String {
        if let Some(secs) = self.elapsed_secs {
            return format_duration(secs);
        }
        let (Some(submitted), Some(finished)) = (&self.submitted_at, &self.finished_at) else {
            return "-".to_string();
        };
        let parse = |s: &str| chrono::DateTime::parse_from_rfc3339(s).ok();
        match (parse(submitted), parse(finished)) {
            (Some(a), Some(b)) => {
                let secs = (b - a).num_seconds().max(0);
                format_duration(secs as u64)
            }
            _ => "-".to_string(),
        }
    }
}

fn format_duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    format!("{h}:{m:02}:{s:02}")
}

/// Load every instance recorded under `<run-dir>/jobs/`, sorted by name.
fn load_run_instances(run_dir: &Path) -> Result<Vec<RunInstance>> {
    let jobs = run_dir.join("jobs");
    let entries = std::fs::read_dir(&jobs)
        .with_context(|| format!("cannot read run directory {}", jobs.display()))?;
    let mut instances = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("cannot read an entry of {}", jobs.display()))?;
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let dir = entry.path();
        // Array submissions also create `jobs/<template>/chunk-N/` bookkeeping
        // directories. Only real instances carry `job.id` or `status.json`
        // directly; a container without either is not a rule instance.
        if !dir.join("job.id").exists() && !dir.join("status.json").exists() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        instances.push(RunInstance::from_dir(name, &dir));
    }
    instances.sort_by(|a, b| a.instance.cmp(&b.instance));
    Ok(instances)
}

/// The workflow's default run directory: `run --profile` repoints
/// `.oxo-flow/runs/latest` after every invocation. `None` when this
/// directory has never hosted a cluster run.
fn default_run_dir() -> Option<PathBuf> {
    let latest = PathBuf::from(".oxo-flow").join("runs").join("latest");
    latest.is_dir().then_some(latest)
}

/// Resolve the directory the run-aware `cluster status` view reads.
///
/// `default` is the caller's single [`default_run_dir`] probe. Re-deriving
/// it here (as the old guard did, then again for the value) lets the newest
/// run directory disappear between the two calls — a concurrent run
/// rotating `runs/latest`, a cleanup — and the `expect` on the second
/// result turned that race into a panic in a status command. Both inputs
/// absent is a caller bug with an actionable operator message, not a crash.
fn resolve_run_aware_dir(run_dir: Option<PathBuf>, default: Option<PathBuf>) -> Result<PathBuf> {
    run_dir.or(default).ok_or_else(|| {
        anyhow::anyhow!(
            "run-aware cluster status needs a run directory, but none was given and \
             .oxo-flow/runs does not exist — pass --run-dir <DIR>, or --all for the raw \
             scheduler view"
        )
    })
}

/// The workdir a run directory belongs to (`<workdir>/.oxo-flow/runs/<stamp>`).
fn run_workdir(run_dir: &Path) -> Option<PathBuf> {
    let canonical = run_dir.canonicalize().ok()?;
    let runs = canonical.parent()?;
    if runs.file_name()? != "runs" {
        return None;
    }
    let dot_oxo = runs.parent()?;
    if dot_oxo.file_name()? != ".oxo-flow" {
        return None;
    }
    dot_oxo.parent().map(Path::to_path_buf)
}

/// Scheduler log paths declared by a rendered job script.
///
/// Array scripts carry scheduler placeholders (`%A_%a` for SLURM, `%I` for
/// LSF) rather than the instance name, so the per-element file can only be
/// found by substituting this element's ids.
fn declared_log_paths(script: &str, backend: ClusterBackend, element_id: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in script.lines() {
        let trimmed = line.trim_start();
        let value = match backend {
            ClusterBackend::Slurm => trimmed
                .strip_prefix("#SBATCH --output=")
                .or_else(|| trimmed.strip_prefix("#SBATCH --error=")),
            ClusterBackend::Pbs => trimmed
                .strip_prefix("#PBS -o ")
                .or_else(|| trimmed.strip_prefix("#PBS -e ")),
            ClusterBackend::Sge => trimmed
                .strip_prefix("#$ -o ")
                .or_else(|| trimmed.strip_prefix("#$ -e ")),
            ClusterBackend::Lsf => trimmed
                .strip_prefix("#BSUB -o ")
                .or_else(|| trimmed.strip_prefix("#BSUB -e ")),
            _ => None,
        };
        let Some(value) = value else {
            continue;
        };
        paths.push(PathBuf::from(resolve_array_placeholders(
            value.trim(),
            backend,
            element_id,
        )));
    }
    paths
}

/// Substitute the scheduler's array placeholders with one element's ids.
fn resolve_array_placeholders(pattern: &str, backend: ClusterBackend, element_id: &str) -> String {
    match backend {
        // SLURM element ids are `<base>_<index>`; `%A` is the base and `%a`
        // the index.
        ClusterBackend::Slurm => {
            let (base, index) = element_id
                .rsplit_once('_')
                .filter(|(_, index)| index.parse::<u32>().is_ok())
                .unwrap_or((element_id, ""));
            pattern.replace("%A", base).replace("%a", index)
        }
        // LSF element ids are `<base>[<index>]`; `%I` is the index.
        ClusterBackend::Lsf => {
            let index = element_id
                .rsplit_once('[')
                .and_then(|(_, rest)| rest.strip_suffix(']'))
                .unwrap_or("");
            pattern.replace("%I", index)
        }
        // PBS/SGE append their own per-subjob suffixes to a shared path,
        // which cannot be predicted from the directive alone.
        _ => pattern.to_string(),
    }
}

/// One instance's engine-captured stdout/stderr, if the scheduler wrote it.
///
/// Candidates, in order: `logs/<instance>.out/.err` (plain jobs), then the
/// paths declared by this instance's copied `job.sh` (arrays, where the
/// directive carries `%A_%a`/`%I` rather than the instance name). The
/// capture is the primary `cluster logs` source: it exists on every site,
/// including those without Slurm accounting.
fn read_captured_logs(
    workdir: &Path,
    instance: &str,
    instance_dir: Option<&Path>,
    backend: ClusterBackend,
) -> Option<String> {
    let logs_dir = workdir.join("logs");
    let mut candidates = vec![
        logs_dir.join(format!("{instance}.out")),
        logs_dir.join(format!("{instance}.err")),
    ];
    if let Some(dir) = instance_dir
        && let Ok(script) = std::fs::read_to_string(dir.join("job.sh"))
    {
        let element_id = std::fs::read_to_string(dir.join("job.id"))
            .unwrap_or_default()
            .trim()
            .to_string();
        for path in declared_log_paths(&script, backend, &element_id) {
            let full = if path.is_absolute() {
                path
            } else {
                workdir.join(path)
            };
            if !candidates.contains(&full) {
                candidates.push(full);
            }
        }
    }
    let mut sections: Vec<String> = Vec::new();
    for path in candidates {
        if let Ok(text) = std::fs::read_to_string(&path)
            && !text.trim().is_empty()
        {
            sections.push(format!("==> {} <==\n{}", path.display(), text));
        }
    }
    (!sections.is_empty()).then(|| sections.join("\n"))
}

/// Apply `--tail N`: keep the last N lines, preserving a trailing newline.
fn tail_lines(text: &str, tail: Option<usize>) -> String {
    let Some(n) = tail else {
        return text.to_string();
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    let mut out = lines[start..].join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// The first non-empty candidate in fallback order.
///
/// Pure so the `cluster logs` chain — run-dir capture → accounting →
/// `scontrol` — is unit-testable without a live scheduler. A failed
/// command contributes `None`: the next source is tried, never an error
/// exit before the chain is exhausted (issue #1085 C3).
fn first_log_source(
    attempts: Vec<(&'static str, Option<String>)>,
) -> Option<(&'static str, String)> {
    attempts
        .into_iter()
        .find_map(|(source, text)| text.filter(|t| !t.trim().is_empty()).map(|t| (source, t)))
}

/// Overlay live scheduler states onto the run-directory states. The
/// driver writes `PENDING` at submission and updates it only when the job
/// settles, so without this a running job would render as pending.
fn overlay_scheduler_states(instances: &mut [RunInstance], report: &[(String, BackendJobStatus)]) {
    for instance in instances.iter_mut() {
        let Some(job_id) = instance.job_id.as_deref() else {
            continue;
        };
        let Some((_, status)) = report.iter().find(|(id, _)| id == job_id) else {
            continue;
        };
        if *status != BackendJobStatus::Unknown {
            instance.state = status.label().to_string();
        }
    }
}

/// Per-state counts for the status summary, keyed alphabetically.
fn state_counts(instances: &[RunInstance]) -> std::collections::BTreeMap<String, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for instance in instances {
        *counts.entry(instance.state.clone()).or_insert(0) += 1;
    }
    counts
}

/// The scheduler ids for a set of instances (deduplicated, order-preserved).
fn instance_job_ids(instances: &[RunInstance]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for id in instances.iter().filter_map(|i| i.job_id.clone()) {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// One machine-readable status line: the fields a script needs, tab-separated.
fn instance_tsv(instance: &RunInstance) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}",
        instance.instance,
        instance.job_id.as_deref().unwrap_or("-"),
        instance.state,
        instance.elapsed_label(),
        instance
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-".to_string())
    )
}

pub async fn cluster_command(action: ClusterAction, json: bool) -> Result<()> {
    print_banner();
    match action {
        ClusterAction::Submit {
            workflow,
            backend,
            queue,
            account,
            walltime,
            extra_args,
            output,
            target,
            module,
            dry_run,
            with_dependencies,
        } => {
            // The backend is validated before any workflow is read: a typo
            // must not cost a parse, let alone reach a queue.
            let cluster_backend = resolve_backend(backend.as_deref())?;

            let mut config = WorkflowConfig::from_file(&workflow)
                .with_context(|| format!("failed to parse {}", workflow.display()))?;

            // Expand wildcards before the DAG is built, exactly as `run`
            // does (issue #74 phase 1). Without this a scatter rule stays a
            // single template and the generated script submits a literal
            // `{sample}` to the scheduler.
            config.apply_defaults();
            config
                .expand_wildcards()
                .context("failed to expand wildcard rules")?;

            // Same config-aware DAG construction `run` uses (audit finding:
            // the plain constructor left `{config.x}` unexpanded, so edges
            // between a producer's declared output and a consumer's concrete
            // input were lost and the generated scripts had no dependencies).
            // One map serves the whole command (DAG edges, when-gating, and
            // script rendering below) — computing it once keeps the three
            // consumers in sync by construction.
            let wildcard_values = crate::commands::run::config_placeholder_values(&config.config);
            let dag = WorkflowDag::from_rules_with_config(&config.rules, &wildcard_values)
                .context("failed to build workflow DAG")?;

            // --module partial runs (issue #112 elasticity) — the same
            // resolution `run` and `dry-run` use: each module name resolves
            // to its rules plus the host producers of its declared concrete
            // inputs, unioned into --target for the ordering machinery below.
            let mut target = target;
            for m in &module {
                match config.module_closure(m) {
                    Some(names) => target.extend(names),
                    None => {
                        let known: Vec<&String> = config.module_rules.keys().collect();
                        return Err(anyhow::anyhow!(
                            "unknown module '{m}' — known modules: {}",
                            if known.is_empty() {
                                "(none — no [[include]] modules)".to_string()
                            } else {
                                known
                                    .iter()
                                    .map(|k| k.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            }
                        ));
                    }
                }
            }

            // Targeted cluster runs close over the INSTANTIATED DAG
            // (issue #247): when-gated instances never enter the set.
            let when_false_rules: std::collections::HashSet<String> = if target.is_empty() {
                std::collections::HashSet::new()
            } else {
                config
                    .rules
                    .iter()
                    .filter(|rule| {
                        crate::commands::run_preview::when_condition_false(
                            rule,
                            &config,
                            &wildcard_values,
                        )
                    })
                    .map(|rule| rule.name.clone())
                    .collect()
            };
            let order = if target.is_empty() {
                // When-aware closure (#882/#883), same as the `run` command:
                // raw `execution_order` would send consumers of when-gated
                // producers to the cluster and fail them on absent files.
                let (filtered, pruned) = dag
                    .execution_order_for_targets_skipping_with_source_check(
                        &[],
                        &when_false_rules,
                        // Pre-built inputs (atacseq bwa_mem finding): the
                        // generated scripts run in the workflow's own
                        // directory, so existence anchors there.
                        &|path| {
                            let p = Path::new(path);
                            if p.is_absolute() {
                                p.exists()
                            } else {
                                workflow.parent().unwrap_or(Path::new(".")).join(p).exists()
                            }
                        },
                    )
                    .with_context(|| "failed to compute full-run execution order")?;
                for rule_name in &pruned {
                    eprintln!(
                        "{} rule '{rule_name}' is unreachable — its upstream is when-gated false; removed from the full-run execution set",
                        "Note:".yellow()
                    );
                }
                filtered
            } else {
                let target_refs: Vec<&str> = target.iter().map(String::as_str).collect();
                let (filtered, skipped_targets) = dag
                    .execution_order_for_targets_skipping_with_source_check(
                        &target_refs,
                        &when_false_rules,
                        // Pre-built inputs (atacseq bwa_mem finding): the
                        // generated scripts run in the workflow's own
                        // directory, so existence anchors there.
                        &|path| {
                            let p = Path::new(path);
                            if p.is_absolute() {
                                p.exists()
                            } else {
                                workflow.parent().unwrap_or(Path::new(".")).join(p).exists()
                            }
                        },
                    )
                    .with_context(|| "failed to resolve target rules")?;
                for skipped in &skipped_targets {
                    eprintln!(
                        "{} target '{skipped}' is when-gated false — it never runs; removed from the execution set (its upstream was pruned too)",
                        "Note:".yellow()
                    );
                }
                filtered
            };

            let cluster_config = oxo_flow_core::cluster::ClusterJobConfig {
                backend: cluster_backend,
                queue: queue.clone(),
                account: account.clone(),
                walltime: walltime.clone(),
                extra_args: extra_args.clone(),
                // Scope Job names to this submission directory so re-rendering
                // into the same directory stays idempotent while a different
                // directory never collides (issue #1158).
                name_scope: Some(output.to_string_lossy().into_owned()),
                volume: None,
            };

            // A dry run writes the very scripts a real submit would: an HPC
            // user asks for one to read the #SBATCH headers before the queue
            // sees them. Nothing is submitted — the wrapper is left out and
            // the summary says so (the old dry run generated nothing at all,
            // so there was nothing to review).
            std::fs::create_dir_all(&output)?;

            // The rendered scripts declare `#SBATCH --output=logs/<rule>.out`
            // and slurmd opens that file at job launch — before the script
            // body's `mkdir -p logs` runs. Create the directory now (issue #74
            // phase-1 note 2).
            if let Some(wf_dir) = workflow.parent() {
                std::fs::create_dir_all(wf_dir.join("logs"))?;
            }

            if dry_run {
                eprintln!(
                    "{} (dry-run) generating {} job scripts for {} rule instances — nothing is submitted",
                    "Cluster:".bold().yellow(),
                    cluster_backend,
                    order.len()
                );
            } else {
                eprintln!(
                    "{} Generating {} job scripts for {} rule instances",
                    "Cluster:".bold().cyan(),
                    cluster_backend,
                    order.len()
                );
            }

            // Create environment resolver for command wrapping
            let env_resolver = oxo_flow_core::environment::EnvironmentResolver::new();

            // The scripts run in the workflow's own directory: that is where
            // their `logs/` directory is created below, and a relative `cd`
            // would otherwise make the job run in whatever directory the
            // submitter happened to use (audit finding).
            let cluster_workdir = workflow
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));

            // issue #1013 C2: the run driver exports the run's scratch
            // directory into every job shell; this standalone path must
            // honor `[engine] tmp_dir` / OXO_FLOW_TMPDIR the same way or
            // the two paths silently disagree (tx-ubuntu user-sim finding).
            // Resolved once here — the mkdir fail-fast matches the run path.
            let tmp_dir = crate::commands::run::tmp_dir_resolved(
                config.engine.as_ref().and_then(|e| e.tmp_dir.clone()),
                &cluster_workdir,
            )?;

            // Sensitive config values (issue #1120, audit RB-12): every
            // backend renders a job artifact from each rule's expanded
            // command, so `cluster submit` routes the same
            // `OXO_FLOW_SENSITIVE_<n>` references the live run path does —
            // before the environment wrapper is built, so docker/singularity
            // forward by name. One binding table for the whole command keeps
            // the names consistent across every rule. HPC scripts then source
            // a private (0o600) env file beside the script; cloud bodies
            // carry the reference into their own secret channel.
            let sensitive_bindings = oxo_flow_core::executor::process::sensitive_bindings(
                &crate::commands::run::findings::sensitive_values_of(&config),
            );
            // Script-based schedulers copy the script into scheduler-visible
            // storage, so the value can only travel in the sidecar env file.
            let hpc_scripts = matches!(
                cluster_backend,
                ClusterBackend::Slurm
                    | ClusterBackend::Pbs
                    | ClusterBackend::Sge
                    | ClusterBackend::Lsf
            );
            let mut secret_env_files: Vec<std::path::PathBuf> = Vec::new();
            let executor = oxo_flow_core::backend::cluster::ClusterExecutor::new(
                cluster_backend,
                cluster_config.clone(),
            )
            .with_secret_refs(oxo_flow_core::backend::secret_refs_from_env());

            // Workflow-level terminal hooks (on_complete/on_error) are owned by
            // the CLI run loop, which `cluster submit` never enters, so they
            // would be silently dropped (issue #1340). Reject before rendering
            // any script. The hook fields come from the shared
            // `WorkflowMeta::lifecycle_hooks` definition.
            for (field, hook) in config.workflow.lifecycle_hooks() {
                if hook.is_some() {
                    return Err(anyhow::anyhow!(
                        "workflow-level lifecycle hook `{field}` is not supported by \
                         `cluster submit` — the CLI run loop that fires it is not entered on the \
                         cluster path, so the hook would be silently dropped; remove it or run \
                         the workflow on the local path (`oxo-flow run`) instead"
                    ));
                }
            }

            for rule_name in &order {
                let rule = config
                    .get_rule(rule_name)
                    .ok_or_else(|| anyhow::anyhow!("rule '{}' not found in workflow", rule_name))?;

                // `localrule = true` promises the rule never leaves this
                // machine. A generated cluster script breaks that promise the
                // moment it is submitted — refuse to render one, naming the
                // rule, instead of letting the promise fail silently (issue
                // #1013 E2).
                if rule.localrule {
                    return Err(anyhow::anyhow!(
                        "`localrule = true` cannot be honored by `cluster submit` — rule '{rule_name}' \
                         would leave this machine the moment its script is submitted; remove \
                         `localrule = true` or run the workflow on the local path (`oxo-flow run`) instead"
                    ));
                }

                // Lifecycle hooks are run by the local executor only. `cluster
                // submit` renders just the command, so a declared rule hook
                // would be silently dropped (issue #1340). Refuse to render a
                // script, naming the rule and the hook field, rather than
                // no-op it. The hook fields come from the shared
                // `Rule::lifecycle_hooks` definition (reuse, never retype).
                for (field, hook) in rule.lifecycle_hooks() {
                    if hook.is_some() {
                        return Err(anyhow::anyhow!(
                            "lifecycle hook `{field}` on rule '{rule_name}' is not supported by \
                             `cluster submit` — the job script renders only the command, so the \
                             hook would be silently dropped on the scheduler; remove it or run \
                             the workflow on the local path (`oxo-flow run`) instead"
                        ));
                    }
                }

                let shell_cmd = match oxo_flow_core::executor::process::build_execution_command(
                    rule,
                    &wildcard_values,
                    &config.workflow.interpreter_map,
                    oxo_flow_core::scheduler::detect_system_limits(),
                ) {
                    Ok(Some(cmd)) => cmd,
                    Ok(None) => {
                        eprintln!(
                            "  {} {} — no shell command or script, skipping",
                            "⊘".yellow(),
                            rule_name
                        );
                        continue;
                    }
                    Err(e) => {
                        return Err(e).with_context(|| {
                            format!("rule '{rule_name}' command construction failed")
                        });
                    }
                };

                // Replace sensitive literals with their shared references
                // BEFORE the environment wrapper is built, so the wrapper
                // itself (and the manifest it feeds) never sees the value.
                let shell_cmd = if sensitive_bindings.is_empty() {
                    shell_cmd
                } else {
                    oxo_flow_core::executor::process::route_sensitive_values_with_bindings(
                        &shell_cmd,
                        &sensitive_bindings,
                    )
                    .0
                };

                // Render through the ExecutorBackend trait (issue #78): the
                // command stays a thin render layer over the same directive
                // generator the live submit path uses.
                let wrapped_cmd = env_resolver
                    .wrap_command(
                        &shell_cmd,
                        &rule.environment,
                        Some(&rule.resources),
                        &cluster_workdir,
                    )
                    .map_err(|e| anyhow::anyhow!("environment wrapping failed: {}", e))?;
                let mut wrapped_cmd = match tmp_dir.as_deref() {
                    Some(tmp) => oxo_flow_core::backend::with_tmp_dir_exports(&wrapped_cmd, tmp),
                    None => wrapped_cmd,
                };
                // HPC: the value rides a private env file beside the script,
                // which the job shell sources before the wrapped command. The
                // script itself carries only the reference. A dry run renders
                // the very script a submit would but never writes the secret
                // file (a preview must not persist values).
                if hpc_scripts {
                    let refs = oxo_flow_core::executor::process::sensitive_env_refs(&wrapped_cmd);
                    if !refs.is_empty() {
                        let env_path = output.join(format!("{rule_name}.secrets.env"));
                        if !dry_run {
                            oxo_flow_core::backend::write_sensitive_env_file(
                                &env_path,
                                &sensitive_bindings,
                                &refs,
                                rule.environment.kind(),
                            )
                            .map_err(|e| {
                                anyhow::anyhow!("cannot write {}: {e}", env_path.display())
                            })?;
                            secret_env_files.push(env_path.clone());
                        }
                        wrapped_cmd = format!(
                            "{} && {}",
                            oxo_flow_core::backend::source_sensitive_env_file(&env_path),
                            wrapped_cmd
                        );
                    }
                }
                let scheduled = oxo_flow_core::backend::ScheduledRule {
                    rule: rule.clone(),
                    // The standalone `cluster submit` command has no
                    // expansion templates to consult — an instance maps to
                    // itself.
                    template: rule_name.clone(),
                    // Issue #1388: pair the rendered environment with the
                    // wrapper-free command, so `cluster submit -b kubernetes`
                    // renders an image-renderable environment exactly like
                    // `run --profile` does. The scratch exports ride both
                    // forms.
                    container_image: oxo_flow_core::backend::container_image_render(
                        rule,
                        &match tmp_dir.as_deref() {
                            Some(tmp) => {
                                oxo_flow_core::backend::with_tmp_dir_exports(&shell_cmd, tmp)
                            }
                            None => shell_cmd.clone(),
                        },
                    ),
                    shell_cmd: wrapped_cmd,
                    workdir: cluster_workdir.clone(),
                    dependencies: dag.dependencies(rule_name).unwrap_or_default(),
                    wildcard_values: wildcard_values.clone(),
                };
                let script = executor.render_script(&scheduled)?;

                let script_path = output.join(format!("{rule_name}.sh"));
                std::fs::write(&script_path, &script)?;
                eprintln!("  {} {}", "✓".green(), script_path.display());
            }

            // The sidecar files hold the routed values until the operator
            // removes them: this path has no driver to notice job completion.
            // The `run --profile` path deletes its own copy as each job
            // settles (issue #1120 / RB-12).
            if !secret_env_files.is_empty() {
                eprintln!(
                    "\n{} {} secret env file(s) written with mode 0600: {}",
                    "Warning:".bold().yellow(),
                    secret_env_files.len(),
                    secret_env_files
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                eprintln!(
                    "  The job shells source them; remove them after the run finishes \
                     (nothing deletes them automatically on this path)."
                );
            }

            // Generate dependency-aware submit script if requested
            if with_dependencies && !dry_run {
                let submit_script = generate_submit_wrapper(
                    &cluster_backend,
                    &order,
                    &dag,
                    &output,
                    cluster_config.queue.as_deref(),
                )?;
                let submit_path = output.join("submit.sh");
                std::fs::write(&submit_path, submit_script)?;
                eprintln!(
                    "  {} {} (dependency-aware submit script)",
                    "✓".green(),
                    submit_path.display()
                );

                eprintln!(
                    "\n{} {} scripts written to {}",
                    "Done:".bold(),
                    order.len() + 1,
                    output.display()
                );
                eprintln!("  Submit with: bash {}", submit_path.display());
                eprintln!(
                    "  Or manually: {}",
                    oxo_flow_core::cluster::submit_invocation_hint(
                        &cluster_backend,
                        &output.display().to_string(),
                        cluster_config.queue.as_deref(),
                    )
                );
            } else {
                eprintln!(
                    "\n{} {} scripts written to {}",
                    "Done:".bold(),
                    order.len(),
                    output.display()
                );
                if dry_run {
                    eprintln!("  DRY-RUN: review the scripts, then submit with:");
                    eprintln!(
                        "    {}",
                        oxo_flow_core::cluster::submit_invocation_hint(
                            &cluster_backend,
                            &output.display().to_string(),
                            cluster_config.queue.as_deref(),
                        )
                    );
                } else {
                    eprintln!(
                        "  Submit with: {}",
                        oxo_flow_core::cluster::submit_invocation_hint(
                            &cluster_backend,
                            &output.display().to_string(),
                            cluster_config.queue.as_deref(),
                        )
                    );
                }
            }
        }

        ClusterAction::Status {
            backend,
            job_ids,
            run_dir,
            queue,
            all,
        } => {
            let cluster_backend = resolve_backend(backend.as_deref())?;

            // Run-aware mode (issue #1085): an explicit `--run-dir`, or the
            // workflow's newest run directory when one exists. `--all` and
            // checkouts that never hosted a cluster run keep the raw
            // scheduler view. Probe the default ONCE: the old guard probed
            // it and then re-derived it for the value, so a run directory
            // rotating in between reached an `expect` with `None`.
            let default_dir = if all { None } else { default_run_dir() };
            if !all && (run_dir.is_some() || default_dir.is_some()) {
                let dir = resolve_run_aware_dir(run_dir.clone(), default_dir)?;
                let mut instances = load_run_instances(&dir)?;
                if !job_ids.is_empty() {
                    instances.retain(|instance| {
                        job_ids.iter().any(|query| {
                            query == &instance.instance
                                || instance.job_id.as_deref() == Some(query.as_str())
                        })
                    });
                }

                // Best-effort live-state overlay (PENDING is written at
                // submission; the driver only updates it on settlement).
                // A scheduler failure must not hide the recorded state.
                let ids = instance_job_ids(&instances);
                if !ids.is_empty() {
                    let queue_for_status = queue.clone();
                    let report = tokio::task::spawn_blocking(move || {
                        query_status(
                            &cluster_backend,
                            &ids,
                            queue_for_status.as_deref(),
                            |program, args| {
                                let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                                let output = std::process::Command::new(program)
                                    .args(&arg_refs)
                                    .output()
                                    .map_err(|e| {
                                        anyhow::anyhow!("Failed to execute {program}: {e}")
                                    })?;
                                Ok(String::from_utf8_lossy(&output.stdout).to_string())
                            },
                        )
                    })
                    .await
                    .context("status query task failed")?;
                    if let Ok(report) = report {
                        overlay_scheduler_states(&mut instances, &report);
                    }
                }

                let counts = state_counts(&instances);
                if json {
                    let doc = serde_json::json!({
                        "run_dir": dir.display().to_string(),
                        "counts": counts,
                        "instances": instances,
                    });
                    println!("{}", serde_json::to_string_pretty(&doc)?);
                    return Ok(());
                }

                eprintln!("{} run {}", "Cluster:".bold().cyan(), dir.display());
                if counts.is_empty() {
                    eprintln!("  no rule instances recorded");
                } else {
                    let summary: Vec<String> = counts
                        .iter()
                        .map(|(state, count)| format!("{state} {count}"))
                        .collect();
                    eprintln!("  {}", summary.join("   "));
                }
                if !instances.is_empty() {
                    let width = instances
                        .iter()
                        .map(|i| i.instance.len())
                        .max()
                        .unwrap_or(8)
                        .max("instance".len());
                    eprintln!(
                        "  {:<width$}  {:<8}  {:<10}  {:<10}  exit",
                        "instance", "job", "state", "elapsed"
                    );
                    for instance in instances.iter().take(STATUS_ROW_LIMIT) {
                        eprintln!(
                            "  {:<width$}  {:<8}  {:<10}  {:<10}  {}",
                            instance.instance,
                            instance.job_id.as_deref().unwrap_or("-"),
                            instance.state,
                            instance.elapsed_label(),
                            instance
                                .exit_code
                                .map(|code| code.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        );
                    }
                    if instances.len() > STATUS_ROW_LIMIT {
                        eprintln!(
                            "  … {} more (use --json for the full list)",
                            instances.len() - STATUS_ROW_LIMIT
                        );
                    }
                }

                // Machine-readable form on stdout (one tab-separated row
                // per instance). In a TTY the human table above is the
                // whole answer — printing the same list twice was issue
                // #1085 C1.
                if !std::io::stdout().is_terminal() {
                    for instance in &instances {
                        println!("{}", instance_tsv(instance));
                    }
                }
                return Ok(());
            }

            if job_ids.is_empty() {
                // No ids: answer "what of MINE is in the queue right now" —
                // the natural first question after a submit, and the one the
                // old hard error left unanswered (the command used to bail
                // with "requires at least one job ID").
                //
                // Tencent Batch's DescribeJobs has no per-user or per-queue
                // scope (only job ids or attribute filters), so an empty
                // listing there means "unsupported", not "no jobs" — say so
                // instead of printing a misleading zero.
                if !oxo_flow_core::backend::cluster::supports_my_jobs_listing(&cluster_backend) {
                    eprintln!(
                        "{} raw job listing is not supported for {} — pass job ids or use \
                         a run-based view (`cluster status --run-dir <DIR>`)",
                        "Note:".yellow(),
                        cluster_backend
                    );
                    if json {
                        println!("[]");
                    }
                    return Ok(());
                }
                // M-8: the scheduler round-trip is a blocking subprocess —
                // off-thread so this async command body never parks a
                // tokio worker.
                let listing = tokio::task::spawn_blocking(move || {
                    query_my_jobs(&cluster_backend, queue.as_deref(), |program, args| {
                        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                        let output =
                            match std::process::Command::new(program).args(&arg_refs).output() {
                                Ok(out) => out,
                                Err(e) => {
                                    eprintln!("  Is {program} installed on this system?");
                                    anyhow::bail!("Failed to execute status command: {e}");
                                }
                            };
                        if !output.status.success() {
                            // bjobs exits non-zero when the queue is empty;
                            // treat that as an empty answer, not a failure.
                            eprintln!(
                                "{} exited {} (no jobs listed)",
                                program,
                                output.status.code().unwrap_or(-1)
                            );
                            return Ok(String::new());
                        }
                        Ok(String::from_utf8_lossy(&output.stdout).to_string())
                    })
                })
                .await
                .context("status query task failed")?;
                let listing = listing?;

                eprintln!(
                    "{} {}",
                    "Cluster:".bold().cyan(),
                    oxo_flow_core::plural(listing.len(), "queued job", "queued jobs")
                );
                for (id, status) in &listing {
                    eprintln!("{}", status_line(id, *status));
                }
                // Machine-readable form on stdout, one `<id>\t<state>` per
                // job — human output stays on stderr, the codebase-wide
                // convention. A TTY keeps only the human listing (issue
                // #1085 C1: the same list printed twice).
                if json {
                    let jobs: Vec<serde_json::Value> = listing
                        .iter()
                        .map(|(id, status)| {
                            serde_json::json!({"job_id": id, "state": status.label()})
                        })
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&jobs)?);
                } else if !std::io::stdout().is_terminal() {
                    for (id, status) in &listing {
                        println!("{id}\t{}", status.label());
                    }
                }
                return Ok(());
            }
            for (program, args) in oxo_flow_core::backend::cluster::status_invocations(
                &cluster_backend,
                &job_ids,
                queue.as_deref(),
            ) {
                eprintln!(
                    "{} Executing '{} {}'...",
                    "Cluster:".bold().cyan(),
                    program,
                    args.join(" ")
                );
            }

            let mut report = {
                let job_ids = job_ids.clone();
                let queue_for_status = queue.clone();
                tokio::task::spawn_blocking(move || {
                    query_status(
                        &cluster_backend,
                        &job_ids,
                        queue_for_status.as_deref(),
                        |program, args| {
                            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                            let output = match std::process::Command::new(program)
                                .args(&arg_refs)
                                .output()
                            {
                                Ok(out) => out,
                                Err(e) => {
                                    eprintln!("  Is {program} installed on this system?");
                                    anyhow::bail!("Failed to execute status command: {e}");
                                }
                            };
                            if !output.status.success() {
                                // A scheduler exits non-zero for "no such job" (qstat -j)
                                // while still naming the others; what it printed still
                                // answers for the ids it did list.
                                eprintln!(
                                    "{} exited {}",
                                    program,
                                    output.status.code().unwrap_or(-1)
                                );
                            }
                            Ok(String::from_utf8_lossy(&output.stdout).to_string())
                        },
                    )
                })
                .await
                .context("status query task failed")??
            };

            eprintln!(
                "{} {}:",
                "Cluster:".bold().cyan(),
                oxo_flow_core::plural(report.len(), "job", "jobs")
            );
            // The live queue only answers for jobs still in it. A finished
            // job is invisible to squeue/qstat, yet that is exactly the
            // state a user asks about most — layer the accounting store
            // over the Unknown verdicts, the same way the run driver does
            // (see the executor's terminal_status and issue #244's
            // no-slurmdbd story). A job with no accounting record either
            // keeps its Unknown ("finished, or an unknown id") below.
            let unknown: Vec<String> = report
                .iter()
                .filter(|(_, s)| *s == BackendJobStatus::Unknown)
                .map(|(id, _)| id.clone())
                .collect();
            if !unknown.is_empty() {
                let executor = oxo_flow_core::backend::cluster::ClusterExecutor::new(
                    cluster_backend,
                    ClusterJobConfig {
                        backend: cluster_backend,
                        queue: None,
                        account: None,
                        walltime: None,
                        extra_args: Vec::new(),
                        name_scope: None,
                        volume: None,
                    },
                );
                for id in &unknown {
                    if let Some(record) = executor.terminal_status(id).await
                        && let Some(entry) = report.iter_mut().find(|(rid, _)| rid == id)
                    {
                        // `id` came from `report`, so the find cannot miss;
                        // a skipped overlay is the safe answer if it ever
                        // does, never a panic in a status command.
                        entry.1 = record.status;
                    }
                }
            }
            for (id, status) in &report {
                eprintln!("{}", status_line(id, *status));
            }
            // Machine-readable form on stdout, one `<id>\t<state>` per job —
            // human output stays on stderr, the codebase-wide convention. A
            // TTY keeps only the human listing (issue #1085 C1).
            if json {
                let jobs: Vec<serde_json::Value> = report
                    .iter()
                    .map(|(id, status)| serde_json::json!({"job_id": id, "state": status.label()}))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&jobs)?);
            } else if !std::io::stdout().is_terminal() {
                for (id, status) in &report {
                    println!("{id}\t{}", status.label());
                }
            }
        }

        ClusterAction::Cancel {
            backend,
            job_ids,
            queue,
        } => {
            let cluster_backend = resolve_backend(backend.as_deref())?;
            // One shared invocation mapping with the driver
            // (`cancel_invocations`): k8s needs `kubectl delete job`, the
            // cloud APIs cancel one id per API call, and the local
            // schedulers take all ids at once (issue #1158 live campaign).
            // `--queue` supplies the GCP location / Tencent zone a cloud
            // CLI needs to resolve a bare id.
            let invocations = oxo_flow_core::backend::cluster::cancel_invocations(
                &cluster_backend,
                &job_ids,
                queue.as_deref(),
            );

            // job_ids is clap-enforced as required, so the empty case never
            // reaches here — usage errors surface at argument parsing with
            // exit code 2, matching `cluster logs` (missing JOB_ID).
            eprintln!(
                "{} Canceling {}...",
                "Cluster:".bold().cyan(),
                oxo_flow_core::plural(job_ids.len(), "job", "jobs")
            );

            let mut outputs: Vec<(String, String)> = Vec::new();
            for (program, args) in invocations {
                let display = format!("{program} {}", args.join(" "));
                let result = tokio::task::spawn_blocking(move || {
                    std::process::Command::new(&program).args(&args).output()
                })
                .await
                .context("cancel task failed")?;
                match result {
                    Ok(out) if out.status.success() => {
                        outputs.push((display, String::from_utf8_lossy(&out.stdout).into_owned()))
                    }
                    Ok(out) => anyhow::bail!(
                        "Command failed with exit code: {}",
                        out.status.code().unwrap_or(-1)
                    ),
                    Err(e) => {
                        eprintln!("  Is the cancel command installed on this system?");
                        anyhow::bail!("Failed to execute cancel command: {}", e);
                    }
                }
            }

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "backend": cluster_backend.to_string(),
                        "job_ids": job_ids,
                        "success": true,
                        "invocations": outputs
                            .iter()
                            .map(|(command, output)| serde_json::json!({
                                "command": command,
                                "output": output,
                            }))
                            .collect::<Vec<_>>(),
                    }))?
                );
                return Ok(());
            }
            eprintln!("{} Successfully canceled jobs.", "✓".green());
        }

        ClusterAction::Logs {
            backend,
            job_id,
            run_dir,
            queue,
            tail,
        } => {
            // Issue #1085 C3: `logs` used to REQUIRE the accounting store
            // (`sacct`) and fail outright on sites without slurmdbd. The
            // chain is now run-dir capture → accounting → scontrol, and the
            // argument may be a rule instance as well as a scheduler id.
            let cluster_backend = resolve_backend(backend.as_deref())?;
            let dir = run_dir.clone().or_else(default_run_dir);
            let mut instance: Option<RunInstance> = None;
            let mut resolved_id = job_id.clone();
            if let Some(dir) = &dir
                && let Ok(instances) = load_run_instances(dir)
            {
                instance = instances
                    .iter()
                    .find(|i| i.instance == job_id)
                    .or_else(|| {
                        instances
                            .iter()
                            .find(|i| i.job_id.as_deref() == Some(job_id.as_str()))
                    })
                    .cloned();
                if let Some(found) = &instance
                    && let Some(id) = &found.job_id
                {
                    resolved_id = id.clone();
                }
            }

            // 1. The engine's own capture. Present for every submitted job,
            //    accounting or not.
            let captured = instance.as_ref().and_then(|found| {
                let dir = dir.as_deref()?;
                let workdir = run_workdir(dir)?;
                read_captured_logs(
                    &workdir,
                    &found.instance,
                    Some(&dir.join("jobs").join(&found.instance)),
                    cluster_backend,
                )
            });

            // 2. Accounting store (sacct / qstat -x / qacct / bacct).
            let executor = oxo_flow_core::backend::cluster::ClusterExecutor::new(
                cluster_backend,
                oxo_flow_core::cluster::ClusterJobConfig {
                    backend: cluster_backend,
                    queue: queue.clone(),
                    account: None,
                    walltime: None,
                    extra_args: Vec::new(),
                    name_scope: None,
                    volume: None,
                },
            );
            let accounting = executor.logs(&resolved_id).await.ok();

            // 3. The controller's in-memory record (SLURM), which survives
            //    briefly after a job leaves the queue and needs no slurmdbd.
            let scontrol = if cluster_backend == ClusterBackend::Slurm {
                tokio::process::Command::new("scontrol")
                    .args(["show", "job", &resolved_id])
                    .output()
                    .await
                    .ok()
                    .filter(|out| out.status.success())
                    .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
            } else {
                None
            };

            let Some((source, logs)) = first_log_source(vec![
                ("run-dir", captured),
                ("accounting", accounting),
                ("scontrol", scontrol),
            ]) else {
                anyhow::bail!(
                    "no logs found for '{job_id}' — the run directory has no captured \
                     logs (try --run-dir) and the scheduler returned neither an \
                     accounting nor a controller record"
                );
            };
            let logs = tail_lines(&logs, tail);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "job_id": resolved_id,
                        "instance": instance.as_ref().map(|i| i.instance.clone()),
                        "source": source,
                        "logs": logs,
                    }))?
                );
            } else {
                eprintln!(
                    "{} logs for {} (source: {source})",
                    "Cluster:".bold().cyan(),
                    resolved_id
                );
                println!("{logs}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        cluster_command, first_log_source, generate_submit_helper, generate_submit_wrapper,
        instance_tsv, load_run_instances, overlay_scheduler_states, query_my_jobs, query_status,
        read_captured_logs, resolve_backend_with, resolve_run_aware_dir, run_workdir, state_counts,
        status_line, tail_lines,
    };
    use crate::ClusterAction;
    use oxo_flow_core::backend::BackendJobStatus;
    use oxo_flow_core::cluster::ClusterBackend;
    use oxo_flow_core::config::WorkflowConfig;
    use oxo_flow_core::dag::WorkflowDag;
    use std::path::PathBuf;

    fn run(action: ClusterAction) -> anyhow::Result<()> {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(cluster_command(action, false))
    }

    fn workflow_file(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("wf.oxoflow");
        std::fs::write(
            &path,
            "[workflow]\nname = \"wf\"\n[[rules]]\nname = \"align\"\n\
             shell = \"true\"\noutput = [\"out.txt\"]\n",
        )
        .unwrap();
        path
    }

    /// The run-aware directory is resolved from the caller's single
    /// `default_run_dir()` probe; an absent pair is a normal, actionable
    /// error — never the panic the old double-probe guard could reach when
    /// `runs/latest` disappeared between the guard and the use.
    #[test]
    fn resolve_run_aware_dir_prefers_explicit_and_errors_when_absent() {
        let explicit = PathBuf::from("/runs/explicit");
        let default = PathBuf::from("/runs/latest");

        // An explicit --run-dir wins; the default is only the fallback.
        assert_eq!(
            resolve_run_aware_dir(Some(explicit.clone()), Some(default.clone())).unwrap(),
            explicit
        );
        assert_eq!(
            resolve_run_aware_dir(Some(explicit.clone()), None).unwrap(),
            explicit
        );
        // No explicit directory: the probed default stands in.
        assert_eq!(
            resolve_run_aware_dir(None, Some(default.clone())).unwrap(),
            default
        );
        // Neither: an error naming the fix, not a panic.
        let message = resolve_run_aware_dir(None, None).unwrap_err().to_string();
        assert!(
            message.contains("--run-dir") && message.contains(".oxo-flow/runs"),
            "the error must name the inputs and the escape hatch: {message}"
        );
    }

    fn localrule_workflow_file(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("local.oxoflow");
        std::fs::write(
            &path,
            "[workflow]\nname = \"wf\"\n[[rules]]\nname = \"stage_refs\"\n\
             localrule = true\nshell = \"true\"\noutput = [\"out.txt\"]\n",
        )
        .unwrap();
        path
    }

    fn submit_action(
        workflow: PathBuf,
        output: PathBuf,
        backend: &str,
        dry_run: bool,
    ) -> ClusterAction {
        ClusterAction::Submit {
            workflow,
            backend: Some(backend.to_string()),
            queue: None,
            account: None,
            walltime: None,
            extra_args: Vec::new(),
            output,
            target: Vec::new(),
            module: Vec::new(),
            dry_run,
            with_dependencies: true,
        }
    }

    /// A DAG where each `sink` depends on every `source`.
    fn dag_with(sinks: &[&str], sources: &[&str]) -> WorkflowDag {
        let mut toml = String::from("[workflow]\nname = \"dag\"\n");
        for name in sources {
            toml.push_str(&format!(
                "[[rules]]\nname = \"{name}\"\nshell = \"true\"\noutput = [\"{name}.out\"]\n"
            ));
        }
        for sink in sinks {
            let deps: Vec<String> = sources.iter().map(|s| format!("\"{s}\"")).collect();
            toml.push_str(&format!(
                "[[rules]]\nname = \"{sink}\"\ndepends_on = [{}]\nshell = \"true\"\noutput = [\"{sink}.out\"]\n",
                deps.join(", ")
            ));
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dag.oxoflow");
        std::fs::write(&path, toml).unwrap();
        let config = WorkflowConfig::from_file(&path).unwrap();
        WorkflowDag::from_rules(&config.rules).unwrap()
    }

    #[test]
    fn unknown_backend_is_rejected_for_every_action() {
        // A typo used to fall back to SLURM silently — submit scripts for the
        // wrong scheduler, scancel where the user asked for qdel. An EXPLICIT
        // bad value must still be rejected even though an omitted flag now
        // defaults to slurm.
        let cases = vec![
            ClusterAction::Status {
                backend: Some("slrm".to_string()),
                job_ids: vec!["1".to_string()],
                run_dir: None,
                queue: None,
                all: false,
            },
            ClusterAction::Cancel {
                backend: Some("torque".to_string()),
                job_ids: vec!["1".to_string()],
                queue: None,
            },
            ClusterAction::Logs {
                backend: Some("ge".to_string()),
                job_id: "1".to_string(),
                run_dir: None,
                queue: None,
                tail: None,
            },
        ];
        for action in cases {
            let msg = run(action).unwrap_err().to_string();
            assert!(
                msg.starts_with("unknown cluster backend '"),
                "must name the offending backend: {msg}"
            );
            assert!(
                msg.contains(oxo_flow_core::cluster::SUPPORTED_BACKENDS_HINT),
                "must list the accepted values: {msg}"
            );
        }
    }

    #[test]
    fn submit_rejects_an_unknown_backend_before_reading_the_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("absent.oxoflow");
        let err = run(submit_action(
            missing,
            dir.path().join("scripts"),
            "slurm2",
            true,
        ))
        .unwrap_err();
        assert!(
            err.to_string().contains("unknown cluster backend 'slurm2'"),
            "the backend is validated before anything else: {err}"
        );
    }

    #[test]
    fn parse_backend_accepts_every_documented_name() {
        for name in ["slurm", "SLURM", "pbs", "sge", "lsf"] {
            assert!(
                resolve_backend_with(Some(name), None).is_ok(),
                "{name} must resolve"
            );
        }
    }

    #[test]
    fn resolve_backend_falls_back_to_slurm_when_the_flag_is_omitted() {
        // The -b flag used to be required on every cluster action; typing it
        // on a SLURM site is pure ceremony. Omitted = slurm, unless the
        // environment names another scheduler.
        assert_eq!(
            resolve_backend_with(None, None).unwrap(),
            ClusterBackend::Slurm
        );
        assert_eq!(
            resolve_backend_with(Some("sge"), Some("pbs")).unwrap(),
            ClusterBackend::Sge
        );
    }

    #[test]
    fn resolve_backend_honors_the_environment_default() {
        // A PBS-site user sets OXO_FLOW_CLUSTER_BACKEND once and never types
        // -b again. Whitespace around the value is tolerated (export typos),
        // an empty value falls through to the slurm default, and an explicit
        // flag still wins over the environment.
        assert_eq!(
            resolve_backend_with(None, Some("pbs")).unwrap(),
            ClusterBackend::Pbs
        );
        assert_eq!(
            resolve_backend_with(Some("lsf"), Some("pbs")).unwrap(),
            ClusterBackend::Lsf
        );
        assert_eq!(
            resolve_backend_with(None, Some("  sge  ")).unwrap(),
            ClusterBackend::Sge
        );
        assert_eq!(
            resolve_backend_with(None, Some("")).unwrap(),
            ClusterBackend::Slurm
        );
        assert_eq!(
            resolve_backend_with(None, Some("   ")).unwrap(),
            ClusterBackend::Slurm
        );

        // And a garbage environment value is an error naming the value —
        // never a silent slurm fallback.
        let msg = resolve_backend_with(None, Some("torque"))
            .unwrap_err()
            .to_string();
        assert!(msg.contains("unknown cluster backend 'torque'"), "{msg}");
    }

    #[test]
    fn query_my_jobs_parses_the_listing() {
        // The seam question: exactly the user-listing invocation, and the
        // returned rows are parsed (id, status) — header lines skipped.
        let listing = query_my_jobs(&ClusterBackend::Slurm, None, |program, args| {
            assert_eq!(program, "squeue");
            assert_eq!(args[0], "-u");
            assert_eq!(args[2], "--noheader");
            Ok("101|PENDING\n202|RUNNING\n".to_string())
        })
        .unwrap();
        assert_eq!(
            listing,
            vec![
                ("101".to_string(), BackendJobStatus::Pending),
                ("202".to_string(), BackendJobStatus::Running),
            ]
        );
    }

    #[test]
    fn query_my_jobs_surfaces_scheduler_failures() {
        let err = query_my_jobs(&ClusterBackend::Slurm, None, |_, _| {
            Err(anyhow::anyhow!("squeue: slurm_load_jobs error"))
        })
        .unwrap_err();
        assert!(
            err.to_string().contains("slurm_load_jobs"),
            "the scheduler's own words must reach the user: {err}"
        );
    }

    /// AWS needs one list-jobs call per live status; the answer is the
    /// merge of every call (first id wins on a transition between calls),
    /// not just the first invocation's rows.
    #[test]
    fn query_my_jobs_merges_every_listing_invocation() {
        let listing = query_my_jobs(&ClusterBackend::AwsBatch, Some("q"), |program, args| {
            assert_eq!(program, "aws");
            let at = args.iter().position(|a| a == "--job-status").unwrap();
            Ok(match args[at + 1].as_str() {
                "PENDING" => "job-a|PENDING\njob-b|PENDING\n".to_string(),
                "RUNNING" => "job-b|RUNNING\njob-c|RUNNING\n".to_string(),
                _ => String::new(),
            })
        })
        .unwrap();
        assert_eq!(
            listing,
            vec![
                ("job-a".to_string(), BackendJobStatus::Pending),
                ("job-b".to_string(), BackendJobStatus::Pending),
                ("job-c".to_string(), BackendJobStatus::Running),
            ]
        );
    }

    #[test]
    fn status_answers_every_requested_id() {
        // SGE asks once per job: two invocations must not lose an answer, and
        // an id the scheduler is silent about reads as unknown — not missing.
        let report = query_status(
            &oxo_flow_core::cluster::ClusterBackend::Sge,
            &["1".to_string(), "2".to_string()],
            None,
            |program, args| {
                assert_eq!(program, "qstat");
                // Job 1 is running; job 2 already left the queue — qstat
                // prints nothing for it.
                match args[1].as_str() {
                    "1" => Ok("job_number: 1\nstate: r\n".to_string()),
                    _ => Ok(String::new()),
                }
            },
        )
        .unwrap();
        assert_eq!(report.len(), 2);
        assert_eq!(report[0], ("1".to_string(), BackendJobStatus::Running));
        assert_eq!(report[1], ("2".to_string(), BackendJobStatus::Unknown));
    }

    #[test]
    fn status_surfaces_scheduler_failures() {
        let err = query_status(
            &oxo_flow_core::cluster::ClusterBackend::Slurm,
            &["1".to_string()],
            None,
            |_, _| Err(anyhow::anyhow!("squeue: slurm_load_jobs error")),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("slurm_load_jobs"),
            "the scheduler's own words must reach the user: {err}"
        );
    }

    #[test]
    fn status_line_explains_a_missing_job() {
        assert_eq!(
            status_line("123", BackendJobStatus::Unknown),
            "  123: not in the queue (finished, or an unknown id)"
        );
        assert_eq!(
            status_line("123", BackendJobStatus::Running),
            "  123: running"
        );
    }

    #[test]
    fn dry_run_writes_the_scripts_but_no_wrapper() {
        // A dry run exists so an HPC user can read the #SBATCH headers before
        // the queue does — it has to leave them on disk, and must leave
        // nothing behind that would submit them.
        let dir = tempfile::tempdir().unwrap();
        let scripts = dir.path().join("scripts");
        let workflow = workflow_file(dir.path());
        run(submit_action(
            workflow.clone(),
            scripts.clone(),
            "slurm",
            true,
        ))
        .unwrap();
        let body = std::fs::read_to_string(scripts.join("align.sh")).unwrap();
        assert!(
            body.contains("#SBATCH --job-name=align"),
            "the generated header must be reviewable: {body}"
        );
        assert!(
            body.contains("#SBATCH --chdir="),
            "the generated header pins the working directory: {body}"
        );
        assert!(
            !scripts.join("submit.sh").exists(),
            "a dry run must not leave a submit wrapper behind"
        );

        // The real path keeps the wrapper it always wrote.
        std::fs::remove_dir_all(&scripts).unwrap();
        run(submit_action(workflow, scripts.clone(), "slurm", false)).unwrap();
        assert!(scripts.join("submit.sh").exists());
    }

    /// Issue #1388: `cluster submit -b kubernetes` renders a docker /
    /// singularity-`docker://` environment as the pod image and drops the
    /// local container wrapper from the job command, exactly like
    /// `run --profile`. A non-renderable environment keeps the wrapper and is
    /// marked `wrapper-fallback` in the manifest.
    #[test]
    fn submit_renders_container_environments_as_kubernetes_pod_images() {
        // The text the Job actually executes: behind the bash shim for an
        // image-rendered rule (`args = [shim, "sh", text]`), bare for the
        // historical fallback (`args = [text]`). Asserting on `args[0]`
        // alone would inspect the shim and pass vacuously (review M4).
        let executed = |manifest: &serde_json::Value| -> String {
            let args = manifest["spec"]["template"]["spec"]["containers"][0]["args"]
                .as_array()
                .expect("container args")
                .clone();
            if args.first().and_then(|v| v.as_str())
                == Some(oxo_flow_core::environment::CONTAINER_BASH_SHIM)
            {
                args[2].as_str().expect("shim command text").to_string()
            } else {
                args[0].as_str().expect("command text").to_string()
            }
        };
        let dir = tempfile::tempdir().unwrap();
        let scripts = dir.path().join("scripts");
        let workflow = dir.path().join("wf.oxoflow");
        std::fs::write(
            &workflow,
            "[workflow]\nname = \"wf\"\n\
             [[rules]]\nname = \"align\"\nshell = \"bwa mem ref.fa > out.bam\"\n\
             output = [\"out.bam\"]\nenvironment = { singularity = \"docker://quay.io/x/bwa:1\" }\n",
        )
        .unwrap();
        run(submit_action(workflow, scripts.clone(), "kubernetes", true)).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(scripts.join("align.sh")).unwrap())
                .unwrap();
        let container = &manifest["spec"]["template"]["spec"]["containers"][0];
        assert_eq!(container["image"], "quay.io/x/bwa:1");
        assert_eq!(
            container["command"],
            serde_json::json!(["/bin/sh", "-c"]),
            "image mode starts the bash-if-available shim from /bin/sh"
        );
        let submitted = executed(&manifest);
        assert!(
            !submitted.contains("singularity exec") && !submitted.contains("apptainer exec"),
            "the executed command must not invoke the local wrapper: {submitted}"
        );
        assert!(
            submitted.contains("bwa mem ref.fa > out.bam"),
            "the executed text must be the rule command: {submitted}"
        );
        assert_eq!(
            manifest["metadata"]["annotations"]["oxoflow.io/environment-render"],
            "image"
        );

        // A conda environment cannot be baked into an image here: the local
        // wrapper stays and the manifest says so.
        let conda_dir = tempfile::tempdir().unwrap();
        let conda_scripts = conda_dir.path().join("scripts");
        let conda_workflow = conda_dir.path().join("wf.oxoflow");
        std::fs::write(
            &conda_workflow,
            "[workflow]\nname = \"wf\"\n\
             [[rules]]\nname = \"qc\"\nshell = \"fastqc in.fq\"\n\
             output = [\"qc.html\"]\nenvironment = { conda = \"envs/qc.yaml\" }\n",
        )
        .unwrap();
        run(submit_action(
            conda_workflow,
            conda_scripts.clone(),
            "kubernetes",
            true,
        ))
        .unwrap();
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(conda_scripts.join("qc.sh")).unwrap())
                .unwrap();
        let submitted = executed(&manifest);
        assert!(
            submitted.contains("conda run"),
            "the fallback must keep the wrapper: {submitted}"
        );
        assert_eq!(
            manifest["metadata"]["annotations"]["oxoflow.io/environment-render"],
            "wrapper-fallback"
        );
    }

    #[test]
    fn cluster_submit_refuses_a_localrule_workflow() {
        // E2: a `localrule = true` rule used to get a rendered cluster
        // script like any other — the directive's whole promise broken by
        // the next `bash submit.sh`. Submit must refuse instead.
        let dir = tempfile::tempdir().unwrap();
        let scripts = dir.path().join("scripts");
        let workflow = localrule_workflow_file(dir.path());
        let err = run(submit_action(workflow, scripts.clone(), "slurm", false)).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("stage_refs") && msg.contains("localrule"),
            "the refusal must name the rule and the directive: {msg}"
        );
        assert!(
            msg.contains("oxo-flow run"),
            "the refusal must point at the local path: {msg}"
        );
        assert!(
            !scripts.join("stage_refs.sh").exists(),
            "no cluster script may be written for a localrule rule"
        );
    }

    /// SLURM-only: straight array chains go element-wise via `aftercorr`;
    /// every guardrail shape keeps the ready-batch `afterok`.
    #[test]
    fn wrapper_chains_aftercorr_only_for_straight_array_pairs() {
        use oxo_flow_core::backend::cluster::elementwise_aftercorr;

        let dir = tempfile::tempdir().unwrap();
        let order = vec!["align".to_string(), "stats".to_string()];
        let dag = dag_with(&["stats"], &["align"]);
        let out = dir.path();

        // Straight chain, both arrays with the same range → aftercorr.
        std::fs::write(
            out.join("align.sh"),
            "#!/bin/bash\n#SBATCH --array=1-3\ntrue\n",
        )
        .unwrap();
        std::fs::write(
            out.join("stats.sh"),
            "#!/bin/bash\n#SBATCH --array=1-3\ntrue\n",
        )
        .unwrap();
        let wrapper =
            generate_submit_wrapper(&ClusterBackend::Slurm, &order, &dag, out, None).unwrap();
        assert!(
            wrapper
                .contains("JOB_IDS[stats]=$(oxo_submit --dependency=aftercorr:${JOB_IDS[align]}"),
            "straight array chain must go element-wise:\n{wrapper}"
        );
        assert!(
            !wrapper.contains("afterok:"),
            "no fallback may remain:\n{wrapper}"
        );

        // Mismatched range → afterok.
        std::fs::write(
            out.join("stats.sh"),
            "#!/bin/bash\n#SBATCH --array=1-4\ntrue\n",
        )
        .unwrap();
        let wrapper =
            generate_submit_wrapper(&ClusterBackend::Slurm, &order, &dag, out, None).unwrap();
        assert!(
            wrapper.contains("JOB_IDS[stats]=$(oxo_submit --dependency=afterok:${JOB_IDS[align]}"),
            "mismatched range must keep the ready-batch chain:\n{wrapper}"
        );

        // Scalar downstream → afterok.
        std::fs::write(out.join("stats.sh"), "#!/bin/bash\necho hi\n").unwrap();
        let wrapper =
            generate_submit_wrapper(&ClusterBackend::Slurm, &order, &dag, out, None).unwrap();
        assert!(
            wrapper.contains("afterok:"),
            "scalar downstream must keep afterok:\n{wrapper}"
        );

        // Non-SLURM backends never use aftercorr, arrays or not.
        std::fs::write(
            out.join("stats.sh"),
            "#!/bin/bash\n#SBATCH --array=1-3\ntrue\n",
        )
        .unwrap();
        for backend in [
            ClusterBackend::Pbs,
            ClusterBackend::Sge,
            ClusterBackend::Lsf,
        ] {
            let wrapper = generate_submit_wrapper(&backend, &order, &dag, out, None).unwrap();
            assert!(
                !wrapper.contains("aftercorr"),
                "{backend} must not emit aftercorr:\n{wrapper}"
            );
        }

        // The decision function itself stays honest: a fan-in (two deps) can
        // never be element-wise — `elementwise_aftercorr` is only reached
        // with one dep, and the wrapper's per-dep check below covers that.
        assert!(elementwise_aftercorr(
            "#SBATCH --array=1-3\n",
            "#SBATCH --array=1-3\n"
        ));
    }

    #[test]
    fn wrapper_falls_back_to_afterok_for_fan_in_and_missing_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let order = vec!["merge".to_string()];
        let dag = dag_with(&["merge"], &["a", "b"]);
        let out = dir.path();
        std::fs::write(out.join("a.sh"), "#!/bin/bash\n#SBATCH --array=1-3\ntrue\n").unwrap();
        std::fs::write(out.join("b.sh"), "#!/bin/bash\n#SBATCH --array=1-3\ntrue\n").unwrap();
        std::fs::write(
            out.join("merge.sh"),
            "#!/bin/bash\n#SBATCH --array=1-3\ntrue\n",
        )
        .unwrap();
        // Even with every script present and all three ranges identical, a
        // downstream with TWO dependencies must never go element-wise —
        // aftercorr can only pair one upstream per downstream.
        let wrapper =
            generate_submit_wrapper(&ClusterBackend::Slurm, &order, &dag, out, None).unwrap();
        assert!(
            wrapper.contains("--dependency=afterok:${JOB_IDS[a]}:${JOB_IDS[b]}")
                || wrapper.contains("--dependency=afterok:${JOB_IDS[b]}:${JOB_IDS[a]}"),
            "fan-in keeps the ready-batch chain:\n{wrapper}"
        );

        // A missing script must also degrade to afterok, never break
        // wrapper generation.
        std::fs::remove_file(out.join("merge.sh")).unwrap();
        let wrapper =
            generate_submit_wrapper(&ClusterBackend::Slurm, &order, &dag, out, None).unwrap();
        assert!(
            wrapper.contains("--dependency=afterok:${JOB_IDS[a]}:${JOB_IDS[b]}")
                || wrapper.contains("--dependency=afterok:${JOB_IDS[b]}:${JOB_IDS[a]}"),
            "missing merge.sh degrades to afterok:\n{wrapper}"
        );
    }

    // ── Run-aware status/logs (issue #1085) ───────────────────────────

    /// A realistic `run --profile` layout: two instances under
    /// `.oxo-flow/runs/<stamp>/jobs/`, plus the engine's captured logs.
    fn run_dir_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let wf = root.path().join("wf");
        let run_dir = wf.join(".oxo-flow/runs/2026-01-01T00-00-00-000");
        std::fs::create_dir_all(run_dir.join("jobs/mixcr_all_S1")).unwrap();
        std::fs::create_dir_all(run_dir.join("jobs/trust4_all_S1")).unwrap();
        std::fs::create_dir_all(wf.join("logs")).unwrap();
        std::fs::write(run_dir.join("jobs/mixcr_all_S1/job.id"), "1400_1\n").unwrap();
        std::fs::write(
            run_dir.join("jobs/mixcr_all_S1/status.json"),
            r#"{"state":"PENDING","job_id":"1400_1","command":"mixcr run"}"#,
        )
        .unwrap();
        std::fs::write(run_dir.join("jobs/trust4_all_S1/job.id"), "1401_2").unwrap();
        std::fs::write(
            run_dir.join("jobs/trust4_all_S1/status.json"),
            r#"{"state":"COMPLETED","job_id":"1401_2","submitted_at":"2026-01-01T00:00:00.000Z","finished_at":"2026-01-01T00:02:05.000Z","exit_code":0,"elapsed_secs":125}"#,
        )
        .unwrap();
        std::fs::write(wf.join("logs/mixcr_all_S1.out"), "alignment 93%\n").unwrap();
        std::fs::write(wf.join("logs/mixcr_all_S1.err"), "warn: low reads\n").unwrap();
        // Array-submission bookkeeping: a template container with a chunk
        // script and no per-instance files. It must not surface as a rule
        // instance.
        let chunk = run_dir.join("jobs/mixcr_template/chunk-1");
        std::fs::create_dir_all(&chunk).unwrap();
        std::fs::write(chunk.join("job.sh"), "#!/bin/bash\ntrue\n").unwrap();
        (root, wf, run_dir)
    }

    #[test]
    fn run_dir_maps_instances_to_scheduler_ids() {
        // Issue #1085 C2: `jobs/<instance>/job.id` already records the
        // mapping — status just never used it.
        let (_root, wf, run_dir) = run_dir_fixture();
        let mut instances = load_run_instances(&run_dir).unwrap();
        assert_eq!(
            instances.len(),
            2,
            "array chunk containers are not instances: {instances:?}"
        );
        assert_eq!(instances[0].instance, "mixcr_all_S1");
        assert_eq!(instances[0].job_id.as_deref(), Some("1400_1"));
        assert_eq!(instances[0].state, "pending");
        assert_eq!(instances[1].exit_code, Some(0));
        assert_eq!(instances[1].elapsed_label(), "0:02:05");
        assert_eq!(
            run_workdir(&run_dir),
            Some(wf.canonicalize().unwrap()),
            "the run directory must resolve back to its workflow workdir"
        );

        // The live scheduler overlays only its own verdicts; an Unknown
        // answer (job already left the queue) leaves the recorded state.
        overlay_scheduler_states(
            &mut instances,
            &[("1400_1".to_string(), BackendJobStatus::Running)],
        );
        assert_eq!(instances[0].state, "running");

        let counts = state_counts(&instances);
        assert_eq!(counts.get("running"), Some(&1));
        assert_eq!(counts.get("completed"), Some(&1));
        assert!(
            instance_tsv(&instances[0]).starts_with("mixcr_all_S1\t1400_1\trunning\t"),
            "machine row must carry instance/id/state: {}",
            instance_tsv(&instances[0])
        );
    }

    #[test]
    fn captured_logs_prefer_child_output_and_apply_tail() {
        // Issue #1085 C3: the engine writes logs/<instance>.out/.err for
        // every job, accounting or not.
        let (_root, wf, _run_dir) = run_dir_fixture();
        let logs = read_captured_logs(&wf, "mixcr_all_S1", None, ClusterBackend::Slurm).unwrap();
        assert!(logs.contains("alignment 93%"));
        assert!(logs.contains("warn: low reads"));
        assert!(read_captured_logs(&wf, "no_such_instance", None, ClusterBackend::Slurm).is_none());
        assert_eq!(tail_lines("a\nb\nc\n", Some(2)), "b\nc\n");
        assert_eq!(tail_lines("a\nb\n", None), "a\nb\n");
    }

    #[test]
    fn array_instance_logs_resolve_from_the_rendered_script() {
        // Real deployments submit thousands of sibling instances as
        // scheduler arrays; the log directive then carries `%A_%a`, not the
        // instance name (issue #1085 C3 — the capture must still be found).
        let root = tempfile::tempdir().unwrap();
        let wf = root.path().join("wf");
        let instance_dir = wf.join(".oxo-flow/runs/2026-01-01/jobs/mixcr_all_S1");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::create_dir_all(wf.join("logs")).unwrap();
        std::fs::write(instance_dir.join("job.id"), "1401_2").unwrap();
        std::fs::write(
            instance_dir.join("job.sh"),
            "#!/bin/bash\n#SBATCH --output=logs/mixcr.%A_%a.out\n#SBATCH --error=logs/mixcr.%A_%a.err\n",
        )
        .unwrap();
        std::fs::write(wf.join("logs/mixcr.1401_2.out"), "array element output\n").unwrap();

        let logs = read_captured_logs(
            &wf,
            "mixcr_all_S1",
            Some(&instance_dir),
            ClusterBackend::Slurm,
        )
        .unwrap();
        assert!(logs.contains("array element output"), "{logs}");
        assert!(
            logs.contains("mixcr.1401_2.out"),
            "source must be named: {logs}"
        );
    }

    #[test]
    fn logs_fallback_chain_skips_failed_sources() {
        // Issue #1085 C3: without sacct the command must fall through to
        // the next source, not exit with "failed to fetch job logs".
        assert_eq!(
            first_log_source(vec![
                ("run-dir", Some("captured".into())),
                ("accounting", Some("sacct".into())),
            ]),
            Some(("run-dir", "captured".to_string()))
        );
        assert_eq!(
            first_log_source(vec![
                ("run-dir", None),
                ("accounting", Some("   ".into())),
                ("scontrol", Some("JobState=RUNNING".into())),
            ]),
            Some(("scontrol", "JobState=RUNNING".to_string()))
        );
        assert_eq!(
            first_log_source(vec![("run-dir", None), ("accounting", None)]),
            None
        );
    }

    /// The dependency-aware submit wrapper shells the same CLI grammar the
    /// executor issues. The GCP helper used to call
    /// `gcloud batch jobs submit "$@" --query 'name' --format=…`: `--query`
    /// is an AWS CLI flag gcloud rejects as an unknown argument, the script
    /// path landed in the positional JOB slot, `--config` was missing, and
    /// `id="$out"` recorded the printed Job resource (not a bare id) as the
    /// dependency key. Pin the corrected shape, and check AWS/Tencent for
    /// the same class of bug.
    #[test]
    fn cloud_submit_helpers_match_the_vendor_cli_grammar() {
        let gcp = generate_submit_helper(&ClusterBackend::GcpBatch, Some("us-central1"));
        assert!(
            gcp.contains("gcloud batch jobs submit \"$id\" --config \"$1\""),
            "the id is positional and the manifest is a plain --config path: {gcp}"
        );
        assert!(
            !gcp.contains("--query"),
            "gcloud has no --query flag (AWS only): {gcp}"
        );
        assert!(
            !gcp.contains("file://"),
            "gcloud --config takes no URI scheme: {gcp}"
        );
        assert!(
            gcp.contains("--location 'us-central1'"),
            "the location resolves the bare id: {gcp}"
        );
        let aws = generate_submit_helper(&ClusterBackend::AwsBatch, None);
        assert!(
            aws.contains("--query 'jobId' --output text"),
            "aws-cli's --query is the real flag and yields the bare id: {aws}"
        );
        let tencent = generate_submit_helper(&ClusterBackend::TencentBatch, Some("ap-shanghai-5"));
        assert!(
            !tencent.contains("--query"),
            "tccli has no --query flag: {tencent}"
        );
        assert!(
            tencent.contains("--region 'ap-shanghai'"),
            "the zone-derived region scopes the call: {tencent}"
        );
        assert!(
            tencent.contains("JobId"),
            "the JobId is parsed out of the unwrapped tccli body: {tencent}"
        );
    }
}
