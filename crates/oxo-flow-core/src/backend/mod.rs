//! Executor-agnostic static plan (issue #78): one plan, many executors.
//!
//! The engine's static parts (wildcard expansion, DAG, invalidation) stay
//! single-implementation; execution backends map the plan onto a scheduler
//! API without re-understanding the DAG.

use crate::config::WorkflowConfig;
use crate::dag::WorkflowDag;
use crate::environment::EnvironmentResolver;
use crate::error::{OxoFlowError, Result};
use crate::executor::process::build_execution_command;
use crate::rule::Rule;
use crate::scheduler::DispatchKeys;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub mod aws_batch;
pub mod cluster;
pub mod driver;
pub mod gcp_batch;
pub mod k8s;

/// Sanitize a free-form name into a DNS-1123 / RFC-1035 compatible label.
///
/// Shared by the Kubernetes and GCP Batch generators (both enforce a 63-char
/// lowercase alphanumeric-plus-dash label). Pass `max_len` to relax the cap
/// (AWS Batch allows 128) and `lowercase = false` to preserve case.
/// `extra_allowed` may contain additional characters that are passed through
/// verbatim (e.g. `'_'` for AWS Batch job names).
pub fn sanitize_cloud_name(
    name: &str,
    max_len: usize,
    lowercase: bool,
    extra_allowed: &str,
) -> String {
    let mut s = if lowercase {
        name.to_lowercase()
    } else {
        name.to_string()
    };
    s = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || extra_allowed.contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    s = s.trim_matches('-').to_string();
    if s.len() > max_len {
        s.truncate(max_len);
        s = s.trim_matches('-').to_string();
    }
    if s.is_empty() {
        s = "job".to_string();
    }
    s
}

/// Scheduler-facing resource sizing shared by the cloud job generators
/// (Kubernetes, AWS Batch, GCP Batch): at least one thread, and memory from
/// the rule's effective declaration with the scheduler-default 1024 MiB
/// fallback floored at 256 MiB.
pub fn scheduled_resources(rule: &Rule) -> (u32, u64) {
    (
        rule.resources.threads.max(1),
        crate::scheduler::effective_memory_mb(rule, 1024).max(256),
    )
}

/// Prefix the run's scratch-directory exports (#907) onto a cluster job
/// shell command (#1013 C2). Shared by the run driver
/// ([`ScheduledPlan::build_with_scratch`]) and the standalone
/// `cluster submit` script renderer so the two paths cannot drift — the
/// tx-ubuntu user-sim caught `cluster submit` scripts silently missing the
/// exports.
pub fn with_tmp_dir_exports(cmd: &str, tmp_dir: &Path) -> String {
    let quoted = crate::environment::escape_for_sh_single_quote(&tmp_dir.display().to_string());
    let exports = format!("export TMPDIR='{q}' TMP='{q}' TEMP='{q}' && ", q = quoted);
    match cmd.split_once(" && ") {
        // The run driver's shell_cmd opens with the `cd '<workdir>'` guard
        // from build_rule — splice after it so a failed cd still aborts
        // before the scratch redirection.
        Some((head, tail)) if head.starts_with("cd ") => {
            format!("{head} && {exports}{tail}")
        }
        // Standalone `cluster submit` rendering carries no guard yet (the
        // backend pins the working directory separately) — the exports lead
        // the command.
        _ => format!("{exports}{cmd}"),
    }
}

/// File name of the private env file an HPC job shell sources for routed
/// `sensitive = true` values (issue #1120 / ledger RB-12).
///
/// One name for the driver's run directory and `cluster submit`'s output
/// directory, so an operator (or a cleanup script) recognizes the artifact
/// on both paths.
pub const SENSITIVE_ENV_FILE_NAME: &str = "secrets.env";

/// Contents of the private env file that carries routed `sensitive = true`
/// values to an HPC job (issue #1120 / ledger RB-12).
///
/// `bindings` is the run's shared `VAR -> value` table
/// ([`crate::executor::process::sensitive_bindings`]); `refs` are the names a
/// job's commands actually reference, so an env file never carries a value
/// that job has no use for. `env_kind` mirrors the local routing: a
/// `singularity`/`apptainer` job also gets the `APPTAINERENV_`/
/// `SINGULARITYENV_` aliases its runtime reads from the CLI environment.
///
/// Values are single-quote escaped for POSIX `sh`; the file is sourced by the
/// job shell — never embedded in a scheduler script or a command line.
pub fn sensitive_env_file_contents(
    bindings: &[(String, String)],
    refs: &[String],
    env_kind: &str,
) -> String {
    let mut lines = String::new();
    for (var, value) in bindings {
        if !refs.iter().any(|used| used == var) {
            continue;
        }
        let quoted = crate::environment::escape_for_sh_single_quote(value);
        lines.push_str(&format!("export {var}='{quoted}'\n"));
        for alias in crate::executor::process::sensitive_env_aliases(var, env_kind) {
            lines.push_str(&format!("export {alias}='{quoted}'\n"));
        }
    }
    lines
}

/// Write [`sensitive_env_file_contents`] to `path` with `0o600` permissions.
///
/// Shared by the cluster driver (run directory) and `cluster submit`
/// (operator output directory) so both write the same file with the same
/// mode — a hand-rolled writer on either path would drift.
pub fn write_sensitive_env_file(
    path: &Path,
    bindings: &[(String, String)],
    refs: &[String],
    env_kind: &str,
) -> Result<()> {
    let contents = sensitive_env_file_contents(bindings, refs, env_kind);
    write_private(path, contents.as_bytes())
}

/// The operator's secret-reference mapping for cloud job bodies
/// (`OXO_FLOW_SECRET_REFS`, issue #1120 / ledger RB-12).
///
/// GCP Batch resolves a routed `OXO_FLOW_SENSITIVE_<n>` through
/// `taskSpec.environment.secretVariables`, whose value must be a Secret
/// Manager resource path — the engine cannot invent the project, so the
/// operator states it here. Syntax: `NAME=REFERENCE` pairs separated by `;`
/// (`,` and newlines also split). AWS Batch's `SubmitJob` has no secrets
/// field, so an AWS operator maps the same names on the job definition
/// instead; the map is ignored for that backend.
pub fn secret_refs_from_env() -> std::collections::BTreeMap<String, String> {
    secret_refs_from(std::env::var("OXO_FLOW_SECRET_REFS").ok().as_deref())
}

