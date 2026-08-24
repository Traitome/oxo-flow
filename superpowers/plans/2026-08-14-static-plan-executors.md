# Static Plan + Pluggable Executors (issue #78) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land issue #78's three phases — P1 ExecutorBackend trait + ClusterExecutor + BackendDriver with mock-scheduler CI harness, P2 unified storage invalidation (etag-aware manifests), P3 checkpoint re-entry (static+dynamic hybrid DAG) — each phase independently committed with `make ci` green.

**Architecture:** The engine's static parts (wildcard expansion, DAG, invalidation, checkpoint) stay single-implementation; a new `core::backend` module maps the static plan onto scheduler APIs. `core/cluster.rs` directive generation stays untouched (per #74). `run.rs` gets zero P1/P2 logic changes (two one-line resolver passes in P2) and a bounded additive re-entry hook in P3.

**Tech Stack:** Rust edition 2024, tokio, async-trait, serde, petgraph DAG; shell-script mock scheduler (bash) for CI without a cluster.

**Spec:** [docs/superpowers/specs/2026-08-14-static-plan-executors-design.md](../specs/2026-08-14-static-plan-executors-design.md)

**Implementation notes (refinements of spec, same behaviour):**
- `ScheduledRule` carries the expanded `Rule` clone (single source for rendering) instead of duplicated resource fields — spec §3.1 field list superseded.
- `cd <workdir>` is composed into the wrapped shell command (`cd '<workdir>' && <wrapped>`) rather than injected into the rendered script — keeps `render_script` byte-compatible with `cluster.rs` output.
- `ExecutorBackend::submit` is per-rule (`submit(script_path)`) instead of per-fragment; the driver composes fragments (waves). Same contract, simpler adapters.
- Cancel-on-drop is implemented as cancel-on-error-path + `cancel_inflight()`; SIGINT-driven cancel is CLI wiring (→ #74 phase 2), documented.

## Global Constraints

- `forbid(unsafe_code)` in core; no new production dependencies (async-trait, regex, tokio already in tree); `regex` is already a core dep (used by wildcard.rs).
- `make ci` (fmt + clippy -D warnings + build + test + audit) must pass after EVERY commit.
- Existing 1527 tests must stay green throughout.
- Files <800 lines, functions <50 lines, early returns, no silent error swallowing, immutability-first (no in-place mutation of shared state without clone).
- `core/cluster.rs` directive generation: additive changes only (no behaviour change).
- Conventional commits (`feat:`/`test:`/`docs:`), no attribution footer, commit to `main` directly (repo convention), push only at the end per phase.
- tracing to stderr; stdout reserved for machine output.

---

# PART 1 — ExecutorBackend, ClusterExecutor, BackendDriver, mock scheduler

## Task 1: `ScheduledRule` + `ScheduledPlan` types and builder

**Files:**
- Create: `crates/oxo-flow-core/src/backend/mod.rs`
- Modify: `crates/oxo-flow-core/src/lib.rs` (add `pub mod backend;`)
- Test: inline `#[cfg(test)] mod tests` in `backend/mod.rs`

**Interfaces:**
- Consumes: `WorkflowDag::execution_order`, `WorkflowDag::parallel_groups`, `WorkflowDag::dependencies`, `WorkflowConfig::get_rule`, `build_execution_command`, `EnvironmentResolver::wrap_command`, `Rule`.
- Produces (used by Tasks 2–5, 15):
  - `backend::ScheduledRule { rule: Rule, shell_cmd: String, workdir: PathBuf, dependencies: Vec<String>, wildcard_values: HashMap<String, String> }`
  - `backend::ScheduledPlan { order: Vec<String>, groups: Vec<Vec<String>>, rules: HashMap<String, ScheduledRule> }`
  - `ScheduledPlan::build(config: &WorkflowConfig, dag: &WorkflowDag, workdir: &Path, env_resolver: &EnvironmentResolver, wildcard_values: &HashMap<String, String>) -> Result<ScheduledPlan>`
  - `ScheduledPlan::merge_new_instances(&mut self, config, dag, workdir, env_resolver, wildcard_values, new_names: &[String]) -> Result<()>`

- [ ] **Step 1: Write the failing tests** (inline in `backend/mod.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkflowConfig;
    use crate::dag::WorkflowDag;

    fn demo_config() -> WorkflowConfig {
        // two-rule chain: preprocess (no wildcards) -> analyze_{group}_{sample}
        let toml = r#"
            [config]
            ref = "/data/ref.fa"
            [[sample_groups]]
            name = "batch"
            samples = ["S1", "S2"]
            [[rules]]
            name = "preprocess"
            shell = "cp {config.ref} ref.bak"
            output = ["ref.bak"]
            [[rules]]
            name = "analyze"
            input = ["ref.bak"]
            output = ["out/{sample}.txt"]
            shell = "touch out/{sample}.txt"
        "#;
        WorkflowConfig::from_str(toml).unwrap()   // or from_file on a temp file
    }

    #[test]
    fn build_produces_order_groups_and_deps() {
        let mut config = demo_config();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let values = std::collections::HashMap::from([
            ("config.ref".to_string(), "/data/ref.fa".to_string()),
        ]);
        let plan = ScheduledPlan::build(&config, &dag, std::path::Path::new("/tmp/wf"),
                                        &EnvironmentResolver::new(), &values).unwrap();
        assert_eq!(plan.order, dag.execution_order().unwrap());
        assert_eq!(plan.groups, dag.parallel_groups().unwrap());
        // analyze instances depend on preprocess
        for name in ["analyze_batch_S1", "analyze_batch_S2"] {
            let r = &plan.rules[name];
            assert_eq!(r.dependencies, vec!["preprocess".to_string()]);
            assert!(r.shell_cmd.contains("cd '/tmp/wf'"));
            assert!(r.shell_cmd.contains("touch out/S1.txt") || r.shell_cmd.contains("touch out/S2.txt"));
        }
    }

    #[test]
    fn build_skips_rules_without_shell() {
        // rule with neither shell nor script is not schedulable and is omitted
    }
}
```

(Adjust `WorkflowConfig::from_str`/`from_file` to the actual constructor used in the repo — check `config.rs` parse entry points before writing; the repo's existing tests use `WorkflowConfig::from_file` with temp files.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p oxo-flow-core backend::tests -- --nocapture`
Expected: FAIL — `use of undeclared crate or module 'backend'`.

- [ ] **Step 3: Write minimal implementation**

```rust
//! Executor-agnostic static plan (issue #78): one plan, many executors.

use crate::config::WorkflowConfig;
use crate::dag::WorkflowDag;
use crate::environment::EnvironmentResolver;
use crate::error::{OxoFlowError, Result};
use crate::executor::process::build_execution_command;
use crate::rule::Rule;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub mod cluster;
pub mod driver;

/// One executable unit of the static plan: a fully resolved rule instance.
#[derive(Debug, Clone)]
pub struct ScheduledRule {
    pub rule: Rule,
    /// Environment-wrapped command with `cd <workdir>` folded in.
    pub shell_cmd: String,
    pub workdir: PathBuf,
    /// Instance-level dependencies (DAG edges + resolved `depends_on`).
    pub dependencies: Vec<String>,
    /// `{config.x}`-style bindings for late path expansion.
    pub wildcard_values: HashMap<String, String>,
}

/// The executor-agnostic static plan.
#[derive(Debug, Clone)]
pub struct ScheduledPlan {
    pub order: Vec<String>,
    pub groups: Vec<Vec<String>>,
    pub rules: HashMap<String, ScheduledRule>,
}

impl ScheduledPlan {
    pub fn build(
        config: &WorkflowConfig,
        dag: &WorkflowDag,
        workdir: &Path,
        env_resolver: &EnvironmentResolver,
        wildcard_values: &HashMap<String, String>,
    ) -> Result<Self> {
        let order = dag.execution_order()?;
        let groups = dag.parallel_groups()?;
        let mut rules = HashMap::new();
        for name in &order {
            if let Some(sr) = build_rule(name, config, dag, workdir, env_resolver, wildcard_values)? {
                rules.insert(name.clone(), sr);
            }
        }
        Ok(Self { order, groups, rules })
    }

    /// Append newly created instances after a checkpoint re-entry (P3).
    pub fn merge_new_instances(
        &mut self,
        config: &WorkflowConfig,
        dag: &WorkflowDag,
        workdir: &Path,
        env_resolver: &EnvironmentResolver,
        wildcard_values: &HashMap<String, String>,
        new_names: &[String],
    ) -> Result<()> {
        for name in new_names {
            if let Some(sr) = build_rule(name, config, dag, workdir, env_resolver, wildcard_values)? {
                self.rules.insert(name.clone(), sr);
                self.order.push(name.clone());
            }
        }
        self.groups = dag.parallel_groups()?;
        Ok(())
    }
}

fn build_rule(
    name: &str,
    config: &WorkflowConfig,
    dag: &WorkflowDag,
    workdir: &Path,
    env_resolver: &EnvironmentResolver,
    wildcard_values: &HashMap<String, String>,
) -> Result<Option<ScheduledRule>> {
    let rule = config.get_rule(name).ok_or_else(|| OxoFlowError::Config {
        message: format!("rule '{name}' not found in workflow"),
    })?;
    let Some(cmd) = build_execution_command(rule, wildcard_values, &config.workflow.interpreter_map) else {
        return Ok(None); // no shell/script — not schedulable
    };
    let wrapped = env_resolver
        .wrap_command(&cmd, &rule.environment, Some(&rule.resources), workdir)
        .map_err(|e| OxoFlowError::Config {
            message: format!("environment wrapping failed for '{name}': {e}"),
        })?;
    let dependencies = dag.dependencies(name).unwrap_or_default();
    Ok(Some(ScheduledRule {
        rule: rule.clone(),
        shell_cmd: format!("cd '{}' && {}", workdir.display(), wrapped),
        workdir: workdir.to_path_buf(),
        dependencies,
        wildcard_values: wildcard_values.clone(),
    }))
}
```

- [ ] **Step 4: Run tests to verify they pass** — same command, expected PASS.
- [ ] **Step 5: Commit** — `git add crates/oxo-flow-core/src/backend/mod.rs crates/oxo-flow-core/src/lib.rs && git commit -m "feat(core): ScheduledPlan + ScheduledRule — executor-agnostic static plan (#78)"`

## Task 2: `ExecutorBackend` trait, `BackendJobStatus`, shared job-id parsing

**Files:**
- Modify: `crates/oxo-flow-core/src/backend/mod.rs` (trait + status + `parse_job_id` + `parse_status_line`)
- Test: inline in `backend/mod.rs`

**Interfaces:**
- Produces (used by Tasks 3–6, 15):
  - `pub enum BackendJobStatus { Pending, Running, Completed, Failed, Cancelled, Unknown }` (Copy, Clone, Debug, PartialEq, Eq)
  - `pub trait ExecutorBackend: Send + Sync { fn name(&self) -> &'static str; fn render_script(&self, rule: &ScheduledRule, cluster: &ClusterJobConfig) -> Result<String>; async fn submit(&self, script_path: &Path) -> Result<String>; async fn poll(&self, job_ids: &[String]) -> Result<HashMap<String, BackendJobStatus>>; async fn cancel(&self, job_id: &str) -> Result<()>; async fn logs(&self, job_id: &str) -> Result<String>; }` (async_trait)
  - `pub fn parse_job_id(backend: &ClusterBackend, stdout: &str, stderr: &str) -> Result<String>`
  - `pub fn parse_status_line(backend: &ClusterBackend, line: &str) -> Option<(String, BackendJobStatus)>`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn parse_job_id_slurm_parsable() {
    assert_eq!(parse_job_id(&ClusterBackend::Slurm, "12345\n", "").unwrap(), "12345");
}
#[test]
fn parse_job_id_slurm_sentence_fallback() {
    assert_eq!(parse_job_id(&ClusterBackend::Slurm, "", "Submitted batch job 67890\n").unwrap(), "67890");
}
#[test]
fn parse_job_id_pbs_bare() {
    assert_eq!(parse_job_id(&ClusterBackend::Pbs, "777.queue\n", "").unwrap(), "777.queue");
}
#[test]
fn parse_job_id_sge_sentence() {
    assert_eq!(parse_job_id(&ClusterBackend::Sge, "Your job 4242 (\"align.sh\") has been submitted\n", "").unwrap(), "4242");
}
#[test]
fn parse_job_id_lsf_sentence() {
    assert_eq!(parse_job_id(&ClusterBackend::Lsf, "Job <9999> is submitted to queue <normal>.\n", "").unwrap(), "9999");
}
#[test]
fn parse_job_id_slurm_unparseable_is_error() {
    assert!(parse_job_id(&ClusterBackend::Slurm, "garbage", "also garbage").is_err());
}

#[test]
fn parse_status_line_slurm() {
    assert_eq!(parse_status_line(&ClusterBackend::Slurm, "12345|RUNNING"), Some(("12345".into(), BackendJobStatus::Running)));
    assert_eq!(parse_status_line(&ClusterBackend::Slurm, "12345_7|COMPLETED"), Some(("12345_7".into(), BackendJobStatus::Completed)));
    assert_eq!(parse_status_line(&ClusterBackend::Slurm, "12345|OUT_OF_MEMORY"), Some(("12345".into(), BackendJobStatus::Failed)));
    assert_eq!(parse_status_line(&ClusterBackend::Slurm, "12345|CANCELLED"), Some(("12345".into(), BackendJobStatus::Cancelled)));
    assert_eq!(parse_status_line(&ClusterBackend::Slurm, "12345|WEIRD"), Some(("12345".into(), BackendJobStatus::Unknown)));
    assert_eq!(parse_status_line(&ClusterBackend::Slurm, "no-pipe-here"), None);
}
#[test]
fn parse_status_line_pbs() { /* "777.queue  user  queue  jobname  SessID  NDS  TSK  mem  time  S  time" → 10th field S: R→Running, Q→Pending, C→Completed, E→Failed */ }
#[test]
fn parse_status_line_lsf() { /* "JOBID USER STAT QUEUE ..." → RUN→Running, PEND→Pending, DONE→Completed, EXIT→Failed */ }
```

- [ ] **Step 2: Run to verify FAIL** (compile error: no `parse_job_id`).
- [ ] **Step 3: Implement**

```rust
use crate::cluster::{ClusterBackend, ClusterJobConfig};
use crate::error::{OxoFlowError, Result};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendJobStatus { Pending, Running, Completed, Failed, Cancelled, Unknown }

#[async_trait::async_trait]
pub trait ExecutorBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn render_script(&self, rule: &ScheduledRule, cluster: &ClusterJobConfig) -> Result<String>;
    async fn submit(&self, script_path: &Path) -> Result<String>;
    async fn poll(&self, job_ids: &[String]) -> Result<HashMap<String, BackendJobStatus>>;
    async fn cancel(&self, job_id: &str) -> Result<()>;
    async fn logs(&self, job_id: &str) -> Result<String>;
}

/// Parse the scheduler-assigned job id from a submission's output.
/// Shared helper (issue #74 comment 5): tracking and array-index mapping
/// need the same parsing.
pub fn parse_job_id(backend: &ClusterBackend, stdout: &str, stderr: &str) -> Result<String> {
    let out = stdout.trim();
    match backend {
        ClusterBackend::Slurm => {
            if !out.is_empty() && out.chars().all(|c| c.is_ascii_digit()) {
                return Ok(out.to_string()); // --parsable
            }
            let re = regex::Regex::new(r"Submitted batch job (\d+)").unwrap();
            re.captures(stderr).and_then(|c| c.get(1)).map(|m| m.as_str().to_string())
                .ok_or_else(|| OxoFlowError::Config { message: format!("cannot parse SLURM job id from '{}' / '{}'", out, stderr.trim()) })
        }
        ClusterBackend::Pbs => {
            if !out.is_empty() && !out.contains(' ') { return Ok(out.to_string()); }
            Err(OxoFlowError::Config { message: format!("cannot parse PBS job id from '{out}'") })
        }
        ClusterBackend::Sge => parse_with_regex(r"Your job(?:-array)? (\d+)", stdout),
        ClusterBackend::Lsf => parse_with_regex(r"Job <(\d+)> is submitted", stdout),
    }
}

fn parse_with_regex(pattern: &str, text: &str) -> Result<String> {
    let re = regex::Regex::new(pattern).unwrap();
    re.captures(text).and_then(|c| c.get(1)).map(|m| m.as_str().to_string())
        .ok_or_else(|| OxoFlowError::Config { message: format!("cannot parse job id from '{text}'") })
}

/// Parse one status line into (job id, status); `None` for unrecognised shapes.
pub fn parse_status_line(backend: &ClusterBackend, line: &str) -> Option<(String, BackendJobStatus)> {
    match backend {
        ClusterBackend::Slurm => {
            let (id, state) = line.split_once('|')?;
            Some((id.to_string(), match state {
                "PENDING" | "CONFIGURING" => BackendJobStatus::Pending,
                "RUNNING" | "COMPLETING" => BackendJobStatus::Running,
                "COMPLETED" => BackendJobStatus::Completed,
                "FAILED" | "TIMEOUT" | "OUT_OF_MEMORY" | "NODE_FAIL" | "BOOT_FAIL" => BackendJobStatus::Failed,
                "CANCELLED" | "PREEMPTED" => BackendJobStatus::Cancelled,
                _ => BackendJobStatus::Unknown,
            }))
        }
        ClusterBackend::Pbs => {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let id = (*fields.first()?).to_string();
            let state = *fields.get(9)?; // S column
            Some((id, match state { "Q" | "H" => BackendJobStatus::Pending, "R" | "E" => BackendJobStatus::Running, "C" => BackendJobStatus::Completed, _ => BackendJobStatus::Unknown }))
        }
        ClusterBackend::Lsf => {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let id = (*fields.first()?).to_string();
            let state = *fields.get(2)?; // STAT column
            Some((id, match state { "PEND" | "PSUSP" => BackendJobStatus::Pending, "RUN" => BackendJobStatus::Running, "DONE" => BackendJobStatus::Completed, "EXIT" | "ZOMBI" => BackendJobStatus::Failed, _ => BackendJobStatus::Unknown }))
        }
        ClusterBackend::Sge => {
            // qstat -j output: "job_number: 4242" / "state: r"
            let number = line.strip_prefix("job_number:")?.trim();
            Some((number.to_string(), BackendJobStatus::Unknown)) // state resolved from the paired "state:" line
        }
    }
}
```

Note: SGE state resolution needs the paired `state:` line; `ClusterExecutor::poll` for SGE collects pairs. The unit test above covers the number extraction; the executor test covers pairing.

- [ ] **Step 4: Run to verify PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(core): ExecutorBackend trait + shared job-id/status parsing (#78)"`

## Task 3: `ClusterExecutor` (render/submit/poll/cancel/logs)

**Files:**
- Create: `crates/oxo-flow-core/src/backend/cluster.rs`
- Test: inline in `backend/cluster.rs`

**Interfaces:**
- Consumes: Task 1 (`ScheduledRule`), Task 2 (trait, `parse_job_id`, `parse_status_line`), `crate::cluster::{generate_submit_script, submit_command}`.
- Produces: `pub struct ClusterExecutor { backend: ClusterBackend }` with `pub fn new(backend: ClusterBackend) -> Self`; `impl ExecutorBackend for ClusterExecutor`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn render_parity_with_cluster_rs() {
    // Byte-identical to the existing directive generator for the same inputs.
    let mut config = demo_config(); // as in Task 1, single non-wildcard rule
    config.apply_defaults();
    config.expand_wildcards().unwrap();
    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    let plan = ScheduledPlan::build(&config, &dag, Path::new("."), &EnvironmentResolver::new(), &HashMap::new()).unwrap();
    let exec = ClusterExecutor::new(ClusterBackend::Slurm);
    let via_trait = exec.render_script(&plan.rules["preprocess"], &cluster_config()).unwrap();
    let direct = generate_submit_script(&ClusterBackend::Slurm, &plan.rules["preprocess"].rule,
                                         &plan.rules["preprocess"].shell_cmd, &cluster_config());
    assert_eq!(via_trait, direct);
}

fn cluster_config() -> ClusterJobConfig {
    ClusterJobConfig { backend: ClusterBackend::Slurm, queue: Some("compute".into()), account: None, walltime: None, extra_args: vec![] }
}
```

- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement** (submit/poll/cancel/logs via `tokio::process::Command` with a 30 s timeout each):

```rust
//! Cluster (SLURM/PBS/SGE/LSF) implementation of [`ExecutorBackend`].
//!
//! Directive generation lives in `crate::cluster` and stays as-is (issue #74);
//! this module is the submission/tracking layer above it.

use super::{parse_job_id, parse_status_line, BackendJobStatus, ExecutorBackend, ScheduledRule};
use crate::cluster::{generate_submit_script, submit_command, ClusterBackend, ClusterJobConfig};
use crate::error::{OxoFlowError, Result};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

const SCHEDULER_CALL_TIMEOUT: Duration = Duration::from_secs(30);

pub struct ClusterExecutor { backend: ClusterBackend }

impl ClusterExecutor {
    pub fn new(backend: ClusterBackend) -> Self { Self { backend } }
    async fn run_cmd(program: &str, args: &[&str]) -> Result<std::process::Output> {
        let out = tokio::process::Command::new(program).args(args)
            .kill_on_drop(true)
            .output().await
            .map_err(|e| OxoFlowError::Config { message: format!("failed to run '{program}': {e}") })?;
        if !out.status.success() {
            return Err(OxoFlowError::Config {
                message: format!("'{program}' exited {}: {}", out.status.code().unwrap_or(-1),
                                 String::from_utf8_lossy(&out.stderr).trim()),
            });
        }
        Ok(out)
    }
}

#[async_trait::async_trait]
impl ExecutorBackend for ClusterExecutor {
    fn name(&self) -> &'static str { match self.backend { ClusterBackend::Slurm => "slurm", ClusterBackend::Pbs => "pbs", ClusterBackend::Sge => "sge", ClusterBackend::Lsf => "lsf" } }

    fn render_script(&self, rule: &ScheduledRule, cluster: &ClusterJobConfig) -> Result<String> {
        Ok(generate_submit_script(&self.backend, &rule.rule, &rule.shell_cmd, cluster))
    }

    async fn submit(&self, script_path: &Path) -> Result<String> {
        let mut args: Vec<&str> = Vec::new();
        if matches!(self.backend, ClusterBackend::Slurm) { args.push("--parsable"); }
        let path_str = script_path.to_string_lossy().to_string();
        args.push(&path_str);
        let out = Self::run_cmd(submit_command(&self.backend), &args).await?;
        parse_job_id(&self.backend, &String::from_utf8_lossy(&out.stdout), &String::from_utf8_lossy(&out.stderr))
    }

    async fn poll(&self, job_ids: &[String]) -> Result<HashMap<String, BackendJobStatus>> {
        let mut statuses = HashMap::new();
        match self.backend {
            ClusterBackend::Slurm => {
                let list = job_ids.join(",");
                let out = Self::run_cmd("squeue", &["-j", &list, "--noheader", "-o", "%i|%t"]).await?;
                for line in String::from_utf8_lossy(&out.stdout).lines().map(str::trim).filter(|l| !l.is_empty()) {
                    if let Some((id, st)) = parse_status_line(&self.backend, line) {
                        statuses.insert(id, st);
                    }
                }
            }
            ClusterBackend::Pbs => {
                let out = Self::run_cmd("qstat", &job_ids.iter().map(String::as_str).collect::<Vec<_>>().as_slice()).await?;
                for line in String::from_utf8_lossy(&out.stdout).lines().map(str::trim).filter(|l| !l.is_empty()) {
                    if let Some((id, st)) = parse_status_line(&self.backend, line) { statuses.insert(id, st); }
                }
            }
            ClusterBackend::Lsf => {
                let out = Self::run_cmd("bjobs", &job_ids.iter().map(String::as_str).collect::<Vec<_>>().as_slice()).await?;
                for line in String::from_utf8_lossy(&out.stdout).lines().map(str::trim).filter(|l| !l.is_empty()) {
                    if let Some((id, st)) = parse_status_line(&self.backend, line) { statuses.insert(id, st); }
                }
            }
            ClusterBackend::Sge => {
                for id in job_ids {
                    let out = Self::run_cmd("qstat", &["-j", id]).await?;
                    let mut number = None; let mut state = BackendJobStatus::Unknown;
                    for line in String::from_utf8_lossy(&out.stdout).lines() {
                        if let Some((n, _)) = parse_status_line(&self.backend, line.trim()) { number = Some(n); }
                        if let Some(s) = line.trim().strip_prefix("state:") {
                            state = match s.trim() { "r" | "t" => BackendJobStatus::Running, "qw" | "hqw" => BackendJobStatus::Pending, "d" => BackendJobStatus::Completed, "E" => BackendJobStatus::Failed, _ => BackendJobStatus::Unknown };
                        }
                    }
                    if let Some(n) = number { statuses.insert(n, state); }
                }
            }
        }
        Ok(statuses)
    }

    async fn cancel(&self, job_id: &str) -> Result<()> {
        let cmd = match self.backend { ClusterBackend::Slurm => "scancel", ClusterBackend::Pbs | ClusterBackend::Sge => "qdel", ClusterBackend::Lsf => "bkill" };
        Self::run_cmd(cmd, &[job_id]).await.map(|_| ())
    }

    async fn logs(&self, job_id: &str) -> Result<String> {
        let (program, args) = match self.backend {
            ClusterBackend::Slurm => ("sacct", vec!["-j", job_id, "--format=JobID,State,ExitCode,Elapsed,MaxRSS"]),
            ClusterBackend::Pbs => ("qstat", vec!["-x", "-f", job_id]),
            ClusterBackend::Sge => ("qacct", vec!["-j", job_id]),
            ClusterBackend::Lsf => ("bacct", vec![job_id]),
        };
        let out = Self::run_cmd(program, &args).await?;
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }
}
```

(Watch clippy: `collect::<Vec<_>>().as_slice()` temporaries — bind to locals first.)

- [ ] **Step 4: Run to verify PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(core): ClusterExecutor — submit/poll/cancel/logs over the directive generator (#78)"`

## Task 4: Mock scheduler fixtures

**Files:**
- Create: `tests/fixtures/mock-scheduler/sbatch`, `tests/fixtures/mock-scheduler/squeue`, `tests/fixtures/mock-scheduler/scancel`, `tests/fixtures/mock-scheduler/sacct` (all `chmod +x`)
- Test: exercised by Tasks 6 and 15; smoke-test here by hand (see Step 1)

**Interfaces:**
- Environment: `MOCK_SCHEDULER_DIR` (state root; `<dir>/jobs/<id>/{state,exit_code,stdout.log,stderr.log,job.sh,pid}`).
- Produces: a fake SLURM on `PATH` for integration tests (issue #74 comment 3; reusable by @andrewbudge).

- [ ] **Step 1: Write the fixtures**

`sbatch`:

```bash
#!/bin/bash
# Mock SLURM sbatch — executes the submitted script in the background and
# records state under $MOCK_SCHEDULER_DIR/jobs/<id>.
set -eu
dir="${MOCK_SCHEDULER_DIR:?MOCK_SCHEDULER_DIR not set}"
mkdir -p "$dir/jobs"
parsable=0
script=""
while [ $# -gt 0 ]; do
  case "$1" in
    --parsable) parsable=1; shift;;
    *) script="$1"; shift;;
  esac
done
id=$(find "$dir/jobs" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l | tr -d ' ')
id=$((id + 1))
mkdir -p "$dir/jobs/$id"
echo "PENDING" > "$dir/jobs/$id/state"
cp "$script" "$dir/jobs/$id/job.sh"
(
  echo "RUNNING" > "$dir/jobs/$id/state"
  echo $$ > "$dir/jobs/$id/pid"
  bash "$script" > "$dir/jobs/$id/stdout.log" 2> "$dir/jobs/$id/stderr.log"
  code=$?
  if [ "$code" -eq 0 ]; then echo "COMPLETED" > "$dir/jobs/$id/state"; else echo "FAILED" > "$dir/jobs/$id/state"; fi
  echo "$code" > "$dir/jobs/$id/exit_code"
) &
if [ "$parsable" -eq 1 ]; then
  echo "$id"
else
  echo "Submitted batch job $id" >&2
  echo "$id"
fi
```

`squeue`:

```bash
#!/bin/bash
# Mock squeue — answers `squeue -j <ids> --noheader -o "%i|%t"`.
set -eu
dir="${MOCK_SCHEDULER_DIR:?MOCK_SCHEDULER_DIR not set}"
ids=""
fmt="%i|%t"
while [ $# -gt 0 ]; do
  case "$1" in
    -j) ids="$2"; shift 2;;
    -o) fmt="$2"; shift 2;;
    --noheader) shift;;
    *) shift;;
  esac
done
for id in $(echo "$ids" | tr ',' ' '); do
  [ -d "$dir/jobs/$id" ] || continue
  state=$(cat "$dir/jobs/$id/state" 2>/dev/null || echo "PENDING")
  line="$fmt"
  line=${line//%i/$id}
  line=${line//%t/$state}
  echo "$line"
done
```

`scancel`:

```bash
#!/bin/bash
# Mock scancel — kills the job process and marks it cancelled.
set -eu
dir="${MOCK_SCHEDULER_DIR:?MOCK_SCHEDULER_DIR not set}"
for id in "$@"; do
  [ -d "$dir/jobs/$id" ] || continue
  if [ -f "$dir/jobs/$id/pid" ]; then
    pid=$(cat "$dir/jobs/$id/pid")
    kill "$pid" 2>/dev/null || true
  fi
  echo "CANCELLED" > "$dir/jobs/$id/state"
done
```

`sacct`:

```bash
#!/bin/bash
# Mock sacct — answers `sacct -j <id> --format=JobID,State,ExitCode,Elapsed,MaxRSS`.
set -eu
dir="${MOCK_SCHEDULER_DIR:?MOCK_SCHEDULER_DIR not set}"
id=""
while [ $# -gt 0 ]; do
  case "$1" in
    -j) id="$2"; shift 2;;
    *) shift;;
  esac
done
[ -d "$dir/jobs/$id" ] || { echo "slurm_load_jobs error: Invalid job id specified"; exit 1; }
state=$(cat "$dir/jobs/$id/state")
code=$(cat "$dir/jobs/$id/exit_code" 2>/dev/null || echo "")
echo "JobID|State|ExitCode|Elapsed|MaxRSS"
echo "$id|$state|$code|00:00:05|1234K"
```

- [ ] **Step 2: Smoke test by hand**

```bash
chmod +x tests/fixtures/mock-scheduler/*
d=$(mktemp -d); MOCK_SCHEDULER_DIR="$d" PATH="$(pwd)/tests/fixtures/mock-scheduler:$PATH" bash -c '
  echo "#!/bin/bash
  echo hello > out.txt" > /tmp/job.sh
  id=$(sbatch --parsable /tmp/job.sh)
  sleep 1
  squeue -j "$id" --noheader -o "%i|%t"
  sacct -j "$id" --format=JobID,State,ExitCode,Elapsed,MaxRSS
  cat out.txt'
```
Expected: `1|COMPLETED` (or RUNNING if raced — accept both), sacct table, `hello`.

- [ ] **Step 3: Commit** — `git commit -m "test: mock SLURM scheduler fixtures for cluster CI (issue #78/#74)"`

## Task 5: `BackendDriver`

**Files:**
- Create: `crates/oxo-flow-core/src/backend/driver.rs`
- Test: inline in `driver.rs` (runs against the mock fixtures via `PATH` override)

**Interfaces:**
- Consumes: Tasks 1–3. `JobRecord`/`JobStatus` from `crate::executor`.
- Produces (used by Tasks 6, 15):
  - `pub struct DriverConfig { pub max_submitted: usize, pub poll_interval: std::time::Duration, pub poll_timeout: Option<std::time::Duration> }` + `Default` (max_submitted 50, poll_interval 5 s, poll_timeout None)
  - `pub struct BackendDriver { backend: Arc<dyn ExecutorBackend>, cluster: ClusterJobConfig, config: DriverConfig }` with `pub fn new(...) -> Self`
  - `pub struct DriverOptions<'a> { pub run_dir: &'a Path, pub reentry: Option<Box<dyn FnMut(&str) -> Result<Vec<String>> + 'a>> }`
  - `pub async fn run(&self, plan: &mut ScheduledPlan, to_run: &HashSet<String>, opts: DriverOptions<'_>) -> Result<Vec<JobRecord>>`
  - `pub async fn cancel_inflight(&self, jobs: &[(String, String)]) -> Result<()>`

- [ ] **Step 1: Write failing tests** (inline; each test sets `MOCK_SCHEDULER_DIR` + `PATH` with the fixture dir and builds a tiny plan via Task 1):

```rust
// helper: build a 3-rule plan a -> b -> c with shell "echo x > out.txt" etc.
// helper: run_driver(plan, to_run, run_dir) → records

#[tokio::test]
async fn driver_executes_chain_and_records_success() { /* 3 records Success, outputs exist in workdir, events.jsonl has 3 SUBMITTED + 3 COMPLETED */ }
#[tokio::test]
async fn driver_propagates_failure_and_skips_dependents() { /* b's shell is "exit 3" → records: a Success, b Failed, c Skipped(skip_reason contains "blocked by failed upstream") */ }
#[tokio::test]
async fn driver_respects_max_submitted() { /* chain of 4, max_submitted=2 → events.jsonl never shows >2 overlapping SUBMITTED-before-terminal; assert via parsing events */ }
#[tokio::test]
async fn driver_cancels_inflight_on_submit_error() { /* PATH without sbatch → run() returns Err; no panic */ }
```

- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement** the driver. Core structure:

```rust
//! BackendDriver: executes a [`ScheduledPlan`] through an [`ExecutorBackend`].
//!
//! The driver never decides *what* to run: the caller computes `to_run` from
//! the shared invalidation predicates (run_preview). It submits waves of at
//! most `max_submitted` in-flight jobs, polls, and maps scheduler states to
//! [`JobRecord`]s — the same record type the local executor produces, so
//! checkpoint semantics are identical.

use super::{BackendJobStatus, ExecutorBackend, ScheduledPlan};
use crate::cluster::ClusterJobConfig;
use crate::error::{OxoFlowError, Result};
use crate::executor::{JobRecord, JobStatus};
use chrono::Utc;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct DriverConfig { pub max_submitted: usize, pub poll_interval: Duration, pub poll_timeout: Option<Duration> }
impl Default for DriverConfig { fn default() -> Self { Self { max_submitted: 50, poll_interval: Duration::from_secs(5), poll_timeout: None } } }

pub struct BackendDriver { backend: Arc<dyn ExecutorBackend>, cluster: ClusterJobConfig, config: DriverConfig }
impl BackendDriver {
    pub fn new(backend: Arc<dyn ExecutorBackend>, cluster: ClusterJobConfig, config: DriverConfig) -> Self { Self { backend, cluster, config } }
    pub async fn cancel_inflight(&self, jobs: &[(String, String)]) -> Result<()> {
        for (id, _rule) in jobs { let _ = self.backend.cancel(id).await; }
        Ok(())
    }

    pub async fn run(&self, plan: &mut ScheduledPlan, to_run: &HashSet<String>, opts: DriverOptions<'_>) -> Result<Vec<JobRecord>> {
        // -- setup --
        std::fs::create_dir_all(opts.run_dir).map_err(...)?;
        let events_path = opts.run_dir.join("events.jsonl");
        let mut events = std::fs::OpenOptions::new().create(true).append(true).open(&events_path)?;
        let started = Instant::now();
        let deadline = self.config.poll_timeout.map(|t| started + t);
        let mut records: Vec<JobRecord> = Vec::new();
        let mut done: HashMap<String, JobStatus> = HashMap::new(); // rule → Success/Failed/Skipped
        let mut inflight: HashMap<String, (String, Instant)> = HashMap::new(); // job id → (rule, submitted_at)

        // -- helper closures --
        let deps_ok = |rule: &str, done: &HashMap<String, JobStatus>| -> bool {
            plan.rules.get(rule).map(|r| r.dependencies.iter().all(|d| matches!(done.get(d), Some(JobStatus::Success | JobStatus::Skipped)))).unwrap_or(true)
        };
        let emit = |events: &mut std::fs::File, line: String| -> std::io::Result<()> {
            use std::io::Write; writeln!(events, "{line}")
        };

        loop {
            // 1. failure propagation: pending rules blocked by a failed dep
            let failed: Vec<String> = done.iter().filter(|(_, s)| **s == JobStatus::Failed).map(|(r, _)| r.clone()).collect();
            for name in plan.order.iter().filter(|n| to_run.contains(*n) && !done.contains_key(*n)) {
                let blocked = plan.rules[name].dependencies.iter().any(|d| failed.contains(d));
                if blocked {
                    records.push(JobRecord { rule: name.clone(), status: JobStatus::Skipped, started_at: None, finished_at: Some(Utc::now()), exit_code: None, stdout: None, stderr: None, command: None, retries: 0, timeout: None, skip_reason: Some("blocked by failed upstream dependency".into()) });
                    done.insert(name.clone(), JobStatus::Skipped);
                    let _ = emit(&mut events, format!(r#"{{"t":"SKIPPED","rule":{name:?},"reason":"blocked by failed upstream"}}"#));
                }
            }

            // 2. submit a wave of ready rules
            let ready: Vec<String> = plan.order.iter().filter(|n| to_run.contains(*n) && !done.contains_key(*n) && deps_ok(n, &done)).cloned().collect();
            let slots = self.config.max_submitted.saturating_sub(inflight.len());
            for name in ready.into_iter().take(slots) {
                let sr = &plan.rules[&name];
                let script = self.backend.render_script(sr, &self.cluster)?;
                let job_dir = opts.run_dir.join("jobs").join(sanitize(&name));
                std::fs::create_dir_all(&job_dir)?;
                let script_path = job_dir.join("job.sh");
                std::fs::write(&script_path, &script)?;
                let job_id = self.backend.submit(&script_path).await.map_err(|e| { /* cancel-inflight on error */ e })?;
                // persist job.id + stdout/stderr log file names for the run directory
                std::fs::write(job_dir.join("job.id"), &job_id)?;
                inflight.insert(job_id.clone(), (name.clone(), Instant::now()));
                let _ = emit(&mut events, format!(r#"{{"t":"SUBMITTED","rule":{name:?},"job":{job_id:?}}}"#));
            }

            // 3. poll and settle terminal jobs
            if inflight.is_empty() {
                let all_done = plan.order.iter().all(|n| !to_run.contains(n) || done.contains_key(n));
                if all_done { break; }
                // nothing in flight but rules pending → dependency deadlock (shouldn't happen for DAG plans)
                tokio::time::sleep(self.config.poll_interval).await;
                continue;
            }
            let ids: Vec<String> = inflight.keys().cloned().collect();
            let statuses = self.backend.poll(&ids).await?;
            for (job_id, (rule, submitted_at)) in inflight.clone() {
                match statuses.get(&job_id) {
                    Some(BackendJobStatus::Completed) => {
                        inflight.remove(&job_id);
                        let status = if plan.rules[&rule].rule.checkpoint && let Some(ref mut f) = opts.reentry {
                            match f(&rule) { Ok(new_names) => { if !new_names.is_empty() { plan.merge_new_instances(...)?; /* extend to_run via records below */ } JobStatus::Success }, Err(e) => JobStatus::Failed /* manifest error fails the checkpoint rule */ }
                        } else { JobStatus::Success };
                        records.push(JobRecord { rule: rule.clone(), status, started_at: Some(submitted_at.into()), finished_at: Some(Utc::now()), exit_code: Some(0), stdout: None, stderr: None, command: plan.rules.get(&rule).map(|r| r.shell_cmd.clone()), retries: 0, timeout: None, skip_reason: None });
                        done.insert(rule.clone(), status);
                        let _ = emit(&mut events, format!(r#"{{"t":"COMPLETED","rule":{rule:?},"job":{job_id:?}}}"#));
                    }
                    Some(BackendJobStatus::Failed) => { /* JobRecord Failed, exit_code Some(1); done.insert(rule, Failed) */ }
                    Some(BackendJobStatus::Cancelled) => { /* JobRecord Cancelled */ }
                    _ => {}
                }
            }
            if let Some(d) = deadline && Instant::now() > d { self.cancel_inflight(&inflight.iter().map(|(id, (r, _))| (id.clone(), r.clone())).collect::<Vec<_>>()).await; return Err(OxoFlowError::Config { message: "poll timeout exceeded".into() }); }
            tokio::time::sleep(self.config.poll_interval).await;
        }
        Ok(records)
    }
}

fn sanitize(name: &str) -> String { name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '=') { c } else { '_' }).collect() }
```

Notes for the implementer: write `status.json` (state + submitted_at + finished_at) into each job dir when settling; the `Instant` → `DateTime<Utc>` conversion goes through `Utc::now() - started.elapsed() + submitted_at.duration_since(started)` (a `submitted_utc: Vec<DateTime<Utc>>` map is simpler — build it at submit time with `Utc::now()`). The reentry branch above is fleshed out in Task 15 — Task 5 can ship it as a no-op placeholder return of `JobStatus::Success` (the callback runs, new instances merge). Keep the reentry test in Task 15.

- [ ] **Step 4: Run to verify PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(core): BackendDriver — submit/poll loop with queue cap and failure propagation (#78)"`

## Task 6: P1 E2E parity + preview parity integration tests

**Files:**
- Create: `tests/cluster_backend.rs` (workspace root; copy the `workspace_bin` helper style from `tests/cli_integration.rs`)

**Interfaces:**
- Consumes: Tasks 1–5; the `oxo-flow` binary; `checkpoint.json` format; dry-run `--json` output (see `run_preview.rs` JSON structs).

- [ ] **Step 1: Write the failing tests**

```rust
//! P1 acceptance (issue #78): local execution and cluster-backend execution
//! of the same workflow produce the same checkpoint semantics; the dry-run
//! preview's will-run set equals what the driver submits.

fn write_wf(dir: &Path) { /* sample_pattern = "data/{sample}.fq", 3 sample files, rules: align{sample} -> stats{sample} with a dependency chain; stats reads align output */ }
fn mock_env(cmd: &mut StdCommand, state: &Path) -> &mut StdCommand { cmd.env("MOCK_SCHEDULER_DIR", state).env("PATH", format!("{}:{}", fixtures_dir().display(), std::env::var("PATH").unwrap())) }
fn fixtures_dir() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock-scheduler") }

#[test]
fn local_run_and_backend_run_produce_same_checkpoint_semantics() {
    // -- local --
    let dir_a = tempfile::tempdir().unwrap();
    write_wf(dir_a.path());
    // run: oxo-flow run wf.oxoflow -j 2 (workdir defaults to workflow dir)
    let out = run_oxo(dir_a.path(), &["run", "wf.oxoflow", "-j", "2"]); assert!(out.status.success());
    let ck_a: serde_json::Value = read checkpoint.json; completed_a = sorted completed_rules;

    // -- backend (mock SLURM) --
    let dir_b = tempfile::tempdir().unwrap();
    write_wf(dir_b.path());
    let scheduler_state = dir_b.path().join("sched"); create_dir;
    // drive the driver from a small #[tokio::test]-style async block: parse config, expand, dag, plan,
    // to_run = all rules, BackendDriver+ClusterExecutor with PATH/MOCK_SCHEDULER_DIR set on the CURRENT
    // process env for the driver's child processes (std::env::set_var is unsafe in Rust 2024 tests —
    // instead spawn the driver inside a helper BINARY or use the env-set approach via a dedicated
    // test harness thread). Simplest: a tiny example bin? No — use `tokio::process::Command` from the
    // test process: the driver spawns scheduler calls via tokio::process::Command which INHERITS the
    // test process env; set env via `std::env::set_var` guarded by a mutex (single-threaded test) —
    // Rust 2024 forbids set_var in multithreaded tests; use a static Mutex and `#[test]` (not tokio::test)
    // wrapping a tokio runtime block_on. (Follow the pattern already used in tests/web_integration.rs
    // if one exists; otherwise the mutex-guarded set_var in a #[test] fn is the way.)
    // After the driver run: write checkpoint.json from the returned records:
    //   CheckpointState { completed_rules = records where Success/Skipped(blocked)... } — for parity,
    //   complete rules = Success only (local checkpoint records Success for executed, and Skipped is not
    //   in completed_rules; blocked rules appear in failed_rules? Check run.rs semantics and mirror them.)
    let completed_b = sorted Success rules;
    assert_eq!(completed_a, completed_b);

    // outputs identical
    for f in walk(dir_a) filter files not checkpoint/oxo-flow internals:
        assert files_exist_and_bytes_equal(dir_b counterpart);

    // checkpoint shape parity: same benchmark keys, same input_manifests keys (values differ in mtime)
}

#[test]
fn dry_run_will_run_set_equals_driver_submitted_set() {
    // dir_a: oxo-flow dry-run wf.oxoflow --json → parse will-run rule list
    // dir_b: driver with to_run = that list → assert events.jsonl SUBMITTED rules == will-run list
}

#[test]
fn backend_failure_propagates_and_inflight_is_cancelled() {
    // workflow: branch A (sleep 5 in shell) parallel to branch B (exit 3) — both after a common first rule.
    // driver run → records contain Failed for B, Skipped for its dependents; after the run the mock
    // scheduler state shows A's job CANCELLED (driver cancelled in-flight on completion of all terminal? —
    // per design: cancel_inflight fires on error paths AND after the run settles: assert A cancelled).
}
```

Exact assertion targets for the dry-run `--json` schema: read `crates/oxo-flow-cli/src/commands/run_preview.rs` for the JSON field names (issue #63 added report/JSON) and match them here. The local checkpoint completion semantics (which statuses enter `completed_rules`) are in `run.rs` — mirror them in the test's checkpoint writer.

- [ ] **Step 2: Run to verify FAIL** (no `tests/cluster_backend.rs` semantics yet / driver missing).
- [ ] **Step 3: Implement what the tests need** (mostly test-side; any uncovered driver gap — e.g. missing `status.json` write — gets fixed here, not in Task 5).
- [ ] **Step 4: Run to verify PASS** — `cargo test -p oxo-flow --test cluster_backend`.
- [ ] **Step 5: Commit** — `git commit -m "test: P1 acceptance — local vs backend checkpoint parity, preview parity, failure cancellation (#78)"`

## Task 7: CLI `cluster` command refactor through the trait + `mkdir -p logs`

**Files:**
- Modify: `crates/oxo-flow-cli/src/commands/cluster.rs`
- Test: extend an existing CLI integration test or `tests/cluster_backend.rs` (byte parity of rendered scripts)

**Interfaces:**
- Consumes: Tasks 1–3 (`ClusterExecutor::render_script`, `ScheduledRule`).

- [ ] **Step 1: Write failing test** — `cluster submit` on a 2-rule workflow writes scripts; assert each script is byte-identical to `generate_submit_script_with_env` output computed in the test process (link against oxo-flow-core, which the root test crate can do), and that `logs/` exists before the script references it (check the `--output` directive in `cluster.rs` — if present, create its parent dir before writing).
- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement**

In `commands/cluster.rs`, replace the `generate_submit_script_with_env` block (lines ~224-231) with:

```rust
let wrapped_cmd = env_resolver
    .wrap_command(&shell_cmd, &rule.environment, Some(&rule.resources), Path::new("."))
    .map_err(|e| anyhow::anyhow!("environment wrapping failed: {}", e))?;
let scheduled = oxo_flow_core::backend::ScheduledRule {
    rule: rule.clone(),
    shell_cmd: wrapped_cmd,
    workdir: PathBuf::from("."),
    dependencies: dag.dependencies(rule_name).unwrap_or_default(),
    wildcard_values: wildcard_values.clone(),
};
let executor = oxo_flow_core::backend::cluster::ClusterExecutor::new(cluster_backend);
let script = executor.render_script(&scheduled, &cluster_config)?;
```

and, before writing each script, create any log directory the script's `--output` directive references (grep `core/cluster.rs` for `--output`; compute the dir from the same string the generator uses; `std::fs::create_dir_all` it under the workflow dir — the generator derives it from `rule.log`; follow the same derivation).

- [ ] **Step 4: Run to verify PASS.**
- [ ] **Step 5: Commit** — `git commit -m "refactor(cli): cluster submit renders via ExecutorBackend trait; create log dirs before scripts (#78)"`

---

# PART 2 — Unified storage invalidation (etag-aware manifests)

## Task 8: `StorageBackend::head` + `RemoteStat` (local / s3 / gcs)

**Files:**
- Modify: `crates/oxo-flow-core/src/storage/mod.rs` (trait + type), `storage/local.rs`, `storage/s3.rs`, `storage/gcs.rs`
- Test: inline in `storage/mod.rs` (fake backend), `storage/s3.rs` (fixture), `storage/gcs.rs` (fixture)

**Interfaces:**
- Consumes: `StoragePath`, `aws_sdk_s3` (feature `s3-storage`), gcs XML helpers.
- Produces (used by Tasks 9–11):
  - `pub struct RemoteStat { pub size: u64, pub etag: Option<String> }` (Clone, Debug, PartialEq, Eq)
  - `StorageBackend::head(&self, path: &StoragePath) -> Result<Option<RemoteStat>>`

- [ ] **Step 1: Write failing tests**

```rust
// storage/mod.rs
#[tokio::test]
async fn head_defaults() { assert!(StorageResolver::with_local().get_backend(&StorageScheme::Local).unwrap().head(&StoragePath::parse("/x")).await.unwrap().is_none()); }

// storage/s3.rs (feature-gated): parse a canned HeadObject XML response is NOT possible with aws-sdk's
// typed client without a mock server — instead unit-test the mapping via a fake client is also heavy.
// Pragmatic: test S3 head() error-path shape only (missing bucket → Config error) — the response
// mapping is exercised by the in-memory fake in Task 10. Same for GCS: test the x-goog-hash parsing
// helper directly.
#[cfg(feature = "gcs-storage")]
#[test]
fn parse_gcs_md5_header() {
    assert_eq!(gcs::parse_md5_hash_header(Some("md5=oVPGkKJcW4+2nW/eW3B+WA==")), Some("oVPGkKJcW4+2nW/eW3B+WA=="));
    assert_eq!(gcs::parse_md5_hash_header(Some("crc32c=xyz,md5=abc==")), Some("abc=="));
    assert_eq!(gcs::parse_md5_hash_header(None), None);
    assert_eq!(gcs::parse_md5_hash_header(Some("md5=bad*chars")), None);
}
```

- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement**

```rust
// storage/mod.rs
/// Metadata of a remote object, used for content-addressed invalidation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteStat { pub size: u64, pub etag: Option<String> }

// trait addition (after `exists`):
/// Return metadata for a remote object, or `Ok(None)` when it does not exist.
/// Local backends return `Ok(None)` — local invalidation uses size+mtime+sha256.
async fn head(&self, path: &StoragePath) -> Result<Option<RemoteStat>>;
```

```rust
// storage/local.rs
async fn head(&self, _path: &StoragePath) -> Result<Option<RemoteStat>> { Ok(None) }
```

```rust
// storage/s3.rs — refactor exists() to share the HEAD call:
async fn head(&self, path: &StoragePath) -> Result<Option<RemoteStat>> {
    let bucket = require_bucket(path)?;
    match self.client.head_object().bucket(bucket).key(&path.key).send().await {
        Ok(resp) => Ok(Some(RemoteStat {
            size: resp.content_length().unwrap_or(0) as u64,
            etag: resp.e_tag().map(str::to_string),
        })),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("NotFound") || msg.contains("404") { Ok(None) }
            else { Err(s3_error(format!("head_object failed for s3://{bucket}/{}: {msg}", path.key))) }
        }
    }
}
// exists() becomes: Ok(self.head(path).await?.is_some())
```

```rust
// storage/gcs.rs — add a metadata variant of the existing gcs_head:
/// Parse the `md5=` component of an `x-goog-hash` header (base64, kept verbatim).
pub fn parse_md5_hash_header(header: Option<&str>) -> Option<String> {
    let header = header?;
    header.split(',').map(str::trim)
        .find_map(|part| part.strip_prefix("md5="))
        .filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='))
        .map(str::to_string)
}
// gcs_head_stat(bucket, key) -> Result<Option<RemoteStat>> mirrors gcs_head's request but returns
// (content-length, x-goog-hash) instead of bool; gcs_head becomes .map(|s| s.is_some()) — refactor
// the existing helper to return the stat and keep `exists` calling it.
```

- [ ] **Step 4: Run to verify PASS** (including `--features s3-storage,gcs-storage` compile check: `cargo check -p oxo-flow-core --features s3-storage,gcs-storage`).
- [ ] **Step 5: Commit** — `git commit -m "feat(core): StorageBackend::head + RemoteStat (S3 ETag / GCS md5Hash) (#78)"`

## Task 9: `RemoteManifestEntry` + `InputManifestEntry.remote` + legacy compatibility

**Files:**
- Modify: `crates/oxo-flow-core/src/executor/checkpoint.rs`
- Test: inline in `checkpoint.rs`

**Interfaces:**
- Consumes: Task 8 (`RemoteStat`).
- Produces (used by Tasks 10–11): `pub struct RemoteManifestEntry { pub scheme: String, pub key: String, pub size: u64, #[serde(default, skip_serializing_if = "Option::is_none")] pub etag: Option<String> }`; `InputManifestEntry.remote: Option<RemoteManifestEntry>`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn legacy_manifest_json_loads_without_remote() {
    let json = r#"[{"path":"data/S1.fq","size":100,"mtime_nanos":5}]"#;
    let entries: InputManifest = serde_json::from_str(json).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].remote.is_none());
}
#[test]
fn remote_entry_roundtrips() {
    let e = RemoteManifestEntry { scheme: "s3".into(), key: "s3://b/k".into(), size: 42, etag: Some("\"abc\"".into()) };
    let s = serde_json::to_string(&e).unwrap();
    let back: RemoteManifestEntry = serde_json::from_str(&s).unwrap();
    assert_eq!(back, e);
}
```

- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement** — add the struct + field exactly as declared above (serde defaults as in the Produces block).
- [ ] **Step 4: Run to verify PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(core): remote manifest entries (scheme/key/etag/size), backward compatible (#78)"`

