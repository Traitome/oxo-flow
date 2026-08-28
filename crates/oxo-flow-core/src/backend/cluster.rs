//! Cluster (SLURM/PBS/SGE/LSF) executor + shared scheduler-output parsing.
//!
//! Directive generation lives in `crate::cluster` and stays as-is (issue #74);
//! this module is the submission/tracking layer above it. Job-id and
//! status-line parsing is shared (issue #74 comment 5): tracking and
//! array-index mapping need the same logic.

use super::{BackendJobStatus, ScheduledRule, TerminalRecord};
use crate::cluster::{
    ClusterBackend, ClusterJobConfig, generate_array_submit_script, generate_submit_script,
};
use crate::error::{OxoFlowError, Result};
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
    }
}

fn parse_with_regex(pattern: &str, text: &str) -> Result<String> {
    let re = regex::Regex::new(pattern).expect("static regex compiles");
    re.captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| unparseable("job id", text, ""))
}

fn unparseable(what: &str, stdout: &str, stderr: &str) -> OxoFlowError {
    OxoFlowError::Config {
        message: format!("cannot parse {what} from '{stdout}' / '{stderr}'"),
    }
}

/// Parse one scheduler status line into `(job id, status)`; `None` for
/// unrecognised shapes. SLURM lines use the driver's `%i|%t` format (array
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
                "PENDING" | "CONFIGURING" => BackendJobStatus::Pending,
                "RUNNING" | "COMPLETING" => BackendJobStatus::Running,
                "COMPLETED" => BackendJobStatus::Completed,
                "FAILED" | "TIMEOUT" | "OUT_OF_MEMORY" | "NODE_FAIL" | "BOOT_FAIL" => {
                    BackendJobStatus::Failed
                }
                "CANCELLED" | "PREEMPTED" => BackendJobStatus::Cancelled,
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
                "R" | "E" => BackendJobStatus::Running,
                "C" => BackendJobStatus::Completed,
                _ => BackendJobStatus::Unknown,
            };
            Some((id, status))
        }
        ClusterBackend::Lsf => {
            // "JOBID  USER  STAT  QUEUE  ..."
            let fields: Vec<&str> = line.split_whitespace().collect();
            let id = (*fields.first()?).to_string();
            let status = match *fields.get(2)? {
                "PEND" | "PSUSP" | "USUSP" => BackendJobStatus::Pending,
                "RUN" => BackendJobStatus::Running,
                "DONE" => BackendJobStatus::Completed,
                "EXIT" | "ZOMBI" => BackendJobStatus::Failed,
                _ => BackendJobStatus::Unknown,
            };
            Some((id, status))
        }
        ClusterBackend::Sge => {
            // qstat -j output pairs "job_number: N" with "state: r"; the
            // executor collects both lines — here we extract the number only.
            Some((
                line.strip_prefix("job_number:")?.trim().to_string(),
                BackendJobStatus::Unknown,
            ))
        }
    }
}

/// The scheduler invocations that report `job_ids`, in the form each
/// scheduler actually accepts: `(program, arguments)`, one entry per call.
///
/// SLURM's `-j` takes ONE comma-separated list — ids as separate arguments
/// answer "Invalid job id specified" on a real cluster (the driver's own
/// poller always joined them; the standalone `cluster status` command did
/// not). PBS and LSF take ids positionally, SGE wants one `-j` per job.
/// SLURM is asked for the same `%i|%t` shape the poller reads, so both
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
                "%i|%t".to_string(),
            ],
        )],
        ClusterBackend::Pbs => vec![("qstat", job_ids.to_vec())],
        ClusterBackend::Lsf => vec![("bjobs", job_ids.to_vec())],
        ClusterBackend::Sge => job_ids
            .iter()
            .map(|id| ("qstat", vec!["-j".to_string(), id.clone()]))
            .collect(),
    }
}

/// The command that cancels a job, matching [`ClusterExecutor::cancel`] so
/// the standalone `cluster cancel` command and the driver agree.
pub fn cancel_command(backend: &ClusterBackend) -> &'static str {
    match backend {
        ClusterBackend::Slurm => "scancel",
        ClusterBackend::Pbs | ClusterBackend::Sge => "qdel",
        ClusterBackend::Lsf => "bkill",
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
                let status = match state {
                    "r" | "t" => BackendJobStatus::Running,
                    "qw" | "hqw" | "h" => BackendJobStatus::Pending,
                    "Eqw" => BackendJobStatus::Failed,
                    _ => BackendJobStatus::Unknown,
                };
                found.insert(id.to_string(), status);
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
    let dir = absolute_workdir(workdir).display().to_string();
    let directive = match backend {
        ClusterBackend::Slurm => format!("#SBATCH --chdir={dir}"),
        ClusterBackend::Pbs => format!("#PBS -d {dir}"),
        ClusterBackend::Sge => format!("#$ -wd {dir}"),
        ClusterBackend::Lsf => format!("#BSUB -cwd {dir}"),
    };
    let mut lines = script.lines();
    let mut finished = vec![lines.next().unwrap_or_default().to_string(), directive];
    for line in lines {
        let duplicated = (line == "set -e" || line == "mkdir -p logs")
            && finished.iter().any(|existing| existing.as_str() == line);
        if !duplicated {
            finished.push(line.to_string());
        }
    }
    // Keep whatever trailing newline the generator emitted.
    let tail = if script.ends_with('\n') { "\n" } else { "" };
    format!("{}{tail}", finished.join("\n"))
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
}