/// Parse the `OXO_FLOW_SECRET_REFS` syntax (see [`secret_refs_from_env`]).
///
/// A malformed pair is skipped with a warning: guessing a reference for a
/// secret would route it somewhere the operator did not name.
pub fn secret_refs_from(raw: Option<&str>) -> std::collections::BTreeMap<String, String> {
    let mut refs = std::collections::BTreeMap::new();
    let Some(raw) = raw else {
        return refs;
    };
    for entry in raw.split([';', ',', '\n']) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((name, reference)) = entry.split_once('=') else {
            tracing::warn!(
                entry,
                "ignoring OXO_FLOW_SECRET_REFS entry without '=' — expected NAME=REFERENCE"
            );
            continue;
        };
        let (name, reference) = (name.trim(), reference.trim());
        let valid_name = !name.is_empty()
            && name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid_name || reference.is_empty() {
            tracing::warn!(
                entry,
                "ignoring malformed OXO_FLOW_SECRET_REFS entry — expected NAME=REFERENCE"
            );
            continue;
        }
        refs.insert(name.to_string(), reference.to_string());
    }
    refs
}

/// The shell fragment that loads a private secret env file: POSIX `.`
/// (source) with the path shell-quoted. Prepended to an HPC job command
/// *inside* the `env -i` isolation, so the isolated shell exports the routed
/// values itself and the allowlist never has to carry them.
pub fn source_sensitive_env_file(path: &Path) -> String {
    format!(
        ". '{}'",
        crate::environment::escape_for_sh_single_quote(&path.display().to_string())
    )
}

/// Write `contents` to `path` with `0o600` permissions (owner read/write).
///
/// The run directory holds scheduler scripts, job ids, status records and —
/// for HPC backends — the private secret env file. Everything the driver
/// writes goes through this one writer so the permission can never differ
/// per artifact (issue #136, RB-12).
pub(crate) fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| OxoFlowError::Config {
        message: format!("cannot open {}: {e}", path.display()),
    })?;
    file.write_all(contents).map_err(|e| OxoFlowError::Config {
        message: format!("cannot write {}: {e}", path.display()),
    })?;
    file.sync_all().map_err(|e| OxoFlowError::Config {
        message: format!("cannot sync {}: {e}", path.display()),
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| {
            OxoFlowError::Config {
                message: format!("cannot set private permissions on {}: {e}", path.display()),
            }
        })?;
    }
    Ok(())
}

/// Scheduler-agnostic job state, mapped from each backend's native codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendJobStatus {
    /// Queued, waiting to start.
    Pending,
    /// Currently executing.
    Running,
    /// Finished successfully.
    Completed,
    /// Finished with an error.
    Failed,
    /// Cancelled by the driver or a user.
    Cancelled,
    /// Status could not be determined.
    #[default]
    Unknown,
}

impl BackendJobStatus {
    /// Lower-case name for machine-readable output (the `cluster status`
    /// table) — the same vocabulary the driver writes into `events.jsonl`.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }
}

/// What a scheduler's accounting store knows about a job that has left the
/// live queue: the terminal state, plus whatever the store measured.
///
/// Everything but `status` is optional on purpose. Accounting is a per-site
/// deployment choice — a SLURM cluster without `slurmdbd` answers `sacct`
/// with nothing, and PBS/SGE/LSF each expose a different subset — so a
/// backend reports what it can prove and leaves the rest `None` rather than
/// inventing a number that would land in a checkpoint as fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TerminalRecord {
    /// Terminal state, mapped from the store's native vocabulary.
    pub status: BackendJobStatus,
    /// Process exit code. A job killed by a signal reports the shell's
    /// `128 + signum` rather than the exit component, which is `0` for a
    /// signalled job and would otherwise read as success.
    pub exit_code: Option<i32>,
    /// Wall-clock seconds the job spent RUNNING, excluding queue wait.
    /// Separating the two is the whole point of reading accounting: the
    /// driver only ever observes submit-to-settle, which on a busy queue is
    /// mostly waiting.
    pub elapsed_secs: Option<u64>,
    /// Peak resident set size over every step of the job, in MB.
    pub max_rss_mb: Option<u64>,
    /// Total CPU seconds consumed across all cores.
    pub cpu_seconds: Option<u64>,
}

impl TerminalRecord {
    /// A record from a store that reports state and nothing else.
    pub fn status_only(status: BackendJobStatus) -> Self {
        Self {
            status,
            ..Self::default()
        }
    }
}

/// Maps a static plan onto a scheduler API.
///
/// Implementations must not re-derive execution order or invalidation: they
/// receive fully resolved [`ScheduledRule`]s and translate them into
/// submissions. Script-based schedulers (SLURM/PBS/SGE/LSF) get one submit
/// call per rendered script; the driver composes fragments (waves).
#[async_trait::async_trait]
pub trait ExecutorBackend: Send + Sync {
    /// Human-readable backend name for logging / diagnostics.
    fn name(&self) -> &'static str;

    /// Render a scheduler submit script for one scheduled rule.
    fn render_script(&self, rule: &ScheduledRule) -> Result<String>;

    /// True when this backend's job artifacts must receive routed
    /// `sensitive = true` values through a private (`0o600`) env file the job
    /// shell sources (issue #1120 / ledger RB-12).
    ///
    /// Script-based HPC schedulers copy the script into scheduler-visible
    /// storage, so the literal can never be part of it; the env file lives on
    /// the run directory's shared filesystem and is removed when the driver
    /// observes the job terminal. Cloud backends return `false` — their bodies
    /// carry an `OXO_FLOW_SENSITIVE_<n>` reference that resolves through the
    /// backend's own secret channel instead.
    fn uses_secret_env_file(&self) -> bool {
        false
    }

    /// Submit one rendered script; returns the scheduler-assigned job id.
    async fn submit(&self, script_path: &Path) -> Result<String>;

    /// Poll statuses for the given job ids; missing ids are simply absent.
    async fn poll(&self, job_ids: &[String]) -> Result<HashMap<String, BackendJobStatus>>;

    /// Cancel a submitted job.
    async fn cancel(&self, job_id: &str) -> Result<()>;

    /// Render an ARRAY submit script for `count` same-rule instances whose
    /// per-index commands live in `cmd_dir` (issue #74 phase 3). Backends
    /// without array support return an error — the driver then falls back
    /// to per-job submission via the chunk-of-one path.
    fn render_array_script(
        &self,
        _rule: &crate::rule::Rule,
        _cmd_dir: &str,
        _count: usize,
        _sensitive_refs: &[String],
    ) -> Result<String> {
        Err(OxoFlowError::Config {
            message: format!("backend '{}' does not support job arrays", self.name()),
        })
    }

    /// Fetch job logs / accounting output (raw text).
    async fn logs(&self, job_id: &str) -> Result<String>;