## Task 10: `snapshot_input_manifest` remote support + `manifests_match` remote matrix

**Files:**
- Modify: `crates/oxo-flow-core/src/executor/checkpoint.rs`
- Test: inline in `checkpoint.rs` (in-memory fake backend with mutable etag)

**Interfaces:**
- Consumes: Task 8 (`StorageBackend::head`), Task 9, `StoragePath`.
- Produces (used by Task 11):
  - `snapshot_input_manifest(rule: &Rule, workdir: &Path, wildcard_values: &HashMap<String, String>, resolver: &StorageResolver) -> Result<Option<InputManifest>>` (new resolver param)
  - `manifests_match` remote branch.

- [ ] **Step 1: Write failing tests**

```rust
struct FakeCloudStorage { etags: Arc<Mutex<HashMap<String, String>>> }
#[async_trait::async_trait]
impl StorageBackend for FakeCloudStorage {
    // head(): look up the etag map by key; missing key → Ok(None); present → Some(RemoteStat{size: 100, etag: Some(v)})
    // exists/read_to_string/write/stage/upload: trivial (unused in this test)
}

#[tokio::test]
async fn same_size_etag_change_invalidates_remote_input() {
    let fake = Arc::new(FakeCloudStorage { etags: Arc::new(Mutex::new(HashMap::from([("s3://b/k".into(), "v1".into())]))) });
    let mut resolver = StorageResolver::new();
    resolver.add_backend(StorageScheme::S3, fake.clone());
    let rule: Rule = /* input = ["s3://b/k"], output = ["out.txt"], shell = "true" */;
    let recorded = snapshot_input_manifest(&rule, Path::new("."), &HashMap::new(), &resolver).await... // (fn stays sync; head is async — snapshot becomes async OR uses a blocking bridge. Decision: make snapshot_input_manifest async and update call sites accordingly (run.rs call site already inside async fn).)
    // ^ NOTE for implementer: making this async is the cleanest path; run.rs:1263 is inside an async block.
    let recorded = recorded.unwrap().unwrap();
    assert!(recorded[0].remote.as_ref().unwrap().etag.as_deref() == Some("v1"));

    // etag changes, same size → mismatch
    *fake.etags.lock().unwrap().get_mut("s3://b/k").unwrap() = "v2".to_string();
    let current = snapshot_input_manifest(&rule, Path::new("."), &HashMap::new(), &resolver).await.unwrap().unwrap();
    assert!(!manifests_match(&recorded, &current));

    // etag unchanged → match
    let again = snapshot_input_manifest(&rule, Path::new("."), &HashMap::new(), &resolver).await.unwrap().unwrap();
    assert!(manifests_match(&recorded, &again));
}

#[test]
fn manifests_match_remote_matrix() {
    // build two-entry manifests by hand: scheme mismatch → false; etag None on both → size decides;
    // local vs remote → false; local vs local unchanged behaviour.
}
```

- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement**

`snapshot_input_manifest` becomes `pub async fn`; in the pattern loop after `expand_config_in_path`:

```rust
let sp = StoragePath::parse(&expanded);
if sp.is_remote() {
    match resolver.get_backend(&sp.scheme).and_then(|b| Some(b.clone())) {
        Some(backend) => match backend.head(&sp).await {
            Ok(Some(stat)) => {
                entries.insert(expanded.clone(), InputManifestEntry {
                    path: expanded.clone(),
                    size: stat.size,
                    mtime_nanos: 0,
                    hash: None,
                    remote: Some(RemoteManifestEntry {
                        scheme: match sp.scheme { StorageScheme::S3 => "s3", StorageScheme::Gcs => "gs", StorageScheme::Local => "local" }.to_string(),
                        key: expanded.clone(),
                        size: stat.size,
                        etag: stat.etag,
                    }),
                });
                saw_resolvable = true;
                continue;
            }
            Ok(None) => { tracing::warn!(input = %expanded, "remote input does not exist at snapshot time; entry skipped"); continue; }
            Err(e) => { tracing::warn!(input = %expanded, error = %e, "remote input metadata unavailable; entry skipped"); continue; }
        },
        None => { tracing::warn!(input = %expanded, "no storage backend registered for scheme; entry skipped"); continue; }
    }
}
// existing local branch unchanged
```