impl ClusterExecutor {
    pub fn new(backend: ClusterBackend, cluster: ClusterJobConfig) -> Self {
        Self {
            backend,
            cluster,
            bin_dir: None,
            env: Vec::new(),
            workdir: PathBuf::from("."),
        }
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

    async fn poll_pbs_lsf(
        &self,
        program: &str,
        job_ids: &[String],
    ) -> Result<HashMap<String, BackendJobStatus>> {
        let args: Vec<&str> = job_ids.iter().map(String::as_str).collect();
        let out = self.run_cmd(program, &args).await?;
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
        }
    }

    fn render_script(&self, rule: &ScheduledRule) -> Result<String> {
        Ok(finish_script(
            &generate_submit_script(&self.backend, &rule.rule, &rule.shell_cmd, &self.cluster),
            &self.backend,
            &rule.workdir,
        ))
    }

    fn render_array_script(
        &self,
        rule: &crate::rule::Rule,
        cmd_dir: &str,
        count: usize,
    ) -> Result<String> {
        Ok(finish_script(
            &generate_array_submit_script(&self.backend, rule, cmd_dir, count, &self.cluster),
            &self.backend,
            &self.workdir,
        ))
    }

    async fn submit(&self, script_path: &Path) -> Result<String> {
        let mut args: Vec<&str> = Vec::new();
        if matches!(self.backend, ClusterBackend::Slurm) {
            args.push("--parsable");
        }
        let path_str = script_path.to_string_lossy().to_string();
        args.push(&path_str);
        let out = self
            .run_cmd(crate::cluster::submit_command(&self.backend), &args)
            .await?;
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
                let out = self
                    .run_cmd("squeue", &["-j", &list, "--noheader", "-o", "%i|%t"])
                    .await?;
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
                            state = match s.trim() {
                                "r" | "t" => BackendJobStatus::Running,
                                "qw" | "hqw" => BackendJobStatus::Pending,
                                "d" => BackendJobStatus::Completed,
                                "E" => BackendJobStatus::Failed,
                                _ => BackendJobStatus::Unknown,
                            };
                        }
                    }
                    if let Some(n) = number {
                        statuses.insert(n, state);
                    }
                }
                Ok(statuses)
            }
        }
    }

    async fn cancel(&self, job_id: &str) -> Result<()> {
        let cmd = match self.backend {
            ClusterBackend::Slurm => "scancel",
            ClusterBackend::Pbs | ClusterBackend::Sge => "qdel",
            ClusterBackend::Lsf => "bkill",
        };
        self.run_cmd(cmd, &[job_id]).await.map(|_| ())
    }

    async fn logs(&self, job_id: &str) -> Result<String> {
        let (program, args) = match self.backend {
            ClusterBackend::Slurm => (
                "sacct",
                vec![
                    "-j",
                    job_id,
                    "--format=JobID,State,ExitCode,Elapsed,MaxRSS,TotalCPU",
                ],
            ),
            ClusterBackend::Pbs => ("qstat", vec!["-x", "-f", job_id]),
            ClusterBackend::Sge => ("qacct", vec!["-j", job_id]),
            ClusterBackend::Lsf => ("bacct", vec![job_id]),
        };
        let out = self.run_cmd(program, &args).await?;
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    /// Terminal record for a job that has left the live queue, read from the
    /// scheduler's accounting store (sacct / qstat -x / qacct / bacct).
    async fn terminal_status(&self, job_id: &str) -> Option<TerminalRecord> {
        let text = self.logs(job_id).await.ok()?;
        parse_accounting(&self.backend, &text)
    }

    fn polls_elements_directly(&self) -> bool {
        // squeue lists array elements as `{jobid}_{index}`; qstat/bjobs
        // report only the array base id (issue #136 H4).
        matches!(self.backend, ClusterBackend::Slurm)
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

/// Parse a scheduler duration (`[DD-]HH:MM:SS`, `MM:SS`, or bare seconds)
/// into seconds. Sub-second precision (`00:00:05.004`, common in SLURM's
/// `TotalCPU`) truncates.
fn parse_duration_secs(raw: &str) -> Option<u64> {
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
            // measurements under "resources_used.<field>".
            if !text.lines().any(|l| l.trim() == "job_state = C") {
                return None;
            }
            let field = |key: &str| {
                text.lines()
                    .map(str::trim)
                    .find_map(|l| l.strip_prefix(key)?.strip_prefix(" = "))
            };
            let exit_code = field("Exit_status").and_then(parse_exit_code);
            Some(TerminalRecord {
                status: if exit_code == Some(0) {
                    BackendJobStatus::Completed
                } else {
                    BackendJobStatus::Failed
                },
                exit_code,
                elapsed_secs: field("resources_used.walltime").and_then(parse_duration_secs),
                max_rss_mb: field("resources_used.mem").and_then(parse_rss_mb),
                cpu_seconds: field("resources_used.cput").and_then(parse_duration_secs),
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
                elapsed_secs: field("ru_wallclock").and_then(parse_duration_secs),
                max_rss_mb: field("ru_maxrss").and_then(parse_rss_mb),
                cpu_seconds: field("cpu").and_then(parse_duration_secs),
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
    let rows: Vec<Vec<&str>> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("JobID") && !l.starts_with('-'))
        .map(accounting_columns)
        .collect();
    let allocation = rows.first()?;
    let state = allocation.get(1)?.trim();
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
        .filter_map(|r| r.get(4).copied().and_then(parse_rss_mb))
        .max();
    // TotalCPU is likewise per-step; the allocation row leaves it blank.
    let cpu_seconds = rows
        .iter()
        .filter_map(|r| r.get(5).copied().and_then(parse_duration_secs))
        .max();
    Some(TerminalRecord {
        status,
        exit_code: allocation.get(2).copied().and_then(parse_exit_code),
        elapsed_secs: allocation.get(3).copied().and_then(parse_duration_secs),
        max_rss_mb,
        cpu_seconds,
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
            &generate_submit_script(
                &ClusterBackend::Slurm,
                &plan.rules["align"].rule,
                &plan.rules["align"].shell_cmd,
                &cluster_config(),
            ),
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
                    "%i|%t".to_string(),
                ]
            )]
        );
        assert_eq!(
            status_invocations(&ClusterBackend::Pbs, &ids),
            vec![("qstat", ids.to_vec())]
        );
        assert_eq!(
            status_invocations(&ClusterBackend::Lsf, &ids),
            vec![("bjobs", ids.to_vec())]
        );
        // SGE answers one job per -j.
        assert_eq!(
            status_invocations(&ClusterBackend::Sge, &ids),
            vec![
                ("qstat", vec!["-j".to_string(), "101".to_string()]),
                ("qstat", vec!["-j".to_string(), "202".to_string()]),
            ]
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
            (ClusterBackend::Slurm, "#SBATCH --chdir=/wf"),
            (ClusterBackend::Pbs, "#PBS -d /wf"),
            (ClusterBackend::Sge, "#$ -wd /wf"),
            (ClusterBackend::Lsf, "#BSUB -cwd /wf"),
        ];
        for (backend, directive) in cases {
            let script = finish_script("#!/bin/bash\nset -e\ntrue\n", &backend, Path::new("/wf"));
            assert_eq!(
                script,
                format!("#!/bin/bash\n{directive}\nset -e\ntrue\n"),
                "{backend}: the working directory must be pinned"
            );
        }
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
            "#!/bin/bash\n#SBATCH --chdir={}\ntrue\n",
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
            .render_array_script(&rule, "/run/jobs/align/chunk-1", 2)
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
            script.contains("#SBATCH --chdir=/wf\n"),
            "the array pins the working directory too: {script}"
        );
        assert!(
            script.starts_with("#!/bin/bash\n#SBATCH --chdir=/wf\n#SBATCH --array=1-2\n"),
            "the array range stays right after the new chdir directive: {script}"
        );
    }
}

#[cfg(test)]
mod accounting_tests {
    use super::*;

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
        assert_eq!(parse_duration_secs("00:00:05"), Some(5));
        assert_eq!(parse_duration_secs("01:02:03"), Some(3723));
        assert_eq!(parse_duration_secs("2-00:00:00"), Some(172_800));
        assert_eq!(parse_duration_secs("04:30"), Some(270));
        assert_eq!(parse_duration_secs("82"), Some(82));
        // SLURM TotalCPU carries sub-second precision; it truncates.
        assert_eq!(parse_duration_secs("00:00:04.500"), Some(4));
        assert_eq!(parse_duration_secs(""), None);
        assert_eq!(parse_duration_secs("UNLIMITED"), None);

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
}