    /// Terminal record for a job that has left the live queue, read from the
    /// scheduler's accounting store. `None` means "no terminal record" —
    /// the driver keeps the job in flight. Backends without an accounting
    /// store keep the default and rely on their poller.
    async fn terminal_status(&self, _job_id: &str) -> Option<TerminalRecord> {
        None
    }

    /// True when `poll` can answer for individual ARRAY ELEMENTS by the ids
    /// [`array_element_id`](Self::array_element_id) mints — SLURM's squeue
    /// lists `{jobid}_{index}`, a Kubernetes Indexed Job reports its
    /// completed/failed indexes, and AWS Batch / GCP Batch expose one child
    /// job or task per element. The driver then settles each element on its
    /// own verdict.
    ///
    /// Schedulers this adapter only ever queries by the array's base id
    /// (PBS/LSF/SGE, issue #136 H4) return false — the driver polls and
    /// cancels by base id, expands the aggregate verdict to every element,
    /// and marks each record with that provenance instead of presenting the
    /// aggregate as an independently observed outcome (issue #1132 / RB-06).
    fn polls_elements_directly(&self) -> bool {
        false
    }

    /// The scheduler id of one array element, derived from the id qsub
    /// echoed for the array submission. The default is SLURM's
    /// `{base}_{index}`; OpenPBS prints the array id as `38[]` and
    /// addresses elements as `38[2]`, so that base swaps the brackets
    /// instead (found live, issue #356).
    fn array_element_id(&self, base: &str, index: usize) -> String {
        format!("{}_{}", base, index)
    }
}

/// One executable unit of the static plan: a fully resolved rule instance.
#[derive(Debug, Clone)]
pub struct ScheduledRule {
    /// Expanded rule instance (post `expand_wildcards`): the single source
    /// for script rendering and resource directives.
    pub rule: Rule,
    /// Template rule name this instance was expanded from (issue #74 phase
    /// 3) — the array-grouping key. Equals the instance name for rules
    /// that never fanned out.
    pub template: String,
    /// Environment-wrapped command with `cd <workdir>` folded in.
    pub shell_cmd: String,
    /// Working directory the command runs in.
    pub workdir: PathBuf,
    /// Instance-level dependencies (DAG edges + resolved `depends_on`).
    pub dependencies: Vec<String>,
    /// `{config.x}`-style bindings for late path expansion.
    pub wildcard_values: HashMap<String, String>,
}

/// The executor-agnostic static plan: topological order, parallel groups,
/// and one [`ScheduledRule`] per schedulable instance.
#[derive(Debug, Clone)]
pub struct ScheduledPlan {
    pub order: Vec<String>,
    pub groups: Vec<Vec<String>>,
    pub rules: HashMap<String, ScheduledRule>,
    /// Ready-rule tie-break policy and per-instance keys (`[engine]
    /// dispatch_order`, issue #1089). The default keeps the historical
    /// name ordering, so plans built outside [`ScheduledPlan::build`] are
    /// unaffected.
    pub dispatch: DispatchKeys,
}

impl ScheduledPlan {
    /// Build the plan from an expanded config + DAG.
    pub fn build(
        config: &WorkflowConfig,
        dag: &WorkflowDag,
        workdir: &Path,
        env_resolver: &EnvironmentResolver,
        wildcard_values: &HashMap<String, String>,
    ) -> Result<Self> {
        Self::build_with_sensitive(config, dag, workdir, env_resolver, wildcard_values, &[])
    }

    /// [`Self::build`] with `sensitive = true` config values routed out of
    /// every rule command BEFORE the environment wrapper is built (issue
    /// #1120 / ledger RB-12).
    ///
    /// Ordering is load-bearing: the local executor routes the raw command
    /// first ([`crate::executor::process`]), so the docker/singularity
    /// wrappers see the `OXO_FLOW_SENSITIVE_<n>` reference and forward the
    /// value by name (`docker run -e NAME`, `SINGULARITYENV_NAME`) instead of
    /// re-expanding the literal into the container argv. Routing the wrapped
    /// text afterwards left the cluster path with the pre-#1176 leak: the
    /// value was re-materialized on the compute node. One binding table per
    /// plan keeps every rule and array element on the same variable names.
    pub fn build_with_sensitive(
        config: &WorkflowConfig,
        dag: &WorkflowDag,
        workdir: &Path,
        env_resolver: &EnvironmentResolver,
        wildcard_values: &HashMap<String, String>,
        sensitive_values: &[String],
    ) -> Result<Self> {
        let order = dag.execution_order()?;
        let groups = dag.parallel_groups()?;
        let bindings = crate::executor::process::sensitive_bindings(sensitive_values);
        let mut rules = HashMap::new();
        for name in &order {
            if let Some(sr) = build_rule(
                name,
                config,
                dag,
                workdir,
                env_resolver,
                wildcard_values,
                &bindings,
            )? {
                rules.insert(name.clone(), sr);
            }
        }
        Ok(Self {
            order,
            groups,
            rules,
            dispatch: DispatchKeys::from_config(config),
        })
    }

    /// Build the plan with the run's scratch directory exported into every
    /// job shell (`[engine] tmp_dir` / `OXO_FLOW_TMPDIR`, issues #907 +
    /// #1013 C2). Cluster jobs do not inherit the submit host's `TMPDIR`
    /// (the generated job script admits only the env allowlist — issue
    /// #1108, RB-15), so the scratch exports must be part of the rendered
    /// command itself — placed after the `cd` guard and OUTSIDE the
    /// environment wrapper so wrapped child processes (conda/pixi/venv
    /// run, containers with host mounts) inherit `TMPDIR`. The local
    /// executor instead injects `TMPDIR`/`TMP`/`TEMP` into the process
    /// env directly (`insert_tmpdir_vars`) and must keep using
    /// [`ScheduledPlan::build`].
    pub fn build_with_scratch(
        config: &WorkflowConfig,
        dag: &WorkflowDag,
        workdir: &Path,
        env_resolver: &EnvironmentResolver,
        wildcard_values: &HashMap<String, String>,
        tmp_dir: Option<&Path>,
    ) -> Result<Self> {
        Self::build_with_scratch_and_sensitive(
            config,
            dag,
            workdir,
            env_resolver,
            wildcard_values,
            tmp_dir,
            &[],
        )
    }

    /// [`Self::build_with_scratch`] plus pre-wrapper sensitive-value routing
    /// (issue #1120 / ledger RB-12) — the production cluster-run entry point.
    pub fn build_with_scratch_and_sensitive(
        config: &WorkflowConfig,
        dag: &WorkflowDag,
        workdir: &Path,
        env_resolver: &EnvironmentResolver,
        wildcard_values: &HashMap<String, String>,
        tmp_dir: Option<&Path>,
        sensitive_values: &[String],
    ) -> Result<Self> {
        let mut plan = Self::build_with_sensitive(
            config,
            dag,
            workdir,
            env_resolver,
            wildcard_values,
            sensitive_values,
        )?;
        if let Some(tmp) = tmp_dir {
            for rule in plan.rules.values_mut() {
                rule.shell_cmd = with_tmp_dir_exports(&rule.shell_cmd, tmp);
            }
        }
        Ok(plan)
    }