`manifests_match` entry-wise branch:

```rust
recorded.iter().zip(current).all(|(r, c)| match (&r.remote, &c.remote) {
    (None, None) => r.path == c.path && r.size == c.size && match &r.hash {
        Some(rec_hash) => c.hash.as_deref() == Some(rec_hash.as_str()),
        None => r.mtime_nanos == c.mtime_nanos,
    },
    (Some(rr), Some(rc)) => rr.scheme == rc.scheme && rr.key == rc.key && rr.size == rc.size
        && match (&rr.etag, &rc.etag) { (Some(a), Some(b)) => a == b, _ => true },
    _ => false,
})
```

- [ ] **Step 4: Run to verify PASS.** Update the existing `snapshot_input_manifest` call sites in core (checkpoint.rs internal callers/tests) — the compiler will list them.
- [ ] **Step 5: Commit** — `git commit -m "feat(core): etag-aware remote manifest snapshot + unified manifests_match (#78)"`

## Task 11: Call-site threading + fake-backend invalidation integration test

**Files:**
- Modify: `crates/oxo-flow-cli/src/commands/run.rs` (two one-line resolver passes at the `snapshot_input_manifest` call sites), any other caller the compiler lists.
- Test: `tests/` or inline — an integration test in `tests/cluster_backend.rs` is NOT the place; add to `tests/integration_test.rs` or a new `tests/storage_invalidation.rs`: a workflow whose input is `s3://`… but the real CLI has no s3 backend by default. Instead: core-level test (Task 10's fake) is the semantic proof; add a CLI-level test only for "remote URI in inputs warns and degrades gracefully" (no backend registered): run the binary with an `s3://` input → run completes, checkpoint records no manifest for that entry, a warning is logged.

**Interfaces:**
- Consumes: Task 10 signatures.

- [ ] **Step 1: Write failing test** — `tests/storage_invalidation.rs`: workflow with `input = ["s3://b/k", "data/S1.fq"]`; run completes; `checkpoint.json` input_manifests contains the local entry and no remote entry (no backend registered); stderr contains "no storage backend registered".
- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement** — resolver construction in run.rs:

```rust
let mut storage = oxo_flow_core::storage::StorageResolver::with_local();
#[cfg(feature = "s3-storage")]
storage.add_backend(oxo_flow_core::storage::StorageScheme::S3, Arc::new(oxo_flow_core::storage::s3::S3Storage::new()));
#[cfg(feature = "gcs-storage")]
storage.add_backend(oxo_flow_core::storage::StorageScheme::Gcs, Arc::new(oxo_flow_core::storage::gcs::GcsStorage::new()));
```

Then pass `&storage` at both snapshot sites (run.rs ~1263 and the dry-run/preview snapshot if present). Check whether the CLI crate enables `s3-storage`/`gcs-storage` on its core dependency (cli/Cargo.toml) — enable them so the cfg branches compile (they're already deps of core behind features; enabling adds aws-sdk build time — if the CLI doesn't enable them today, do NOT enable; instead gate the registration code with the same cfg and leave a comment).
- [ ] **Step 4: Run to verify PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(cli): thread StorageResolver into manifest snapshots; graceful remote-input degradation (#78)"`

---

# PART 3 — Checkpoint re-entry (static + dynamic hybrid DAG)

## Task 12: `checkpoint_manifest` field + validation rules

**Files:**
- Modify: `crates/oxo-flow-core/src/rule.rs` (field), `crates/oxo-flow-core/src/format.rs` (validation near the existing W010 checkpoint check at ~line 782)
- Test: inline in `format.rs` tests + a serde roundtrip in `rule.rs` tests

**Interfaces:**
- Consumes: `Rule`, `Diagnostic` (format.rs types).
- Produces: `Rule.checkpoint_manifest: Option<String>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`, after `checkpoint: bool`).

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn checkpoint_without_manifest_is_error() {
    // workflow: rule with checkpoint = true and no checkpoint_manifest → validation returns
    // an Error-severity diagnostic with code "E0xx" (pick the next free E-code in format.rs,
    // e.g. E013 — check the existing list and append).
}
#[test]
fn checkpoint_rule_with_sample_wildcard_is_error() {
    // checkpoint = true + shell containing {sample} → Error diagnostic.
}
#[test]
fn checkpoint_rule_with_manifest_validates() { /* checkpoint=true + checkpoint_manifest="d.toml", no wildcards → no Error */ }
```

- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement** — field + two validation branches following the exact `Diagnostic` style of the W010 block (message + rule + code + suggestion).
- [ ] **Step 4: Run to verify PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(core): checkpoint_manifest field + validation (bounded re-entry) (#78)"`

## Task 13: `rule_templates` preservation + `apply_reentry` / `replay_valid_reentries`

**Files:**
- Create: `crates/oxo-flow-core/src/reentry.rs`
- Modify: `crates/oxo-flow-core/src/config.rs` (`rule_templates` field with `#[serde(skip)]`; capture in `expand_wildcards`; `pub mod reentry;` in lib.rs)
- Test: inline in `reentry.rs`

**Interfaces:**
- Consumes: `WorkflowConfig`, `expand_wildcards`, `SampleGroup`, `Rule`.
- Produces (used by Tasks 14–17):
  - `pub const MAX_REENTRY_ROUNDS: u32 = 32;`
  - `pub struct ReentryRecord { pub round: u32, pub rule: String, pub group: Option<String>, pub samples: Vec<String> }` (Serialize, Deserialize, Clone, Debug, PartialEq, Eq)
  - `pub fn apply_reentry(config: &mut WorkflowConfig, group: Option<&str>, samples: &[String]) -> Result<Vec<String>>`
  - `pub fn replay_valid_reentries(config: &mut WorkflowConfig, records: &[ReentryRecord], valid_rules: &std::collections::HashSet<String>) -> Result<Vec<ReentryRecord>>`
  - `pub fn parse_manifest(content: &str) -> Result<(Option<String>, Vec<String>)>` (returns (group, samples))

- [ ] **Step 1: Write failing tests**

```rust
fn config_two_rules() -> WorkflowConfig { /* discover (no wildcards) + analyze (output "out/{sample}.txt") + group "batch" = ["S1"] */ }

#[test]
fn apply_reentry_adds_only_new_instances() {
    let mut config = config_two_rules();
    config.apply_defaults(); config.expand_wildcards().unwrap();
    let before: Vec<String> = config.rules.iter().map(|r| r.name.clone()).collect();
    let new = apply_reentry(&mut config, None, &["S2".into(), "S1".into() /* dup */]).unwrap();
    assert_eq!(new, vec!["analyze_batch_S2".to_string()]);
    // existing instances untouched (same names, same order prefix)
    assert!(before.iter().all(|n| config.rules.iter().any(|r| &r.name == n)));
}

#[test]
fn apply_reentry_is_idempotent() { /* second call with same samples returns empty vec */ }

#[test]
fn apply_reentry_unknown_group_creates_group() { /* group = Some("late") → instances analyze_late_S9 */ }

#[test]
fn replay_reentries_only_keeps_valid_records() {
    // records: [ {rule:"discover", samples:["S2"]} ]; valid_rules = {} → re-expansion from templates
    // yields NO analyze_batch_S2 (revoked); valid_rules = {"discover"} → instance present.
}

#[test]
fn parse_manifest_shapes() {
    assert_eq!(parse_manifest("[reentry]\nsample = [\"S4\"]\n").unwrap(), (None, vec!["S4".to_string()]));
    assert_eq!(parse_manifest("[reentry]\ngroup = \"g\"\nsample = [\"S4\",\"S5\"]\n").unwrap(), (Some("g".into()), vec!["S4".into(), "S5".into()]));
    assert!(parse_manifest("no reentry table").is_err());
}
```

- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement**

```rust
// config.rs — in expand_wildcards, FIRST lines:
if self.rule_templates.is_empty() {
    self.rule_templates = self.rules.clone();
}
```

```rust
// reentry.rs
//! Checkpoint re-entry (issue #78 P3): static + dynamic hybrid DAG.
//!
//! A `checkpoint = true` rule writes a TOML manifest at runtime declaring new
//! wildcard values (new samples); the engine merges them and re-expands the
//! rule templates — every round is still a static plan, so previews stay
//! deterministic and resumes reconstruct the same plan.

use crate::config::{SampleGroup, WorkflowConfig};
use crate::error::{OxoFlowError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_REENTRY_ROUNDS: u32 = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReentryRecord {
    pub round: u32,
    pub rule: String,
    pub group: Option<String>,
    pub samples: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Manifest { reentry: ReentryTable }
#[derive(Debug, Deserialize)]
struct ReentryTable { #[serde(default)] group: Option<String>, #[serde(default)] sample: Vec<String> }

/// Parse a checkpoint re-entry manifest: `(group, samples)`.
pub fn parse_manifest(content: &str) -> Result<(Option<String>, Vec<String>)> {
    let m: Manifest = toml::from_str(content).map_err(|e| OxoFlowError::Config {
        message: format!("invalid re-entry manifest: {e}"),
    })?;
    Ok((m.reentry.group, m.reentry.sample))
}

/// Merge new samples into the target group and re-expand from templates.
/// Returns the names of newly created instances (already in `config.rules`).
pub fn apply_reentry(config: &mut WorkflowConfig, group: Option<&str>, samples: &[String]) -> Result<Vec<String>> {
    let prev: HashSet<String> = config.rules.iter().map(|r| r.name.clone()).collect();
    let group_name = group.unwrap_or("auto-discovered");
    let added = merge_samples(config, group_name, samples);
    if added.is_empty() { return Ok(Vec::new()); }
    config.rules = config.rule_templates.clone();
    config.expand_wildcards()?;
    Ok(config.rules.iter().map(|r| r.name.clone()).filter(|n| !prev.contains(n)).collect())
}

fn merge_samples(config: &mut WorkflowConfig, group_name: &str, samples: &[String]) -> Vec<String> {
    let group = match config.sample_groups.iter_mut().find(|g| g.name == group_name) {
        Some(g) => g,
        None => { config.sample_groups.push(SampleGroup { name: group_name.to_string(), samples: Vec::new(), metadata: Default::default() }); config.sample_groups.last_mut().unwrap() }
    };
    let mut added = Vec::new();
    for s in samples { if !group.samples.contains(s) { group.samples.push(s.clone()); added.push(s.clone()); } }
    added
}

/// Replay recorded re-entries whose checkpoint rule still stands, then
/// re-expand from templates. Records for invalidated rules are revoked:
/// their samples are not merged, so their instances disappear from the plan.
pub fn replay_valid_reentries(config: &mut WorkflowConfig, records: &[ReentryRecord], valid_rules: &HashSet<String>) -> Result<Vec<ReentryRecord>> {
    let mut replayed = Vec::new();
    for rec in records {
        if valid_rules.contains(&rec.rule) {
            let group_name = rec.group.as_deref().unwrap_or("auto-discovered");
            merge_samples(config, group_name, &rec.samples);
            replayed.push(rec.clone());
        }
    }
    config.rules = config.rule_templates.clone();
    config.expand_wildcards()?;
    Ok(replayed)
}
```

Edge cases to handle in tests: `rule_templates` empty (expand never ran) → replay is a no-op re-expansion; `apply_reentry` with zero new samples (all dup) → no re-expansion, empty return.

- [ ] **Step 4: Run to verify PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(core): checkpoint re-entry — template preservation, merge, deterministic replay (#78)"`

## Task 14: `ReentryRecord` persistence in `CheckpointState` + `SchedulerState::add_rule`

**Files:**
- Modify: `crates/oxo-flow-core/src/executor/checkpoint.rs`, `crates/oxo-flow-core/src/scheduler.rs`
- Test: inline in both files

**Interfaces:**
- Consumes: Task 13 (`ReentryRecord`).
- Produces:
  - `CheckpointState.reentries: Vec<ReentryRecord>` (`#[serde(default, skip_serializing_if = "Vec::is_empty")]`)
  - `CheckpointState::record_reentry(&mut self, record: ReentryRecord)` — removes any existing record for the same rule, then pushes.
  - `SchedulerState::add_rule(&mut self, rule: &str)` — `statuses.entry(rule.to_string()).or_insert(JobStatus::Pending)`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn checkpoint_reentry_roundtrip_and_supersede() {
    let mut ck = CheckpointState::new();
    ck.record_reentry(ReentryRecord { round: 1, rule: "discover".into(), group: None, samples: vec!["S2".into()] });
    ck.record_reentry(ReentryRecord { round: 2, rule: "discover".into(), group: None, samples: vec!["S2".into(), "S3".into()] });
    assert_eq!(ck.reentries.len(), 1);
    assert_eq!(ck.reentries[0].samples, vec!["S2", "S3"]);
    let json = ck.to_json().unwrap();
    let back = CheckpointState::from_json(&json).unwrap();
    assert_eq!(back.reentries, ck.reentries);
}
#[test]
fn legacy_checkpoint_json_without_reentries_loads() { /* existing fixture JSON → reentries empty */ }

#[test]
fn add_rule_makes_rule_ready_when_deps_done() { /* SchedulerState::new(["a"]); add_rule("b"); mark a Success; ready_rules(dag with edge a->b) contains b */ }
```

- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement** — as declared.
- [ ] **Step 4: Run to verify PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(core): reentry records in checkpoint state; SchedulerState::add_rule (#78)"`

## Task 15: `run.rs` event-loop hookup (the only P3 loop edits)

**Files:**
- Modify: `crates/oxo-flow-cli/src/commands/run.rs`
- Test: integration — `tests/reentry_integration.rs` (drives the real `oxo-flow run` binary)

**Interfaces:**
- Consumes: Tasks 13–14; the run loop structure (order/order_set/dag/sched/checkpoint at run.rs ~1097-1290; the completion handler inside the spawned task / join_set collection).

- [ ] **Step 1: Write failing integration tests** (`tests/reentry_integration.rs`):

```rust
// workflow wf.oxoflow:
//   [[rules]] discover — shell: "printf '[reentry]\nsample = [\"S4\",\"S5\"]\n' > discover.toml",
//       checkpoint = true, checkpoint_manifest = "discover.toml", output = ["discover.toml"]
//   [[rules]] analyze — input = ["data/{sample}.fq"], output = ["out/{sample}.txt"], shell = "cp data/{sample}.fq out/{sample}.txt"
//   sample_pattern = "data/{sample}.fq" with data/S1.fq (S2/S3 absent — they arrive via re-entry)

#[test]
fn reentry_adds_round2_instances_and_records_rounds() {
    // run → out/S1.txt AND out/S4.txt, out/S5.txt exist (S4/S5 declared by discover at runtime)
    // checkpoint.json reentries has 1 record {rule:"discover", samples:["S4","S5"], round:1}
}

#[test]
fn resume_replays_reentry_deterministically() {
    // run again (nothing changed) → all 3 analyze instances skipped; same reentries record;
    // a dry-run preview shows the round-1 instances as up-to-date
}

#[test]
fn invalidating_checkpoint_rule_revokes_its_samples() {
    // touch data/S1.fq? No — discover doesn't read S1. Give discover input = ["catalog.txt"];
    // modify catalog.txt → run → discover re-executes → manifest now says sample = ["S4"] →
    // S5's instance revoked: out/S5.txt absent from plan (fresh workdir copy proves it) and the
    // reentries record is superseded
}

#[test]
fn missing_manifest_fails_the_checkpoint_rule() {
    // discover with shell "true" (writes nothing) → run fails; error message mentions
    // "re-entry manifest" and the rule name; dependents never run
}

#[test]
fn empty_manifest_is_valid_noop() { /* manifest `[reentry]\nsample = []` → success, no new instances */ }
```

- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement the hookup**

a) After the existing invalidation analysis and BEFORE the `rule_names`/sched setup (run.rs ~1090), insert:

```rust
// ── Checkpoint re-entry replay (issue #78 P3) ─────────────────────────
// Re-apply recorded re-entries whose checkpoint rule still stands, then
// rebuild the DAG so resume reconstructs the same static plan a fresh run
// would. Invalidated checkpoint rules are revoked: their samples leave
// the plan until the rule re-runs and re-records.
let valid_reentry_rules: HashSet<String> = /* the set of rules the pre-completion pass would mark
    completed: completed in checkpoint && !rerun && outputs_ok (reuse the same closure/predicates —
    extract the outputs_ok check into a local fn to avoid duplicating logic) */;
let ck = checkpoint.lock().await;
let replayed = oxo_flow_core::reentry::replay_valid_reentries(&mut config, &ck.reentries, &valid_reentry_rules)?;
drop(ck);
if !replayed.is_empty() {
    tracing::info!(count = replayed.len(), "replayed checkpoint re-entries");
}
// rebuild dag + order + order_set from the re-expanded config
let dag = WorkflowDag::from_rules(&config.rules).context("failed to rebuild workflow DAG after re-entry replay")?;
let order = /* recompute with targets as before */;
```

Note: `dag` and `order` bindings must become `let mut` if re-entry happens mid-loop (see c).

b) In `compute_ready`, add the dag as a parameter instead of a capture: `|sched, submitted, dag: &WorkflowDag|` and update the call site. (If the closure already takes dag some other way, adapt minimally.)

