//! Job scheduling with resource constraints.
//!
//! The scheduler determines which jobs can run concurrently based on
//! available resources and dependency satisfaction.

use crate::dag::WorkflowDag;
use crate::error::Result;
use crate::executor::{JobRecord, JobStatus};
use crate::rule::Rule;
use std::collections::{HashMap, HashSet};

/// Tracks the state of all jobs in a workflow execution.
#[derive(Debug)]
pub struct SchedulerState {
    /// Map from rule name to current status.
    statuses: HashMap<String, JobStatus>,

    /// Set of rules currently running.
    running: HashSet<String>,

    /// Map from rule name to execution record (once complete).
    records: HashMap<String, JobRecord>,
}

impl SchedulerState {
    /// Create a new scheduler state for the given rules.
    pub fn new(rule_names: &[&str]) -> Self {
        let statuses = rule_names
            .iter()
            .map(|&name| (name.to_string(), JobStatus::Pending))
            .collect();

        Self {
            statuses,
            running: HashSet::new(),
            records: HashMap::new(),
        }
    }

    /// Returns the status of a rule.
    pub fn status(&self, rule: &str) -> Option<JobStatus> {
        self.statuses.get(rule).copied()
    }

    /// Mark a rule as running.
    pub fn mark_running(&mut self, rule: &str) {
        self.statuses.insert(rule.to_string(), JobStatus::Running);
        self.running.insert(rule.to_string());
    }

    /// Mark a rule as completed with a record.
    pub fn mark_completed(&mut self, record: JobRecord) {
        self.statuses.insert(record.rule.clone(), record.status);
        self.running.remove(&record.rule);
        self.records.insert(record.rule.clone(), record);
    }

    /// Returns all rules that are ready to run (dependencies satisfied, not yet started).
    pub fn ready_rules(&self, dag: &WorkflowDag) -> Result<Vec<String>> {
        let mut ready = Vec::new();

        for (rule, &status) in &self.statuses {
            if status != JobStatus::Pending {
                continue;
            }

            let deps = dag.dependencies(rule)?;
            let all_deps_done = deps.iter().all(|dep| {
                matches!(
                    self.statuses.get(dep),
                    Some(JobStatus::Success | JobStatus::Skipped)
                )
            });

            if all_deps_done {
                ready.push(rule.clone());
            }
        }

        // Sort by name for deterministic ordering
        ready.sort();
        Ok(ready)
    }

    /// Returns all ready rules sorted by priority (descending), then name (ascending).
    pub fn ready_rules_prioritized(
        &self,
        dag: &WorkflowDag,
        rules: &[Rule],
    ) -> Result<Vec<String>> {
        let mut ready = self.ready_rules(dag)?;

        let priority_map: HashMap<&str, i32> = rules
            .iter()
            .map(|r| (r.name.as_str(), r.priority))
            .collect();

        ready.sort_by(|a, b| {
            let pa = priority_map.get(a.as_str()).copied().unwrap_or(0);
            let pb = priority_map.get(b.as_str()).copied().unwrap_or(0);
            pb.cmp(&pa).then_with(|| a.cmp(b))
        });

        Ok(ready)
    }

    /// Returns ready rules prioritized by critical path membership, then by
    /// explicit priority, then by name.
    ///
    /// Rules on the critical path are scheduled first because they determine
    /// the minimum total execution time. Among equally critical rules,
    /// explicit priority and then alphabetical name break ties.
    pub fn ready_rules_critical_path(
        &self,
        dag: &WorkflowDag,
        rules: &[Rule],
    ) -> Result<Vec<String>> {
        let mut ready = self.ready_rules(dag)?;

        let critical_path: HashSet<String> = dag
            .critical_path()
            .unwrap_or_default()
            .into_iter()
            .collect();

        let priority_map: HashMap<&str, i32> = rules
            .iter()
            .map(|r| (r.name.as_str(), r.priority))
            .collect();

        ready.sort_by(|a, b| {
            let a_critical = critical_path.contains(a);
            let b_critical = critical_path.contains(b);
            // Critical path rules first
            b_critical
                .cmp(&a_critical)
                .then_with(|| {
                    // Then by explicit priority (descending)
                    let pa = priority_map.get(a.as_str()).copied().unwrap_or(0);
                    let pb = priority_map.get(b.as_str()).copied().unwrap_or(0);
                    pb.cmp(&pa)
                })
                .then_with(|| a.cmp(b))
        });

        Ok(ready)
    }