    /// Route `sensitive = true` config values out of every already-wrapped
    /// job shell (issue #1120 / ledger RB-12).
    ///
    /// This is the post-wrapper safety net for plans built by
    /// [`Self::build`]: the production cluster path pre-routes the raw
    /// command through [`Self::build_with_sensitive`], which is the only
    /// ordering that lets the docker/singularity wrappers forward the value
    /// by name. Calling this on a plan that has no literal left is a no-op;
    /// on a plan that still carries one, the reference replaces it before any
    /// manifest/spec is rendered.
    ///
    /// Returns the routed `VAR -> value` bindings, sorted key order, for
    /// callers that must hand the values to an actual injection channel.
    pub fn route_sensitive_values(
        &mut self,
        values: &[String],
    ) -> std::collections::BTreeMap<String, String> {
        let mut env = std::collections::BTreeMap::new();
        if values.is_empty() {
            return env;
        }
        // One binding table for the WHOLE plan: every instance of every rule
        // must agree on which `OXO_FLOW_SENSITIVE_<n>` name carries which
        // value — cloud backends project those names into a secret channel
        // shared by every element of an array job.
        let bindings = crate::executor::process::sensitive_bindings(values);
        for sr in self.rules.values_mut() {
            let (cmd, refs) = crate::executor::process::route_sensitive_values_with_bindings(
                &sr.shell_cmd,
                &bindings,
            );
            sr.shell_cmd = cmd;
            env.extend(refs);
        }
        env
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
            // A merge hook belongs to a caller that also owns the sensitive
            // value list; checkpoint re-entry on the cluster path passes no
            // merge hook (`run_cluster` sets `merge: None`), so re-expanded
            // instances keep the unrouted text until a caller supplies the
            // values — never silently route with a different table.
            if let Some(sr) = build_rule(
                name,
                config,
                dag,
                workdir,
                env_resolver,
                wildcard_values,
                &[],
            )? {
                self.rules.insert(name.clone(), sr);
                self.order.push(name.clone());
            }
        }
        self.groups = dag.parallel_groups()?;
        // Rebuild the dispatch keys: checkpoint re-entry can create new
        // instances, and their expansion provenance is only known now.
        self.dispatch = DispatchKeys::from_config(config);
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
    sensitive_bindings: &[(String, String)],
) -> Result<Option<ScheduledRule>> {
    let rule = config.get_rule(name).ok_or_else(|| OxoFlowError::Config {
        message: format!("rule '{name}' not found in workflow"),
    })?;
    let Some(cmd) = build_execution_command(
        rule,
        wildcard_values,
        &config.workflow.interpreter_map,
        crate::scheduler::detect_system_limits(),
    )?
    else {
        return Ok(None); // no shell/script — not schedulable
    };
    // The prelude goes INSIDE the environment wrapper, matching the local
    // executor (issue #92): prepending outside would leave the wrapped
    // command (conda run / docker run bash -c) without fail-fast semantics.
    let cmd = config.defaults.apply_shell_prelude(&cmd);
    // Issue #1177/#1120: container backends discover their automatic host
    // bind mounts from the PRE-routing text, and the wrapper itself must see
    // the routed reference so docker forwards by name (`-e NAME`) instead of
    // re-expanding the literal on the compute node — the same ordering the
    // local executor uses.
    let external_mounts = if rule.environment.kind() == "docker" && !sensitive_bindings.is_empty() {
        Some(crate::environment::external_bind_mounts_for(
            &cmd,
            workdir,
            sensitive_bindings,
        ))
    } else {
        None
    };
    let cmd = if sensitive_bindings.is_empty() {
        cmd
    } else {
        crate::executor::process::route_sensitive_values_with_bindings(&cmd, sensitive_bindings).0
    };
    let wrapped = env_resolver
        .wrap_command_with_external_mounts(
            &cmd,
            &rule.environment,
            Some(&rule.resources),
            workdir,
            external_mounts.as_deref(),
        )
        .map_err(|e| OxoFlowError::Config {
            message: format!("environment wrapping failed for '{name}': {e}"),
        })?;
    let dependencies = dag.dependencies(name).unwrap_or_default();
    // #534: the run dir is a host path — quote it like environment.rs
    // quotes the identical path elsewhere (a `'` in the path broke out of
    // the quoting and injected).
    let quoted_workdir =
        crate::environment::escape_for_sh_single_quote(&workdir.display().to_string());
    let shell_cmd = format!("cd '{quoted_workdir}' && {wrapped}");
    let template = config
        .template_of(name)
        .map(str::to_string)
        .unwrap_or_else(|| name.to_string());
    Ok(Some(ScheduledRule {
        rule: rule.clone(),
        template,
        shell_cmd,
        workdir: workdir.to_path_buf(),
        dependencies,
        wildcard_values: wildcard_values.clone(),
    }))
}