c) In the completion handling (where a successful `JobRecord` is recorded into the checkpoint), insert BEFORE the success accounting:

```rust
// ── Checkpoint re-entry processing (issue #78 P3) ─────────────────────
if rule.checkpoint && record.status == JobStatus::Success {
    match process_reentry(&mut config, &rule, workdir_actual.as_ref(), &wildcard_values,
                          &mut sched, &mut order, &mut order_set, &dag /* → rebuild */,
                          &checkpoint).await {
        Ok(()) => {}
        Err(e) => {
            // fail the checkpoint rule: rewrite record as Failed with the manifest error
            record.status = JobStatus::Failed;
            record.skip_reason = Some(format!("re-entry manifest: {e}"));
            record.exit_code = Some(1);
        }
    }
}
```

`process_reentry` (a new private async fn in run.rs, <50 lines): reads the manifest file (`workdir.join(expand_config_in_path(&manifest_path, &wildcard_values))`), `parse_manifest`, if samples non-empty → `apply_reentry` → for new names: extend `order`/`order_set`, `sched.add_rule(name)`; rebuild `dag` via `*dag = WorkflowDag::from_rules(&config.rules)?` (dag passed as `&mut WorkflowDag`); round = max existing round + 1 (cap `MAX_REENTRY_ROUNDS` → error); `checkpoint.lock().await.record_reentry(...)`.