    /// Returns `true` if all rules have completed (success, failed, or skipped).
    pub fn is_complete(&self) -> bool {
        self.statuses.values().all(|s| {
            matches!(
                s,
                JobStatus::Success
                    | JobStatus::Failed
                    | JobStatus::Skipped
                    | JobStatus::Cancelled
                    | JobStatus::TimedOut
            )
        })
    }

    /// Returns `true` if any rule has failed.
    pub fn has_failures(&self) -> bool {
        self.statuses.values().any(|s| {
            matches!(
                s,
                JobStatus::Failed | JobStatus::Cancelled | JobStatus::TimedOut
            )
        })
    }

    /// Returns the number of currently running rules.
    pub fn running_count(&self) -> usize {
        self.running.len()
    }

    /// Returns all job records.
    pub fn records(&self) -> &HashMap<String, JobRecord> {
        &self.records
    }

    /// Returns a summary of the execution state.
    pub fn summary(&self) -> SchedulerSummary {
        let mut summary = SchedulerSummary::default();
        for status in self.statuses.values() {
            match status {
                JobStatus::Pending => summary.pending += 1,
                JobStatus::Running => summary.running += 1,
                JobStatus::Success => summary.success += 1,
                JobStatus::Failed => summary.failed += 1,
                JobStatus::Skipped => summary.skipped += 1,
                JobStatus::Queued => summary.pending += 1,
                JobStatus::Cancelled => summary.failed += 1,
                JobStatus::TimedOut => summary.failed += 1,
            }
        }
        summary.total = self.statuses.len();
        summary
    }
}

/// Summary of job execution progress.
#[derive(Debug, Default)]
pub struct SchedulerSummary {
    pub total: usize,
    pub pending: usize,
    pub running: usize,
    pub success: usize,
    pub failed: usize,
    pub skipped: usize,
}

impl std::fmt::Display for SchedulerSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "total: {}, success: {}, failed: {}, running: {}, pending: {}, skipped: {}",
            self.total, self.success, self.failed, self.running, self.pending, self.skipped
        )
    }
}

/// Parse a memory string (e.g., "8G", "16384M", "1T") into megabytes.
pub fn parse_memory_mb(memory: &str) -> Option<u64> {
    let memory = memory.trim();
    if memory.is_empty() {
        return None;
    }

    let (num_str, unit) = if memory.ends_with('T') || memory.ends_with('t') {
        (&memory[..memory.len() - 1], "T")
    } else if memory.ends_with('G') || memory.ends_with('g') {
        (&memory[..memory.len() - 1], "G")
    } else if memory.ends_with('M') || memory.ends_with('m') {
        (&memory[..memory.len() - 1], "M")
    } else if memory.ends_with('K') || memory.ends_with('k') {
        (&memory[..memory.len() - 1], "K")
    } else {
        // Assume megabytes
        (memory, "M")
    };

    let num: f64 = num_str.parse().ok()?;
    let mb = match unit {
        "T" => num * 1024.0 * 1024.0,
        "G" => num * 1024.0,
        "M" => num,
        "K" => num / 1024.0,
        _ => return None,
    };

    Some(mb as u64)
}

/// Resource pool tracking available system resources.
#[derive(Debug, Clone)]
pub struct ResourcePool {
    /// Available CPU threads.
    pub threads: u32,

    /// Available memory in MB.
    pub memory_mb: u64,
}

impl ResourcePool {
    /// Create a new resource pool with the given limits.
    pub fn new(threads: u32, memory_mb: u64) -> Self {
        Self { threads, memory_mb }
    }

