//! Cluster (SLURM/PBS/SGE/LSF) executor + shared scheduler-output parsing.
//!
//! Directive generation lives in `crate::cluster` and stays as-is (issue #74);
//! this module is the submission/tracking layer above it. Job-id and
//! status-line parsing is shared (issue #74 comment 5): tracking and
//! array-index mapping need the same logic.

use super::{BackendJobStatus, ScheduledRule};
use crate::cluster::{ClusterBackend, ClusterJobConfig, generate_submit_script};
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
}

impl ClusterExecutor {
    pub fn new(backend: ClusterBackend, cluster: ClusterJobConfig) -> Self {
        Self {
            backend,
            cluster,
            bin_dir: None,
            env: Vec::new(),
        }
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
        Ok(generate_submit_script(
            &self.backend,
            &rule.rule,
            &rule.shell_cmd,
            &self.cluster,
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
                                .unwrap_or(BackendJobStatus::Unknown);
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
                vec!["-j", job_id, "--format=JobID,State,ExitCode,Elapsed,MaxRSS"],
            ),
            ClusterBackend::Pbs => ("qstat", vec!["-x", "-f", job_id]),
            ClusterBackend::Sge => ("qacct", vec!["-j", job_id]),
            ClusterBackend::Lsf => ("bacct", vec![job_id]),
        };
        let out = self.run_cmd(program, &args).await?;
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    /// Terminal status of a job that has left the live queue, read from the
    /// scheduler's accounting store (sacct / qstat -x / qacct / bacct).
    async fn terminal_status(&self, job_id: &str) -> Option<BackendJobStatus> {
        let text = self.logs(job_id).await.ok()?;
        parse_accounting(&self.backend, &text)
    }
}

/// Parse scheduler accounting output into a terminal status, or `None` when
/// the record is absent or the job is not terminal.
fn parse_accounting(backend: &ClusterBackend, text: &str) -> Option<BackendJobStatus> {
    match backend {
        ClusterBackend::Slurm => {
            // sacct rows: "<JobID> <Elapsed> <State> <ExitCode> <MaxRSS>"
            // (header and separator rows carry no job data). The mock
            // scheduler emits the same columns pipe-separated — accept both.
            let line = text.lines().find(|l| {
                let t = l.trim();
                !t.is_empty() && !t.starts_with("JobID") && !t.starts_with('-')
            });
            let state = line.and_then(|l| {
                let cols: Vec<&str> = if l.contains('|') {
                    l.split('|').map(str::trim).collect()
                } else {
                    l.split_whitespace().collect()
                };
                cols.get(1).copied().map(str::to_string)
            });
            if let Some(state) = state {
                match state.split('.').next().unwrap_or(&state) {
                    "COMPLETED" => return Some(BackendJobStatus::Completed),
                    "FAILED" | "TIMEOUT" | "OUT_OF_MEMORY" => {
                        return Some(BackendJobStatus::Failed);
                    }
                    "CANCELLED" => return Some(BackendJobStatus::Cancelled),
                    _ => return None,
                }
            }
            None
        }
        ClusterBackend::Pbs => {
            // qstat -x -f: "    job_state = C" + "    Exit_status = N"
            if !text.lines().any(|l| l.trim() == "job_state = C") {
                return None;
            }
            if text.lines().any(|l| l.trim() == "Exit_status = 0") {
                Some(BackendJobStatus::Completed)
            } else {
                Some(BackendJobStatus::Failed)
            }
        }
        ClusterBackend::Sge => {
            // qacct: "exit_status 0" (success) or a non-zero value (failed).
            for line in text.lines().map(str::trim) {
                if let Some(rest) = line.strip_prefix("exit_status") {
                    let v: i64 = rest.trim().parse().unwrap_or(1);
                    return Some(if v == 0 {
                        BackendJobStatus::Completed
                    } else {
                        BackendJobStatus::Failed
                    });
                }
            }
            None
        }
        ClusterBackend::Lsf => {
            // bacct: "Job <id>, ..., Status <DONE|EXIT|RUN>, ..."
            if text.contains("Status <DONE>") {
                Some(BackendJobStatus::Completed)
            } else if text.contains("Status <EXIT>") {
                Some(BackendJobStatus::Failed)
            } else {
                None
            }
        }
    }
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
        let direct = generate_submit_script(
            &ClusterBackend::Slurm,
            &plan.rules["align"].rule,
            &plan.rules["align"].shell_cmd,
            &cluster_config(),
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
}

#[cfg(test)]
mod accounting_tests {
    use super::*;

    #[test]
    fn parses_sacct_terminal_lines() {
        // Arrange — real sacct --format=JobID,State,ExitCode,Elapsed,MaxRSS
        // (State is the second column in that format order).
        let text = "JobID    State      ExitCode     Elapsed    MaxRSS\n\
------------ ---------- -------- ---------- ----------\n\
12345    COMPLETED      0:0        00:01:22    128.2M\n";

        // Act / Assert
        assert_eq!(
            parse_accounting(&ClusterBackend::Slurm, text),
            Some(BackendJobStatus::Completed)
        );
        let failed = text.replace("COMPLETED", "FAILED");
        assert_eq!(
            parse_accounting(&ClusterBackend::Slurm, &failed),
            Some(BackendJobStatus::Failed)
        );
        // The mock scheduler pipes the same columns.
        let piped = "JobID|State|ExitCode|Elapsed|MaxRSS\n12345|COMPLETED|0:0|00:00:05|1234K\n";
        assert_eq!(
            parse_accounting(&ClusterBackend::Slurm, piped),
            Some(BackendJobStatus::Completed)
        );
    }

    #[test]
    fn parses_pbs_qstat_x_terminal_blocks() {
        // Arrange — qstat -x -f job block
        let ok =
            "Job Id: 12345.vm\n    Job_Name = fastqc\n    job_state = C\n    Exit_status = 0\n";
        assert_eq!(
            parse_accounting(&ClusterBackend::Pbs, ok),
            Some(BackendJobStatus::Completed)
        );
        let bad = ok.replace("Exit_status = 0", "Exit_status = 1");
        assert_eq!(
            parse_accounting(&ClusterBackend::Pbs, &bad),
            Some(BackendJobStatus::Failed)
        );
        // Still running: no terminal state.
        let running = ok.replace("job_state = C", "job_state = R");
        assert_eq!(parse_accounting(&ClusterBackend::Pbs, &running), None);
    }

    #[test]
    fn parses_qacct_exit_status() {
        // Arrange — qacct -j output
        let ok = "qname        all.q\njobnumber    12345\nexit_status  0\nru_wallclock 82\n";
        assert_eq!(
            parse_accounting(&ClusterBackend::Sge, ok),
            Some(BackendJobStatus::Completed)
        );
        let bad = ok.replace("exit_status  0", "exit_status  137");
        assert_eq!(
            parse_accounting(&ClusterBackend::Sge, &bad),
            Some(BackendJobStatus::Failed)
        );
    }

    #[test]
    fn parses_bacct_status_lines() {
        // Arrange — bacct output carries a Status <...> line
        let done = "Job <12345>, User <bioinf>, Status <DONE>, Queue <normal>, Command <fastqc>\n";
        assert_eq!(
            parse_accounting(&ClusterBackend::Lsf, done),
            Some(BackendJobStatus::Completed)
        );
        let exited = done.replace("DONE", "EXIT");
        assert_eq!(
            parse_accounting(&ClusterBackend::Lsf, &exited),
            Some(BackendJobStatus::Failed)
        );
        let running = done.replace("Status <DONE>", "Status <RUN>");
        assert_eq!(parse_accounting(&ClusterBackend::Lsf, &running), None);
    }
}