/// Re-exec the test binary with sentinel ambient credentials exported and
/// assert that `backend`'s generated job bodies carry none of them (issue
/// #1171).
///
/// The cloud generators build their body from the rule and the rendered
/// command only — the job's environment is the image ENV plus what the
/// engine writes into the manifest, never the submit host's ambient
/// environment. This probe pins that boundary per backend.
///
/// A child process is required because `std::env::set_var` is unsafe in
/// edition 2024 and this crate is `#![forbid(unsafe_code)]`, so the test
/// cannot inject the sentinels into its own environment. The child is
/// re-executed with `--ignored --exact` and runs
/// [`tests::submit_host_ambient_probe_child`].
#[cfg(test)]
pub(crate) fn assert_cloud_body_ignores_submit_host_env(backend: &str) {
    let exe = std::env::current_exe().expect("test binary path");
    let output = std::process::Command::new(exe)
        .args([
            "--exact",
            "backend::tests::submit_host_ambient_probe_child",
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .env("PROBE_CLOUD_BACKEND", backend)
        .env("SENTINEL_AMBIENT", "leak-canary")
        .env("ANTHROPIC_AUTH_TOKEN", "leak-token")
        .env("OXO_FLOW_MASTER_KEY", "leak-key")
        .output()
        .expect("failed to re-exec the test binary for the ambient-leak probe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "the {backend} job body leaked submit-host ambient state\n\
         --- child stdout ---\n{stdout}\n--- child stderr ---\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    // A filter that matches no test exits 0, so the handshake is what proves
    // the probe actually ran (a renamed child must not make this vacuous).
    assert!(
        stdout.contains(&format!("CLOUD_AMBIENT_PROBE_OK {backend}")),
        "the ambient-leak probe child did not run for {backend}\n--- child stdout ---\n{stdout}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkflowConfig;
    use crate::dag::WorkflowDag;
    use crate::environment::EnvironmentResolver;
    use std::collections::HashMap;

    fn demo_config(dir: &std::path::Path) -> WorkflowConfig {
        let toml = r#"
            [workflow]
            name = "demo"
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
        let path = dir.join("wf.oxoflow");
        std::fs::write(&path, toml).unwrap();
        WorkflowConfig::from_file(&path).unwrap()
    }

    fn values() -> HashMap<String, String> {
        HashMap::from([("config.ref".to_string(), "/data/ref.fa".to_string())])
    }

    #[test]
    fn build_prepends_shell_prelude_before_cd_guard() {
        // issue #92: the cluster plan applies the prelude to the WHOLE
        // script — before the cd guard — so set -e also aborts on a failed
        // cd, and the command line stays guarded.
        let dir = tempfile::tempdir().unwrap();
        let mut config = demo_config(dir.path());
        config.defaults.shell_prelude = Some("set -euo pipefail".to_string());
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let plan = ScheduledPlan::build(
            &config,
            &dag,
            std::path::Path::new("/tmp/wf"),
            &EnvironmentResolver::new(),
            &values(),
        )
        .unwrap();
        let r = &plan.rules["preprocess"];
        assert!(
            r.shell_cmd
                .starts_with("cd '/tmp/wf' && set -euo pipefail\n"),
            "prelude must sit inside the wrapped command, after the cd guard: {}",
            r.shell_cmd
        );
    }

    #[test]
    fn build_with_scratch_exports_tmpdir_after_cd_guard() {
        // issue #1013 C2: cluster job shells start from the scheduler's
        // clean environment, so the run's scratch directory (issue #907)
        // must be exported into every job script — after the cd guard and
        // OUTSIDE the environment wrapper, so wrapped child processes
        // (conda/pixi/venv run) inherit TMPDIR. `build` (no scratch) must
        // stay byte-identical to the pre-#1013 output.
        let dir = tempfile::tempdir().unwrap();
        let mut config = demo_config(dir.path());
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let plan = ScheduledPlan::build_with_scratch(
            &config,
            &dag,
            std::path::Path::new("/tmp/wf"),
            &EnvironmentResolver::new(),
            &values(),
            Some(std::path::Path::new("/scratch/oxo")),
        )
        .unwrap();
        let r = &plan.rules["preprocess"];
        assert!(
            r.shell_cmd.starts_with(
                "cd '/tmp/wf' && export TMPDIR='/scratch/oxo' TMP='/scratch/oxo' \
                 TEMP='/scratch/oxo' && "
            ),
            "exports must follow the cd guard and precede the wrapped command: {}",
            r.shell_cmd
        );
        let plain = ScheduledPlan::build(
            &config,
            &dag,
            std::path::Path::new("/tmp/wf"),
            &EnvironmentResolver::new(),
            &values(),
        )
        .unwrap();
        assert!(
            !plain.rules["preprocess"].shell_cmd.contains("TMPDIR"),
            "no-scratch build must leave the ambient environment untouched: {}",
            plain.rules["preprocess"].shell_cmd
        );
    }

    #[test]
    fn with_tmp_dir_exports_leads_unchanged_commands() {
        // The standalone `cluster submit` renderer carries no `cd` guard
        // (the backend pins the working directory separately), so the
        // exports lead the command — and a command whose own body contains
        // `&&` must never have the exports spliced into the middle of it.
        let out = with_tmp_dir_exports(
            "conda run python train.py --seed 1 && python eval.py",
            std::path::Path::new("/scratch/oxo"),
        );
        assert!(
            out.starts_with(
                "export TMPDIR='/scratch/oxo' TMP='/scratch/oxo' TEMP='/scratch/oxo' && "
            ),
            "exports must lead a command without a cd guard: {out}"
        );
        assert!(
            out.ends_with("conda run python train.py --seed 1 && python eval.py"),
            "the command body must survive verbatim: {out}"
        );
        // A guarded command keeps the splice after the cd.
        let guarded = with_tmp_dir_exports("cd '/wf' && make", std::path::Path::new("/s"));
        assert_eq!(
            guarded,
            "cd '/wf' && export TMPDIR='/s' TMP='/s' TEMP='/s' && make"
        );
    }

    #[test]
    fn build_produces_order_groups_and_deps() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = demo_config(dir.path());
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let plan = ScheduledPlan::build(
            &config,
            &dag,
            std::path::Path::new("/tmp/wf"),
            &EnvironmentResolver::new(),
            &values(),
        )
        .unwrap();
        assert_eq!(plan.order, dag.execution_order().unwrap());
        assert_eq!(plan.groups, dag.parallel_groups().unwrap());
        // analyze instances depend on preprocess
        for name in ["analyze_batch_S1", "analyze_batch_S2"] {
            let r = &plan.rules[name];
            assert_eq!(r.dependencies, vec!["preprocess".to_string()]);
            assert!(r.shell_cmd.contains("cd '/tmp/wf'"), "{}", r.shell_cmd);
            assert!(
                r.shell_cmd.contains("out/S1.txt") || r.shell_cmd.contains("out/S2.txt"),
                "{}",
                r.shell_cmd
            );
        }
    }

    #[test]
    fn build_skips_rules_without_shell() {
        let dir = tempfile::tempdir().unwrap();
        let toml = r#"
            [workflow]
            name = "noop-wf"
            [[rules]]
            name = "noop"
            input = ["x.txt"]
        "#;
        let path = dir.path().join("wf.oxoflow");
        std::fs::write(&path, toml).unwrap();
        let mut config = WorkflowConfig::from_file(&path).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let plan = ScheduledPlan::build(
            &config,
            &dag,
            std::path::Path::new("/tmp/wf"),
            &EnvironmentResolver::new(),
            &HashMap::new(),
        )
        .unwrap();
        assert!(plan.rules.is_empty());
        assert_eq!(plan.order, vec!["noop".to_string()]);
    }

    /// Cloud job bodies are rendered from `shell_cmd`, and an array job's
    /// elements share one secret channel — so the routing table must be
    /// shared across the WHOLE plan. Two rules that use different subsets of
    /// the declared values still agree on every variable name (issue #1120 /
    /// RB-12; review D4).
    #[test]
    fn plan_sensitive_routing_shares_one_binding_table_across_rules() {
        let mk = |cmd: &str| ScheduledRule {
            rule: crate::rule::Rule::default(),
            template: "t".to_string(),
            shell_cmd: cmd.to_string(),
            workdir: std::path::PathBuf::from("/tmp/wf"),
            dependencies: Vec::new(),
            wildcard_values: HashMap::new(),
        };
        let mut plan = ScheduledPlan {
            order: vec!["a".to_string(), "b".to_string()],
            groups: Vec::new(),
            rules: HashMap::from([
                // Rule `a` uses only the shorter value; rule `b` only the
                // longer one. Per-command numbering would bind index 0 to
                // two different values.
                ("a".to_string(), mk("tool --short SHORTVALUE-7")),
                ("b".to_string(), mk("tool --long A-MUCH-LONGER-VALUE-9")),
            ]),
            dispatch: crate::scheduler::DispatchKeys::default(),
        };

        let env = plan.route_sensitive_values(&[
            "SHORTVALUE-7".to_string(),
            "A-MUCH-LONGER-VALUE-9".to_string(),
        ]);

        // Longest-first binding: index 0 = the longer value.
        assert_eq!(
            plan.rules["a"].shell_cmd,
            "tool --short ${OXO_FLOW_SENSITIVE_1}"
        );
        assert_eq!(
            plan.rules["b"].shell_cmd,
            "tool --long ${OXO_FLOW_SENSITIVE_0}"
        );
        assert_eq!(env.len(), 2);
        assert_eq!(env["OXO_FLOW_SENSITIVE_0"], "A-MUCH-LONGER-VALUE-9");
        assert_eq!(env["OXO_FLOW_SENSITIVE_1"], "SHORTVALUE-7");
        // No literal survives in either command.
        for rule in plan.rules.values() {
            assert!(!rule.shell_cmd.contains("SHORTVALUE-7"));
            assert!(!rule.shell_cmd.contains("A-MUCH-LONGER-VALUE-9"));
        }
    }

    /// Build a one-rule plan whose command embeds a `sensitive = true`
    /// config value, routing it before the environment wrapper (issue #1120
    /// / RB-12).
    fn sensitive_plan(
        workdir: &std::path::Path,
        sentinel: &str,
        shell: &str,
        environment: &str,
    ) -> ScheduledPlan {
        let toml =
            String::from("[workflow]\nname = \"secrets\"\n[[rules]]\nname = \"leak_probe\"\n")
                + &format!("shell = \"{shell}\"\noutput = [\"out.txt\"]\n")
                + environment;
        let path = workdir.join("wf.oxoflow");
        std::fs::write(&path, toml).unwrap();
        let mut config = WorkflowConfig::from_file(&path).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        ScheduledPlan::build_with_sensitive(
            &config,
            &dag,
            workdir,
            &EnvironmentResolver::new(),
            &HashMap::from([("config.api_token".to_string(), sentinel.to_string())]),
            &[sentinel.to_string()],
        )
        .unwrap()
    }

    /// Routing must happen BEFORE the environment wrapper is built: the
    /// docker wrapper then forwards the routed name (`-e NAME`) instead of
    /// re-expanding the literal into the container argv on the compute node.
    /// Routing the wrapped text (the pre-#1120 cluster ordering) leaves the
    /// literal in `shell_cmd`'s container command — this fails on that
    /// behavior.
    #[test]
    fn build_with_sensitive_routes_before_the_container_wrapper() {
        let sentinel = "SENTINEL_SECRET_VALUE_C3X";
        let dir = tempfile::tempdir().unwrap();
        let plan = sensitive_plan(
            dir.path(),
            sentinel,
            "curl -H 'Authorization: {config.api_token}' https://example.invalid",
            "environment = { docker = \"ubuntu:24.04\" }\n",
        );
        let cmd = &plan.rules["leak_probe"].shell_cmd;
        assert!(
            !cmd.contains(sentinel),
            "the sentinel must not survive routing: {cmd}"
        );
        assert!(
            cmd.contains("-e OXO_FLOW_SENSITIVE_0"),
            "the docker wrapper must forward the routed var by name: {cmd}"
        );
        assert!(
            cmd.contains("${OXO_FLOW_SENSITIVE_0}"),
            "the command must reference the routed var: {cmd}"
        );

        // Contrast: the pre-routing build path still carries the literal
        // (the documented historical behavior; callers that need routing
        // use the `_with_sensitive` entry points).
        let toml = std::fs::read_to_string(dir.path().join("wf.oxoflow")).unwrap();
        let wf = dir.path().join("plain.oxoflow");
        std::fs::write(&wf, toml).unwrap();
        let mut config = WorkflowConfig::from_file(&wf).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let plain = ScheduledPlan::build(
            &config,
            &dag,
            dir.path(),
            &EnvironmentResolver::new(),
            &HashMap::from([("config.api_token".to_string(), sentinel.to_string())]),
        )
        .unwrap();
        assert!(
            plain.rules["leak_probe"].shell_cmd.contains(sentinel),
            "build() is the unrouted path; the regression contrast is real"
        );
    }

    #[test]
    fn sensitive_env_file_carries_only_used_refs_and_quotes_values() {
        let bindings = crate::executor::process::sensitive_bindings(&[
            "SENTINEL_USED_VALUE_A1".to_string(),
            "SENTINEL_UNUSED_VALUE_B2".to_string(),
        ]);
        assert_eq!(bindings.len(), 2);
        // Numbering is longest-first, so look the name up by its value.
        let used = bindings
            .iter()
            .find(|(_, value)| value == "SENTINEL_USED_VALUE_A1")
            .expect("binding exists")
            .0
            .clone();
        let contents =
            sensitive_env_file_contents(&bindings, std::slice::from_ref(&used), "system");
        assert!(
            contents.contains(&format!("export {used}='SENTINEL_USED_VALUE_A1'")),
            "the used ref must be exported: {contents}"
        );
        assert!(
            !contents.contains("SENTINEL_UNUSED_VALUE_B2"),
            "an unused value must never enter the env file: {contents}"
        );

        // POSIX single-quote escaping: a quote in the value must not break
        // out of the assignment.
        let hostile = "it's-a-secret";
        let bindings = crate::executor::process::sensitive_bindings(&[hostile.to_string()]);
        let contents = sensitive_env_file_contents(&bindings, &[bindings[0].0.clone()], "system");
        assert!(
            contents.contains(r#"'it'\''s-a-secret'"#),
            "the value must be safely single-quoted: {contents}"
        );
    }

    /// Singularity/apptainer read the container-side value from the
    /// `*ENV_` aliases in the CLI environment — the cluster env file must
    /// spell exactly the aliases the local spawn path exports.
    #[test]
    fn sensitive_env_file_adds_singularity_aliases_only_for_singularity() {
        let bindings =
            crate::executor::process::sensitive_bindings(&["SENTINEL_VALUE_S9".to_string()]);
        let refs = vec![bindings[0].0.clone()];
        let singularity = sensitive_env_file_contents(&bindings, &refs, "singularity");
        assert!(
            singularity.contains("export APPTAINERENV_OXO_FLOW_SENSITIVE_0='SENTINEL_VALUE_S9'")
        );
        assert!(
            singularity.contains("export SINGULARITYENV_OXO_FLOW_SENSITIVE_0='SENTINEL_VALUE_S9'")
        );

        let docker = sensitive_env_file_contents(&bindings, &refs, "docker");
        assert!(!docker.contains("SINGULARITYENV_"));
        assert!(!docker.contains("APPTAINERENV_"));
    }

    #[cfg(unix)]
    #[test]
    fn write_sensitive_env_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SENSITIVE_ENV_FILE_NAME);
        let bindings =
            crate::executor::process::sensitive_bindings(&["SENTINEL_MODE_VALUE_M4".to_string()]);
        write_sensitive_env_file(&path, &bindings, &[bindings[0].0.clone()], "system").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the secret env file must stay owner-only");
    }

    #[test]
    fn secret_refs_env_parses_name_reference_pairs_and_skips_malformed() {
        let refs = secret_refs_from(Some(
            "OXO_FLOW_SENSITIVE_0=projects/p/secrets/s/versions/latest;\
             OXO_FLOW_SENSITIVE_1 = projects/p/secrets/t/versions/3\n\
             broken-entry, =missing-name, OXO_FLOW_SENSITIVE_2=",
        ));
        assert_eq!(
            refs.get("OXO_FLOW_SENSITIVE_0").map(String::as_str),
            Some("projects/p/secrets/s/versions/latest")
        );
        assert_eq!(
            refs.get("OXO_FLOW_SENSITIVE_1").map(String::as_str),
            Some("projects/p/secrets/t/versions/3")
        );
        assert!(!refs.contains_key("broken-entry"));
        assert!(!refs.contains_key("OXO_FLOW_SENSITIVE_2"));
        assert!(secret_refs_from(None).is_empty());
    }

    /// Whether an `env` entry is the engine's own routed sensitive
    /// reference (issue #1120 / #1176): the ONLY legitimate `env` channel a
    /// generated cloud body may carry. Deployment credentials never look
    /// like this.
    fn is_routed_secret_env_entry(entry: &serde_json::Value) -> bool {
        let Some(name) = entry.get("name").and_then(|v| v.as_str()) else {
            return false;
        };
        let Some(suffix) = name.strip_prefix(crate::executor::process::SENSITIVE_ENV_BASE) else {
            return false;
        };
        let suffix = suffix.strip_prefix('_').unwrap_or("");
        !suffix.is_empty()
            && suffix.bytes().all(|b| b.is_ascii_digit())
            && entry.pointer("/valueFrom/secretKeyRef").is_some()
    }

    /// The first environment channel in `value` that is NOT an engine-routed
    /// sensitive reference, if any. A cloud job's environment is the image
    /// ENV plus what the engine writes — and the engine writes no submit-host
    /// state (issue #1171); the one channel it does write is the
    /// `OXO_FLOW_SENSITIVE_<n>` secret reference.
    fn env_channel_violation(value: &serde_json::Value) -> Option<String> {
        match value {
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    if key == "env" {
                        let routed = child.as_array().is_some_and(|items| {
                            !items.is_empty() && items.iter().all(is_routed_secret_env_entry)
                        });
                        if !routed {
                            return Some(format!(
                                "'env' carries something other than OXO_FLOW_SENSITIVE_<n> secretKeyRef entries: {child}"
                            ));
                        }
                        continue;
                    }
                    if key == "environment" {
                        if is_gcp_secret_variables_environment(child) {
                            continue;
                        }
                        return Some(format!("an 'environment' channel: {child}"));
                    }
                    if let Some(violation) = env_channel_violation(child) {
                        return Some(violation);
                    }
                }
                None
            }
            serde_json::Value::Array(items) => items.iter().find_map(env_channel_violation),
            _ => None,
        }
    }

    /// True for the one legitimate GCP Batch environment block: a
    /// `secretVariables` map whose every entry is an `OXO_FLOW_SENSITIVE_<n>`
    /// name pointing at a Secret Manager resource path. Anything else —
    /// plaintext `variables`, a literal value under `secretVariables`, an
    /// extra key — is a leak channel and must fail the boundary test.
    fn is_gcp_secret_variables_environment(value: &serde_json::Value) -> bool {
        let Some(map) = value.as_object() else {
            return false;
        };
        if map.len() != 1 {
            return false;
        }
        let Some(vars) = map
            .get("secretVariables")
            .and_then(serde_json::Value::as_object)
        else {
            return false;
        };
        !vars.is_empty()
            && vars.iter().all(|(name, reference)| {
                name.starts_with("OXO_FLOW_SENSITIVE_")
                    && reference.as_str().is_some_and(is_gcp_secret_resource)
            })
    }

    /// A Secret Manager resource name, not a value: `projects/<p>/secrets/<s>/
    /// versions/<v>` (the `/locations/<l>/` variant keeps the same `/secrets/`
    /// and `/versions/` markers).
    fn is_gcp_secret_resource(reference: &str) -> bool {
        reference.starts_with("projects/")
            && reference.contains("/secrets/")
            && reference.contains("/versions/")
            && !reference.contains(char::is_whitespace)
    }

    /// Fail when a generated cloud job body carries a submit-host environment
    /// channel. The engine's own routed sensitive references are the single
    /// allowed `env` channel.
    fn assert_no_env_channel(value: &serde_json::Value, backend: &str, shape: &str) {
        if let Some(violation) = env_channel_violation(value) {
            panic!(
                "{backend} {shape} body must carry no submit-host environment channel — {violation}"
            );
        }
    }

    /// The detector must accept the legitimate #1176 secret-reference block
    /// and reject every other environment channel — pinning the predicate so
    /// a future relaxation cannot silently reopen the leak.
    #[test]
    fn env_channel_detector_accepts_only_routed_secret_refs() {
        let routed = serde_json::json!({
            "spec": {"containers": [{"env": [{
                "name": "OXO_FLOW_SENSITIVE_0",
                "valueFrom": {"secretKeyRef": {
                    "name": "oxoflow-secrets",
                    "key": "OXO_FLOW_SENSITIVE_0"
                }}
            }]}]}
        });
        assert_eq!(env_channel_violation(&routed), None);

        let ambient = serde_json::json!({"env": [{"name": "USER", "value": "wsx"}]});
        assert!(env_channel_violation(&ambient).is_some());

        let mixed = serde_json::json!({"env": [
            {"name": "OXO_FLOW_SENSITIVE_0",
             "valueFrom": {"secretKeyRef": {"name": "s", "key": "OXO_FLOW_SENSITIVE_0"}}},
            {"name": "AWS_ACCESS_KEY_ID", "value": "x"}
        ]});
        assert!(env_channel_violation(&mixed).is_some());

        // The routed NAME with a plaintext value is still a leak.
        let plaintext = serde_json::json!({"env": [{
            "name": "OXO_FLOW_SENSITIVE_0", "value": "literal-secret"
        }]});
        assert!(env_channel_violation(&plaintext).is_some());

        let environment = serde_json::json!({"environment": {"variables": {"A": "b"}}});
        assert!(env_channel_violation(&environment).is_some());

        // GCP Batch's submission-time secret references are the other
        // legitimate channel — but only as `secretVariables` resource paths.
        let gcp_routed = serde_json::json!({"taskSpec": {"environment": {
            "secretVariables": {
                "OXO_FLOW_SENSITIVE_0": "projects/p/secrets/s/versions/latest"
            }
        }}});
        assert_eq!(env_channel_violation(&gcp_routed), None);

        let gcp_literal = serde_json::json!({"taskSpec": {"environment": {
            "secretVariables": {"OXO_FLOW_SENSITIVE_0": "literal-secret"}
        }}});
        assert!(env_channel_violation(&gcp_literal).is_some());

        let gcp_mixed = serde_json::json!({"taskSpec": {"environment": {
            "secretVariables": {
                "OXO_FLOW_SENSITIVE_0": "projects/p/secrets/s/versions/latest"
            },
            "variables": {"AWS_ACCESS_KEY_ID": "x"}
        }}});
        assert!(env_channel_violation(&gcp_mixed).is_some());

        let unrelated = serde_json::json!({"args": ["echo hi"], "nested": [{"x": 1}]});
        assert_eq!(env_channel_violation(&unrelated), None);
    }

    /// Child half of [`assert_cloud_body_ignores_submit_host_env`]: runs in a
    /// process that carries sentinel ambient credentials and builds the real
    /// cloud job bodies from a rule whose command prints them. Nothing the
    /// submit host carries may appear in the body, while declared `envvars`
    /// must still reach it through the documented command channel
    /// (`export K='V'`).
    #[test]
    #[ignore = "child process of assert_cloud_body_ignores_submit_host_env"]
    fn submit_host_ambient_probe_child() {
        let backend_name = std::env::var("PROBE_CLOUD_BACKEND")
            .expect("the probe child must be started by assert_cloud_body_ignores_submit_host_env");
        let sentinels = [
            ("SENTINEL_AMBIENT", "leak-canary"),
            ("ANTHROPIC_AUTH_TOKEN", "leak-token"),
            ("OXO_FLOW_MASTER_KEY", "leak-key"),
        ];
        for (name, value) in sentinels {
            assert_eq!(
                std::env::var(name).as_deref(),
                Ok(value),
                "the probe child must inherit the ambient sentinel {name}"
            );
        }

        let rule = Rule {
            name: "ambient_probe".to_string(),
            shell: Some(
                "echo $SENTINEL_AMBIENT $ANTHROPIC_AUTH_TOKEN $OXO_FLOW_MASTER_KEY $DECLARED_VAR"
                    .to_string(),
            ),
            envvars: HashMap::from([("DECLARED_VAR".to_string(), "declared-value".to_string())]),
            resources: crate::rule::Resources {
                threads: 2,
                memory: Some("4G".to_string()),
                ..Default::default()
            },
            environment: crate::rule::EnvironmentSpec {
                docker: Some("ubuntu:latest".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let shell_cmd = build_execution_command(
            &rule,
            &HashMap::new(),
            &HashMap::new(),
            crate::scheduler::detect_system_limits(),
        )
        .expect("the probe rule's command must render")
        .expect("a shell rule must render a command");
        let config = crate::cluster::ClusterJobConfig {
            backend: match backend_name.as_str() {
                "kubernetes" => crate::cluster::ClusterBackend::Kubernetes,
                "aws-batch" => crate::cluster::ClusterBackend::AwsBatch,
                "gcp-batch" => crate::cluster::ClusterBackend::GcpBatch,
                other => panic!("unknown probe backend {other}"),
            },
            queue: None,
            account: None,
            walltime: None,
            extra_args: Vec::new(),
        };

        let (single, array) = match backend_name.as_str() {
            "kubernetes" => (
                crate::backend::k8s::generate_k8s_job_manifest(&rule, &shell_cmd, &config)
                    .expect("k8s manifest"),
                crate::backend::k8s::generate_k8s_array_job_manifest(
                    &rule,
                    "run/cmds",
                    3,
                    &[],
                    &config,
                )
                .expect("k8s array manifest"),
            ),
            "aws-batch" => (
                crate::backend::aws_batch::generate_aws_batch_job_spec(&rule, &shell_cmd, &config)
                    .expect("aws batch spec"),
                crate::backend::aws_batch::generate_aws_batch_array_spec(
                    &rule, "run/cmds", 3, &config,
                )
                .expect("aws batch array spec"),
            ),
            "gcp-batch" => (
                crate::backend::gcp_batch::generate_gcp_batch_job_spec(&rule, &shell_cmd, &config)
                    .expect("gcp batch spec"),
                crate::backend::gcp_batch::generate_gcp_batch_array_spec(
                    &rule, "run/cmds", 3, &config,
                )
                .expect("gcp batch array spec"),
            ),
            other => panic!("unknown probe backend {other}"),
        };

        for (shape, body) in [("single", &single), ("array", &array)] {
            for (name, value) in sentinels {
                assert!(
                    !body.contains(value),
                    "{backend_name} {shape} body carries the ambient {name} value: {body}"
                );
            }
            let val: serde_json::Value =
                serde_json::from_str(body).expect("job body must be valid json");
            assert_no_env_channel(&val, &backend_name, shape);
        }

        // Declared `envvars` travel in the rendered command, not in an env
        // array: the body must still carry them (issue #1171 check).
        assert!(
            single.contains("export DECLARED_VAR='declared-value'"),
            "declared envvars must reach the {backend_name} job body through the command channel: {single}"
        );
        // Handshake so the parent can tell a real run from an empty filter
        // match (a renamed child would otherwise make the probe vacuous).
        println!("CLOUD_AMBIENT_PROBE_OK {backend_name}");
    }
}
