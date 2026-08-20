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
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[cfg(feature = "cluster")]
pub mod cluster;
pub mod driver;

/// Scheduler-agnostic job state, mapped from each backend's native codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    Unknown,
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
    ) -> Result<String> {
        Err(OxoFlowError::Config {
            message: format!("backend '{}' does not support job arrays", self.name()),
        })
    }

    /// Fetch job logs / accounting output (raw text).
    async fn logs(&self, job_id: &str) -> Result<String>;

    /// Terminal status of a job that has left the live queue, read from the
    /// scheduler's accounting store. `None` means "no terminal record" —
    /// the driver keeps the job in flight. Backends without an accounting
    /// store keep the default and rely on their poller.
    async fn terminal_status(&self, _job_id: &str) -> Option<BackendJobStatus> {
        None
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
        let order = dag.execution_order()?;
        let groups = dag.parallel_groups()?;
        let mut rules = HashMap::new();
        for name in &order {
            if let Some(sr) = build_rule(name, config, dag, workdir, env_resolver, wildcard_values)?
            {
                rules.insert(name.clone(), sr);
            }
        }
        Ok(Self {
            order,
            groups,
            rules,
        })
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
            if let Some(sr) = build_rule(name, config, dag, workdir, env_resolver, wildcard_values)?
            {
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
    let Some(cmd) = build_execution_command(
        rule,
        wildcard_values,
        &config.workflow.interpreter_map,
        crate::scheduler::detect_system_limits(),
    ) else {
        return Ok(None); // no shell/script — not schedulable
    };
    // The prelude goes INSIDE the environment wrapper, matching the local
    // executor (issue #92): prepending outside would leave the wrapped
    // command (conda run / docker run bash -c) without fail-fast semantics.
    let cmd = config.defaults.apply_shell_prelude(&cmd);
    let wrapped = env_resolver
        .wrap_command(&cmd, &rule.environment, Some(&rule.resources), workdir)
        .map_err(|e| OxoFlowError::Config {
            message: format!("environment wrapping failed for '{name}': {e}"),
        })?;
    let dependencies = dag.dependencies(name).unwrap_or_default();
    let shell_cmd = format!("cd '{}' && {}", workdir.display(), wrapped);
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
}