    /// Check if a rule's resource requirements can be satisfied.
    pub fn can_accommodate(&self, rule: &Rule) -> bool {
        let required_threads = rule.effective_threads();
        let required_memory = rule
            .effective_memory()
            .and_then(parse_memory_mb)
            .unwrap_or(0);

        self.threads >= required_threads && self.memory_mb >= required_memory
    }

    /// Reserve resources for a rule.
    pub fn reserve(&mut self, rule: &Rule) {
        self.threads = self.threads.saturating_sub(rule.effective_threads());
        let mem = rule
            .effective_memory()
            .and_then(parse_memory_mb)
            .unwrap_or(0);
        self.memory_mb = self.memory_mb.saturating_sub(mem);
    }

    /// Release resources after a rule completes.
    pub fn release(&mut self, rule: &Rule, max_threads: u32, max_memory_mb: u64) {
        self.threads = (self.threads + rule.effective_threads()).min(max_threads);
        let mem = rule
            .effective_memory()
            .and_then(parse_memory_mb)
            .unwrap_or(0);
        self.memory_mb = (self.memory_mb + mem).min(max_memory_mb);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::{EnvironmentSpec, Resources};

    fn make_rules() -> Vec<Rule> {
        vec![
            Rule {
                name: "a".to_string(),
                input: vec![],
                output: vec!["a.txt".to_string()],
                shell: Some("echo a".to_string()),
                script: None,
                threads: None,
                memory: None,
                resources: Resources::default(),
                environment: EnvironmentSpec::default(),
                log: None,
                benchmark: None,
                params: HashMap::new(),
                priority: 0,
                target: false,
                group: None,
                description: None,
                ..Default::default()
            },
            Rule {
                name: "b".to_string(),
                input: vec!["a.txt".to_string()],
                output: vec!["b.txt".to_string()],
                shell: Some("echo b".to_string()),
                script: None,
                threads: None,
                memory: None,
                resources: Resources::default(),
                environment: EnvironmentSpec::default(),
                log: None,
                benchmark: None,
                params: HashMap::new(),
                priority: 0,
                target: false,
                group: None,
                description: None,
                ..Default::default()
            },
        ]
    }

    #[test]
    fn scheduler_initial_state() {
        let state = SchedulerState::new(&["a", "b"]);
        assert_eq!(state.status("a"), Some(JobStatus::Pending));
        assert_eq!(state.status("b"), Some(JobStatus::Pending));
        assert!(!state.is_complete());
    }

    #[test]
    fn scheduler_ready_rules() {
        let rules = make_rules();
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let state = SchedulerState::new(&["a", "b"]);

        let ready = state.ready_rules(&dag).unwrap();
        assert_eq!(ready, vec!["a"]);
    }

    #[test]
    fn scheduler_mark_running_and_complete() {
        let rules = make_rules();
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let mut state = SchedulerState::new(&["a", "b"]);

        state.mark_running("a");
        assert_eq!(state.status("a"), Some(JobStatus::Running));
        assert_eq!(state.running_count(), 1);

        state.mark_completed(JobRecord {
            rule: "a".to_string(),
            status: JobStatus::Success,
            started_at: None,
            finished_at: None,
            exit_code: Some(0),
            stdout: None,
            stderr: None,
            command: None,
            retries: 0,
            timeout: None,
        });

        assert_eq!(state.status("a"), Some(JobStatus::Success));
        assert_eq!(state.running_count(), 0);

        // Now "b" should be ready
        let ready = state.ready_rules(&dag).unwrap();
        assert_eq!(ready, vec!["b"]);
    }

    #[test]
    fn scheduler_summary() {
        let mut state = SchedulerState::new(&["a", "b", "c"]);
        state.mark_running("a");
        let summary = state.summary();
        assert_eq!(summary.total, 3);
        assert_eq!(summary.running, 1);
        assert_eq!(summary.pending, 2);
    }

    #[test]
    fn parse_memory() {
        assert_eq!(parse_memory_mb("8G"), Some(8192));
        assert_eq!(parse_memory_mb("16384M"), Some(16384));
        assert_eq!(parse_memory_mb("1T"), Some(1048576));
        assert_eq!(parse_memory_mb("512K"), Some(0)); // rounds down
        assert!(parse_memory_mb("").is_none());
        assert!(parse_memory_mb("abc").is_none());
    }

    #[test]
    fn resource_pool() {
        let rules = make_rules();
        let pool = ResourcePool::new(16, 32768);
        assert!(pool.can_accommodate(&rules[0]));
    }

    #[test]
    fn scheduler_ready_rules_prioritized() {
        // Create three independent rules (no deps between them) with different priorities.
        let rules = vec![
            Rule {
                name: "low".to_string(),
                input: vec![],
                output: vec!["low.txt".to_string()],
                shell: Some("echo low".to_string()),
                script: None,
                threads: None,
                memory: None,
                resources: Resources::default(),
                environment: EnvironmentSpec::default(),
                log: None,
                benchmark: None,
                params: HashMap::new(),
                priority: 1,
                target: false,
                group: None,
                description: None,
                ..Default::default()
            },
            Rule {
                name: "high".to_string(),
                input: vec![],
                output: vec!["high.txt".to_string()],
                shell: Some("echo high".to_string()),
                script: None,
                threads: None,
                memory: None,
                resources: Resources::default(),
                environment: EnvironmentSpec::default(),
                log: None,
                benchmark: None,
                params: HashMap::new(),
                priority: 10,
                target: false,
                group: None,
                description: None,
                ..Default::default()
            },
            Rule {
                name: "mid".to_string(),
                input: vec![],
                output: vec!["mid.txt".to_string()],
                shell: Some("echo mid".to_string()),
                script: None,
                threads: None,
                memory: None,
                resources: Resources::default(),
                environment: EnvironmentSpec::default(),
                log: None,
                benchmark: None,
                params: HashMap::new(),
                priority: 5,
                target: false,
                group: None,
                description: None,
                ..Default::default()
            },
        ];

        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let state = SchedulerState::new(&["low", "high", "mid"]);

        let prioritized = state.ready_rules_prioritized(&dag, &rules).unwrap();
        assert_eq!(prioritized, vec!["high", "mid", "low"]);
    }

    #[test]
    fn scheduler_critical_path_priority() {
        // Create a diamond DAG: source → left, right → merge
        let mut rules = vec![
            Rule {
                name: "source".to_string(),
                output: vec!["a.txt".to_string(), "b.txt".to_string()],
                shell: Some("echo source".to_string()),
                ..Default::default()
            },
            Rule {
                name: "left".to_string(),
                input: vec!["a.txt".to_string()],
                output: vec!["left.txt".to_string()],
                shell: Some("echo left".to_string()),
                ..Default::default()
            },
            Rule {
                name: "right".to_string(),
                input: vec!["b.txt".to_string()],
                output: vec!["right.txt".to_string()],
                shell: Some("echo right".to_string()),
                ..Default::default()
            },
            Rule {
                name: "merge".to_string(),
                input: vec!["left.txt".to_string(), "right.txt".to_string()],
                output: vec!["final.txt".to_string()],
                shell: Some("echo merge".to_string()),
                ..Default::default()
            },
        ];

        for rule in &mut rules {
            rule.resources = Resources::default();
            rule.environment = EnvironmentSpec::default();
        }

        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let mut state = SchedulerState::new(&["source", "left", "right", "merge"]);

        // Initially only "source" is ready
        let ready = state.ready_rules_critical_path(&dag, &rules).unwrap();
        assert_eq!(ready, vec!["source"]);

        // Complete "source"
        state.mark_completed(JobRecord {
            rule: "source".to_string(),
            status: JobStatus::Success,
            started_at: None,
            finished_at: None,
            exit_code: Some(0),
            stdout: None,
            stderr: None,
            command: None,
            retries: 0,
            timeout: None,
        });

        // Both "left" and "right" should be ready
        let ready = state.ready_rules_critical_path(&dag, &rules).unwrap();
        assert_eq!(ready.len(), 2);
        // Both left and right are on a critical path in a diamond, but the function should
        // still return them in a deterministic order
        assert!(ready.contains(&"left".to_string()));
        assert!(ready.contains(&"right".to_string()));
    }
}