Because the completion handler runs inside the spawned task (join_set), the dag/config/sched/order must be reachable there. Current task spawns capture clones only — restructure minimally: run the re-entry processing in the MAIN loop right after `join_set.join_next()` yields a Success (move the hook to the main loop, not the spawned task). Locate the join_next collection site and insert there; the main loop owns `config`, `dag`, `sched`, `order`, `order_set` (make the bindings `mut` as needed — `config` and `dag` may already be `mut`).

d) Deadlock check call sites use the (possibly rebuilt) `dag` — they already reference the binding, so rebinding works.

- [ ] **Step 4: Run to verify PASS** — `cargo test --test reentry_integration` plus the full existing suite (esp. `preview_parity`).
- [ ] **Step 5: Commit** — `git commit -m "feat(cli): checkpoint re-entry hook in the run loop (#78)"`

## Task 16: BackendDriver re-entry hook + driver-path integration test

**Files:**
- Modify: `crates/oxo-flow-core/src/backend/driver.rs` (flesh out the Task 5 reentry branch)
- Test: `tests/reentry_integration.rs` (driver path with mock scheduler)

**Interfaces:**
- Consumes: Tasks 13–15 pieces (`apply_reentry`, `ScheduledPlan::merge_new_instances`).

- [ ] **Step 1: Write failing test** — same discover workflow, driven by `BackendDriver` + mock SLURM: round-1 discover completes → callback `apply_reentry` returns new names → driver merges + executes `analyze_*_S4/S5` → records contain all 5 Success rules; events.jsonl shows SUBMITTED for the round-2 instances AFTER the discover COMPLETED line.
- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement** — in the Completed branch: call `opts.reentry`; when new names are returned, call `plan.merge_new_instances(config, dag, workdir, env_resolver, wildcard_values, &new_names)` — the driver needs these five inputs: extend `DriverOptions` with a `plan_builder` context struct holding (config, dag, workdir, env_resolver, wildcard_values) OR pass `&mut ScheduledPlan` plus a `merge: &dyn Fn(&mut ScheduledPlan, &[String]) -> Result<()>` closure. Cleanest: `DriverOptions { run_dir, merge: Option<Box<dyn FnMut(&mut ScheduledPlan, &[String]) -> Result<()> + '_>>, on_checkpoint: Option<Box<dyn FnMut(&str) -> Result<Vec<String>> + '_>> }`. Update Task 5's signature accordingly and its tests' constructions.
- [ ] **Step 4: Run to verify PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(core): BackendDriver re-entry hook — merge + execute round-2 instances (#78)"`

## Task 17: dry-run preview re-entry support

**Files:**
- Modify: `crates/oxo-flow-cli/src/commands/run_preview.rs` (and `dry_run_command` in run.rs where it consumes preview results)
- Test: extend `tests/preview_parity.rs`

**Interfaces:**
- Consumes: Tasks 13–14; the preview JSON structs.

- [ ] **Step 1: Write failing tests** — `preview_parity.rs`: (a) fresh workdir with a discover workflow → `dry-run --json` shows round-0 plan + a `reentry` section listing the checkpoint rule as a possible re-entry point with its manifest path; (b) after one real run (reentries recorded) → `dry-run --json` shows the round-1 instances as part of the static plan (deterministic reconstruction) and the same will-run set a real run would produce (no discover re-run → analyze instances up-to-date).
- [ ] **Step 2: Run to verify FAIL.**
- [ ] **Step 3: Implement** — in `dry_run_command` (or the shared preview entry): after the existing invalidation computation, call `replay_valid_reentries` with the preview's valid set (read-only: operate on a cloned config, as the preview already does), then re-expand and preview the resulting plan; annotate checkpoint rules (`checkpoint=true`) as potential re-entry points in the report; add the `reentry` field to the `--json` output struct (find the struct in run_preview.rs; add `#[serde(default)]`).
- [ ] **Step 4: Run to verify PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(cli): dry-run previews deterministic re-entry reconstruction (#78)"`

---

# PART 4 — Documentation, issue management, final gate

## Task 18: Documentation sync + full CI

**Files:**
- Modify: `docs/guide/src/` — new `execution-backends.md` (static plan → executor mapping, `cluster` command as render layer, mock-scheduler CI harness, assumptions: shared storage, outputs land where declared); extend `workflow-format.md` (`checkpoint_manifest`, re-entry manifest format, remote inputs + etag invalidation semantics, documented limitations: no remote globs/staging, pairs re-entry out of scope, `MAX_REENTRY_ROUNDS = 32`); extend `run.md`/`dry-run.md` re-entry semantics (resume determinism, revoke-on-invalidate, preview annotation); `status.md` checkpoint format gains `reentries` field; glossary entries (executor backend, re-entry, remote manifest, static plan).
- Modify: `docs/guide/mkdocs.yml` (nav entry for the new page).

- [ ] **Step 1:** Write the docs pages (English, matching existing tone; state assumptions explicitly per #74 comment 6).
- [ ] **Step 2:** `cd docs/guide && mkdocs build --strict` — fix any broken anchors/warnings.
- [ ] **Step 3:** Full gate: `make ci` from the workspace root (fmt + clippy -D warnings + build + test + audit) — all green; record the final test count.
- [ ] **Step 4:** Commit — `git commit -m "docs: execution backends, storage invalidation, checkpoint re-entry (#78)"`

## Task 19: Issue management + memory

- [ ] **Step 1:** Push all phase commits: `git push origin main`.
- [ ] **Step 2:** Comment + close #78: per-phase deliverables, test evidence (test names + counts), links to commits, the two spec refinements (ScheduledRule carries Rule; cancel-on-drop → cancel-on-error + `cancel_inflight`), and pointers to #74/#67 for real-cluster validation.
- [ ] **Step 3:** Reply on #74: foundation in tree — `ExecutorBackend` + `ClusterExecutor` (submit/poll/cancel/logs with `--parsable` ID capture = its Phase-1 item 1), shared `parse_job_id`/`parse_status_line` (its comment item 5), mock-scheduler fixtures under `tests/fixtures/mock-scheduler/` for its CI (comment item 3), `BackendDriver` with queue cap + events.jsonl + jobs/ run dir as the substrate for its Phase 2 wiring; `cluster.rs` untouched as agreed; P2/P3 do not overlap its scope.
- [ ] **Step 4:** Update project memory: new file `issue-78-static-plan-executors.md` (what shipped per phase, key gotchas: async snapshot_input_manifest, templates preserved on first expand, mock-scheduler env pattern, #74/#67 handoff) + MEMORY.md index line.
- [ ] **Step 5:** Final verification (superpowers:verification-before-completion): `make ci` once more on the pushed state; `git status` clean; issue states confirmed via `gh`.

---

## Self-review notes (run by the planner, already applied)

- **Spec coverage:** P1 §3.1–3.7 → Tasks 1–7; P2 §4.1–4.4 → Tasks 8–11; P3 §5.1–5.8 → Tasks 12–17; docs/issue §7–8 → Tasks 18–19. The `warn_if_remote_paths` stub stays (documented in Task 18).
- **Type consistency:** `ScheduledRule` fields used identically in Tasks 1, 3, 5, 7, 16; `ReentryRecord` defined in Task 13 and consumed by 14–17; `DriverOptions` extended in Task 16 — Task 5's initial tests must be updated there (noted inline).
- **Cross-session:** zero changes to `run_preview.rs` predicates (P1/P2); #77 has shipped so run.rs P3 edits are safe; `core/cluster.rs` untouched except nothing (Task 3 only calls it).
