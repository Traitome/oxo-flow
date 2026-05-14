#![allow(deprecated)]
//! DAG (Directed Acyclic Graph) engine for workflow execution.
//!
//! Constructs a DAG from workflow rules by matching rule outputs to downstream
//! rule inputs. Provides topological sorting, cycle detection, and DOT format
//! export for visualization.

use crate::error::{OxoFlowError, Result};
use crate::rule::Rule;
use petgraph::algo::toposort;
use petgraph::dot::{Config, Dot};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::NodeRef;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A node in the workflow DAG, representing a single rule.
#[derive(Debug, Clone)]
pub struct DagNode {
    /// The rule name.
    pub name: String,

    /// Index into the original rule list.
    pub rule_index: usize,
}

impl std::fmt::Display for DagNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// The workflow DAG, built from rules and their input/output dependencies.
#[derive(Debug)]
pub struct WorkflowDag {
    /// The underlying directed graph.
    graph: DiGraph<DagNode, ()>,

    /// Map from rule name to node index.
    name_to_node: HashMap<String, NodeIndex>,

    /// Map from output file pattern to the rule that produces it.
    output_to_node: HashMap<String, NodeIndex>,
}

impl WorkflowDag {
    /// Build a DAG from a list of rules.
    ///
    /// Edges are created by matching rule outputs to downstream rule inputs.
    /// Returns an error if a cycle is detected or if duplicate rule names exist.
    #[must_use = "building a DAG returns a Result that must be used"]
    pub fn from_rules(rules: &[Rule]) -> Result<Self> {
        let mut graph = DiGraph::new();
        let mut name_to_node = HashMap::new();
        let mut output_to_node = HashMap::new();

        // Step 1: Add all rules as nodes
        for (idx, rule) in rules.iter().enumerate() {
            if name_to_node.contains_key(&rule.name) {
                return Err(OxoFlowError::DuplicateRule {
                    name: rule.name.clone(),
                });
            }

            let node = graph.add_node(DagNode {
                name: rule.name.clone(),
                rule_index: idx,
            });
            name_to_node.insert(rule.name.clone(), node);

            // Register outputs
            for output in &rule.output {
                output_to_node.insert(output.clone(), node);
            }
        }

        // Step 2: Add edges based on input/output matching
        for rule in rules {
            let consumer_node = name_to_node[&rule.name];
            for input in &rule.input {
                if let Some(&producer_node) = output_to_node.get(input) {
                    // producer → consumer (producer must run before consumer)
                    graph.add_edge(producer_node, consumer_node, ());
                }
                // If no producer found, the input is assumed to be a source file
            }

            // Step 2b: Add edges for explicit depends_on
            for dep_name in &rule.depends_on {
                if let Some(&dep_node) = name_to_node.get(dep_name) {
                    graph.add_edge(dep_node, consumer_node, ());
                }
                // Unknown depends_on targets are validated separately
            }
        }

        let dag = Self {
            graph,
            name_to_node,
            output_to_node,
        };

        // Step 3: Verify it's actually a DAG (no cycles)
        dag.validate()?;

        Ok(dag)
    }

    /// Validate that the graph is a valid DAG (no cycles).
    #[must_use = "validation returns a Result that must be checked"]
    pub fn validate(&self) -> Result<()> {
        match toposort(&self.graph, None) {
            Ok(_) => Ok(()),
            Err(cycle) => {
                let cycle_path = self.find_cycle_path(cycle.node_id());
                let path_str = cycle_path.join(" → ");
                Err(OxoFlowError::CycleDetected {
                    details: format!("cycle detected: {}", path_str),
                })
            }
        }
    }

    /// Find the actual cycle path starting from a node known to be in a cycle.
    fn find_cycle_path(&self, start: NodeIndex) -> Vec<String> {
        let mut visited = HashSet::new();
        let mut stack = Vec::new();
        let mut on_stack = HashSet::new();

        if let Some(cycle) = self.dfs_find_cycle(start, &mut visited, &mut stack, &mut on_stack) {
            cycle
        } else {
            // Fallback: just return the start node
            vec![self.graph[start].name.clone()]
        }
    }

    fn dfs_find_cycle(
        &self,
        node: NodeIndex,
        visited: &mut HashSet<NodeIndex>,
        stack: &mut Vec<NodeIndex>,
        on_stack: &mut HashSet<NodeIndex>,
    ) -> Option<Vec<String>> {
        visited.insert(node);
        stack.push(node);
        on_stack.insert(node);

        for neighbor in self
            .graph
            .neighbors_directed(node, petgraph::Direction::Outgoing)
        {
            if !visited.contains(&neighbor) {
                if let Some(cycle) = self.dfs_find_cycle(neighbor, visited, stack, on_stack) {
                    return Some(cycle);
                }
            } else if on_stack.contains(&neighbor) {
                // Found a cycle - extract it
                let cycle_start = stack
                    .iter()
                    .position(|&n| n == neighbor)
                    .expect("neighbor must be in stack when on_stack is true");
                let mut cycle: Vec<String> = stack[cycle_start..]
                    .iter()
                    .map(|&n| self.graph[n].name.clone())
                    .collect();
                cycle.push(self.graph[neighbor].name.clone()); // Close the cycle
                return Some(cycle);
            }
        }

        stack.pop();
        on_stack.remove(&node);
        None
    }

    /// Returns the rules in topological order (respecting dependencies).
    #[must_use = "topological ordering returns a Result that must be used"]
    pub fn topological_order(&self) -> Result<Vec<&DagNode>> {
        match toposort(&self.graph, None) {
            Ok(indices) => Ok(indices.iter().map(|&idx| &self.graph[idx]).collect()),
            Err(cycle) => {
                let cycle_path = self.find_cycle_path(cycle.node_id());
                let path_str = cycle_path.join(" → ");
                Err(OxoFlowError::CycleDetected {
                    details: format!("cycle detected: {}", path_str),
                })
            }
        }
    }

    /// Returns rule names in topological order.
    #[must_use = "execution ordering returns a Result that must be used"]
    pub fn execution_order(&self) -> Result<Vec<String>> {
        Ok(self
            .topological_order()?
            .into_iter()
            .map(|n| n.name.clone())
            .collect())
    }

    /// Returns the direct dependencies (upstream rules) for a given rule.
    #[must_use = "querying dependencies returns a Result that must be used"]
    pub fn dependencies(&self, rule_name: &str) -> Result<Vec<String>> {
        let node = self
            .name_to_node
            .get(rule_name)
            .ok_or(OxoFlowError::RuleNotFound {
                name: rule_name.to_string(),
            })?;

        Ok(self
            .graph
            .neighbors_directed(*node, petgraph::Direction::Incoming)
            .map(|n| self.graph[n].name.clone())
            .collect())
    }

    /// Returns the direct dependents (downstream rules) for a given rule.
    #[must_use = "querying dependents returns a Result that must be used"]
    pub fn dependents(&self, rule_name: &str) -> Result<Vec<String>> {
        let node = self
            .name_to_node
            .get(rule_name)
            .ok_or(OxoFlowError::RuleNotFound {
                name: rule_name.to_string(),
            })?;

        Ok(self
            .graph
            .neighbors_directed(*node, petgraph::Direction::Outgoing)
            .map(|n| self.graph[n].name.clone())
            .collect())
    }

    /// Returns the number of rules in the DAG.
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Returns the number of dependency edges in the DAG.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Returns rules that have no dependencies (entry points).
    #[must_use]
    pub fn root_rules(&self) -> Vec<String> {
        self.graph
            .node_indices()
            .filter(|&n| {
                self.graph
                    .neighbors_directed(n, petgraph::Direction::Incoming)
                    .next()
                    .is_none()
            })
            .map(|n| self.graph[n].name.clone())
            .collect()
    }

    /// Returns rules that have no dependents (leaf/target rules).
    #[must_use]
    pub fn leaf_rules(&self) -> Vec<String> {
        self.graph
            .node_indices()
            .filter(|&n| {
                self.graph
                    .neighbors_directed(n, petgraph::Direction::Outgoing)
                    .next()
                    .is_none()
            })
            .map(|n| self.graph[n].name.clone())
            .collect()
    }

    /// Export the DAG in DOT format for visualization with Graphviz.
    ///
    /// Nodes are labelled with the rule name only (not the internal Rust
    /// `DagNode` struct representation), making the output suitable for direct
    /// use with `dot`, `neato`, etc.
    pub fn to_dot(&self) -> String {
        format!(
            "{:?}",
            Dot::with_attr_getters(
                &self.graph,
                &[Config::EdgeNoLabel, Config::NodeNoLabel],
                &|_, _| String::new(),
                &|_, nr| format!("label = {:?}", nr.weight().name),
            )
        )
    }

    /// Returns whether a given output pattern is produced by any rule.
    pub fn has_producer(&self, output: &str) -> bool {
        self.output_to_node.contains_key(output)
    }

    /// Returns rules that have no edges (neither produce outputs consumed by others
    /// nor consume outputs of others). These are isolated in the graph.
    #[must_use]
    pub fn orphan_rules(&self) -> Vec<&str> {
        self.graph
            .node_indices()
            .filter(|&n| {
                self.graph
                    .neighbors_directed(n, petgraph::Direction::Incoming)
                    .next()
                    .is_none()
                    && self
                        .graph
                        .neighbors_directed(n, petgraph::Direction::Outgoing)
                        .next()
                        .is_none()
            })
            .map(|n| self.graph[n].name.as_str())
            .collect()
    }

    /// Returns groups of rule names that can execute in parallel.
    ///
    /// Each group contains rules whose dependencies have all been satisfied
    /// by rules in previous groups. This is computed by assigning each node
    /// a "depth" equal to the length of the longest path from any root node.
    #[must_use = "computing parallel groups returns a Result that must be used"]
    pub fn parallel_groups(&self) -> Result<Vec<Vec<String>>> {
        let order = self.topological_order()?;
        let mut depth: HashMap<NodeIndex, usize> = HashMap::new();

        // Compute depth for each node
        for node_data in &order {
            let node_idx = self.name_to_node[&node_data.name];
            let max_parent_depth = self
                .graph
                .neighbors_directed(node_idx, petgraph::Direction::Incoming)
                .map(|parent| depth.get(&parent).copied().unwrap_or(0))
                .max()
                .map(|d| d + 1)
                .unwrap_or(0);
            depth.insert(node_idx, max_parent_depth);
        }

        // Group nodes by depth
        let max_depth = depth.values().copied().max().unwrap_or(0);
        let mut groups: Vec<Vec<String>> = vec![Vec::new(); max_depth + 1];
        for (&node_idx, &d) in &depth {
            groups[d].push(self.graph[node_idx].name.clone());
        }

        // Sort each group for deterministic output
        for group in &mut groups {
            group.sort();
        }

        Ok(groups)
    }
}

/// Complexity metrics for a workflow DAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagMetrics {
    /// Total number of rules (nodes).
    pub node_count: usize,
    /// Total number of dependencies (edges).
    pub edge_count: usize,
    /// Maximum depth of the DAG (longest path from root to leaf).
    pub max_depth: usize,
    /// Maximum width (max rules at any single depth level).
    pub max_width: usize,
    /// Length of the critical path (longest chain of dependencies).
    pub critical_path_length: usize,
    /// Number of independent parallel groups.
    pub parallel_group_count: usize,
}

impl std::fmt::Display for DagMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DAG metrics: depth={}, width={}, critical_path={}",
            self.max_depth, self.max_width, self.critical_path_length
        )
    }
}

impl WorkflowDag {
    /// Detect output pattern collisions between rules.
    ///
    /// Returns a list of warnings when multiple rules produce outputs that
    /// match the same pattern.
    #[must_use]
    pub fn detect_output_collisions(rules: &[crate::rule::Rule]) -> Vec<String> {
        let mut warnings = Vec::new();
        for (i, r1) in rules.iter().enumerate() {
            for r2 in rules.iter().skip(i + 1) {
                for o1 in &r1.output {
                    for o2 in &r2.output {
                        // Strip wildcards for pattern comparison
                        let p1 = crate::wildcard::extract_wildcards(o1);
                        let p2 = crate::wildcard::extract_wildcards(o2);
                        // If same wildcards produce same template
                        let t1 = o1.replace(['{', '}'], "");
                        let t2 = o2.replace(['{', '}'], "");
                        if t1 == t2 && !p1.is_empty() && !p2.is_empty() {
                            warnings.push(format!(
                                "Output pattern collision: rules '{}' and '{}' both produce '{}' with overlapping wildcards",
                                r1.name, r2.name, o1
                            ));
                        }
                    }
                }
            }
        }
        warnings
    }

    /// Compute complexity metrics for the DAG.
    #[must_use = "computing metrics returns a Result that must be used"]
    pub fn metrics(&self) -> Result<DagMetrics> {
        let groups = self.parallel_groups()?;
        let max_width = groups.iter().map(|g| g.len()).max().unwrap_or(0);
        let max_depth = groups.len();

        // Critical path = longest chain = max_depth (in a DAG grouped by levels)
        let critical_path_length = max_depth;

        Ok(DagMetrics {
            node_count: self.node_count(),
            edge_count: self.edge_count(),
            max_depth,
            max_width,
            critical_path_length,
            parallel_group_count: groups.len(),
        })
    }

    /// Export the DAG in enhanced DOT format with parallel execution groups
    /// shown as ranked subgraph clusters.
    ///
    /// This produces more visually informative output than [`to_dot()`], with:
    /// - Nodes grouped by execution level (parallel groups)
    /// - Styled nodes with shape and color
    /// - Edge labels omitted for cleanliness
    pub fn to_dot_clustered(&self) -> Result<String> {
        let groups = self.parallel_groups()?;
        let mut dot = String::from("digraph workflow {\n");
        dot.push_str("  rankdir=TB;\n");
        dot.push_str("  node [shape=box, style=\"rounded,filled\", fillcolor=\"#e8f0fe\", fontname=\"Helvetica\"];\n");
        dot.push_str("  edge [color=\"#666666\"];\n\n");

        for (i, group) in groups.iter().enumerate() {
            dot.push_str(&format!("  subgraph cluster_{} {{\n", i));
            dot.push_str(&format!("    label = \"Level {}\";\n", i));
            dot.push_str("    style = dashed;\n");
            dot.push_str("    color = \"#cccccc\";\n");
            for name in group {
                dot.push_str(&format!("    \"{}\";\n", name));
            }
            dot.push_str("  }\n\n");
        }

        // Add edges
        for edge in self.graph.edge_indices() {
            if let Some((src, dst)) = self.graph.edge_endpoints(edge) {
                dot.push_str(&format!(
                    "  \"{}\" -> \"{}\";\n",
                    self.graph[src].name, self.graph[dst].name
                ));
            }
        }

        dot.push_str("}\n");
        Ok(dot)
    }

    /// Returns the critical path — the longest chain of sequential dependencies.
    ///
    /// This is the sequence of rules that determines the minimum execution time
    /// even with unlimited parallelism.
    #[must_use = "computing critical path returns a Result that must be used"]
    pub fn critical_path(&self) -> Result<Vec<String>> {
        let order = self.topological_order()?;
        let mut depth: HashMap<NodeIndex, usize> = HashMap::new();
        let mut predecessor: HashMap<NodeIndex, Option<NodeIndex>> = HashMap::new();

        for node_data in &order {
            let node_idx = self.name_to_node[&node_data.name];
            let mut best_parent: Option<NodeIndex> = None;
            let mut best_depth: usize = 0;

            for parent in self
                .graph
                .neighbors_directed(node_idx, petgraph::Direction::Incoming)
            {
                let parent_d = depth.get(&parent).copied().unwrap_or(0) + 1;
                if parent_d > best_depth {
                    best_depth = parent_d;
                    best_parent = Some(parent);
                }
            }

            depth.insert(node_idx, best_depth);
            predecessor.insert(node_idx, best_parent);
        }

        // Find the node with maximum depth
        let end_node = depth.iter().max_by_key(|&(_, &d)| d).map(|(&n, _)| n);

        let Some(mut current) = end_node else {
            return Ok(vec![]);
        };

        // Trace back to build the critical path
        let mut path = vec![self.graph[current].name.clone()];
        while let Some(Some(prev)) = predecessor.get(&current) {
            path.push(self.graph[*prev].name.clone());
            current = *prev;
        }
        path.reverse();
        Ok(path)
    }

    /// Generate an ASCII/terminal visualization of the DAG.
    ///
    /// Produces a simple, readable graph showing:
    /// - Execution levels (parallel groups)
    /// - Dependency arrows between rules
    /// - Summary statistics
    ///
    /// This output is suitable for terminal display without requiring Graphviz.
    #[must_use = "generating ASCII graph returns a Result that must be used"]
    pub fn to_ascii(&self) -> Result<String> {
        let groups = self.parallel_groups()?;
        let metrics = self.metrics()?;

        let mut output = String::new();

        // ANSI color codes for terminal output
        let cyan = "\x1b[36m";
        let green = "\x1b[32m";
        let yellow = "\x1b[33m";
        let bold = "\x1b[1m";
        let reset = "\x1b[0m";

        // Calculate content widths for proper alignment
        let line1 = format!(
            "Workflow DAG: {} rules, {} dependencies",
            self.node_count(),
            self.edge_count()
        );
        let line2 = format!(
            "Depth: {}, Width: {}, Critical path: {} steps",
            metrics.max_depth, metrics.max_width, metrics.critical_path_length
        );
        let max_content_width = std::cmp::max(line1.len(), line2.len());
        let box_width = max_content_width + 4; // 2 spaces on each side

        // Header with metrics (properly aligned)
        output.push_str(&format!(
            "{}\n",
            "┌".to_string() + &"─".repeat(box_width) + "┐"
        ));
        output.push_str(&format!(
            "│  {}{}{}{}{}  │\n",
            bold,
            cyan,
            line1,
            reset,
            " ".repeat(box_width - line1.len() - 4)
        ));
        output.push_str(&format!(
            "│  {}{}{}{}{}  │\n",
            bold,
            yellow,
            line2,
            reset,
            " ".repeat(box_width - line2.len() - 4)
        ));
        output.push_str(&format!(
            "{}\n\n",
            "└".to_string() + &"─".repeat(box_width) + "┘"
        ));

        // Draw execution levels
        for (level, rules) in groups.iter().enumerate() {
            // Level header with color
            output.push_str(&format!("{}Level {}{} ", bold, level, reset));

            // Indicate parallelism
            if rules.len() > 1 {
                output.push_str(&format!(
                    "{}(parallel: {} rules){}\n",
                    green,
                    rules.len(),
                    reset
                ));
            } else {
                output.push_str(&format!("{}(sequential){}\n", yellow, reset));
            }

            // Draw rules in this level
            for (i, rule) in rules.iter().enumerate() {
                if rules.len() > 1 && i == 0 {
                    output.push_str("┌─── ");
                } else if rules.len() > 1 && i == rules.len() - 1 {
                    output.push_str("└─── ");
                } else if rules.len() > 1 {
                    output.push_str("│─── ");
                } else {
                    output.push_str("     ");
                }

                // Get dependencies for this rule
                let deps = self.dependencies(rule)?;
                if deps.is_empty() {
                    output.push_str(&format!("{}{}{}\n", cyan, rule, reset));
                } else {
                    output.push_str(&format!(
                        "{}{}{} {}[depends: {}]\n",
                        cyan,
                        rule,
                        reset,
                        yellow,
                        deps.join(", ")
                    ));
                }
            }

            // Add arrow to next level if exists
            if level < groups.len() - 1 {
                output.push_str("     │\n");
                output.push_str(&format!("     {}▼{}\n", green, reset));
            }
        }

        // Footer with critical path
        let critical = self.critical_path()?;
        if critical.len() > 1 {
            output.push_str(&format!(
                "\n{}Critical path:{} {}{}{}\n",
                bold,
                reset,
                cyan,
                critical.join(&format!(" {}→{} ", green, reset)),
                reset
            ));
        }

        Ok(output)
    }

    /// Generate a compact ASCII graph showing the dependency tree.
    ///
    /// This produces a tree-like visualization focused on the dependency
    /// structure, suitable for quick inspection in the terminal.
    #[must_use = "generating compact ASCII graph returns a Result that must be used"]
    pub fn to_ascii_tree(&self) -> Result<String> {
        let order = self.execution_order()?;
        let mut output = String::new();

        output.push_str("Workflow Graph (terminal output)\n");
        output.push_str(&format!(
            "{} rules, {} edges\n\n",
            self.node_count(),
            self.edge_count()
        ));

        for (i, rule_name) in order.iter().enumerate() {
            let deps = self.dependencies(rule_name)?;
            let dep_str = if deps.is_empty() {
                " ──●".to_string()
            } else {
                format!(" ──● [{}]", deps.join(", "))
            };

            // Draw position indicator
            output.push_str(&format!("{:3}. {}{}\n", i + 1, rule_name, dep_str));

            // Show downstream if exists
            let downstream = self.dependents(rule_name)?;
            if !downstream.is_empty() {
                output.push_str(&format!("      ↓ {}\n", downstream.join(", ")));
            }
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::{EnvironmentSpec, Resources};
    use std::collections::HashMap;

    fn make_rule(name: &str, inputs: Vec<&str>, outputs: Vec<&str>) -> Rule {
        Rule {
            name: name.to_string(),
            input: inputs.into_iter().map(String::from).collect(),
            output: outputs.into_iter().map(String::from).collect(),
            shell: Some(format!("echo {name}")),
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
        }
    }

    #[test]
    fn linear_dag() {
        let rules = vec![
            make_rule("step1", vec!["input.txt"], vec!["mid.txt"]),
            make_rule("step2", vec!["mid.txt"], vec!["output.txt"]),
        ];

        let dag = WorkflowDag::from_rules(&rules).unwrap();
        assert_eq!(dag.node_count(), 2);
        assert_eq!(dag.edge_count(), 1);

        let order = dag.execution_order().unwrap();
        assert_eq!(order, vec!["step1", "step2"]);
    }

    #[test]
    fn diamond_dag() {
        let rules = vec![
            make_rule("source", vec!["raw.txt"], vec!["a.txt", "b.txt"]),
            make_rule("left", vec!["a.txt"], vec!["left.txt"]),
            make_rule("right", vec!["b.txt"], vec!["right.txt"]),
            make_rule("merge", vec!["left.txt", "right.txt"], vec!["final.txt"]),
        ];

        let dag = WorkflowDag::from_rules(&rules).unwrap();
        assert_eq!(dag.node_count(), 4);
        assert_eq!(dag.edge_count(), 4);

        let order = dag.execution_order().unwrap();
        // source must come first, merge must come last
        assert_eq!(order[0], "source");
        assert_eq!(order[3], "merge");
    }

    #[test]
    fn independent_rules() {
        let rules = vec![
            make_rule("a", vec!["x.txt"], vec!["a.txt"]),
            make_rule("b", vec!["y.txt"], vec!["b.txt"]),
        ];

        let dag = WorkflowDag::from_rules(&rules).unwrap();
        assert_eq!(dag.node_count(), 2);
        assert_eq!(dag.edge_count(), 0);
    }

    #[test]
    fn duplicate_rule_name() {
        let rules = vec![
            make_rule("step", vec![], vec!["a.txt"]),
            make_rule("step", vec![], vec!["b.txt"]),
        ];

        let result = WorkflowDag::from_rules(&rules);
        assert!(result.is_err());
    }

    #[test]
    fn root_and_leaf_rules() {
        let rules = vec![
            make_rule("source", vec!["raw.txt"], vec!["mid.txt"]),
            make_rule("sink", vec!["mid.txt"], vec!["out.txt"]),
        ];

        let dag = WorkflowDag::from_rules(&rules).unwrap();
        assert_eq!(dag.root_rules(), vec!["source"]);
        assert_eq!(dag.leaf_rules(), vec!["sink"]);
    }

    #[test]
    fn dependencies_and_dependents() {
        let rules = vec![
            make_rule("a", vec!["in.txt"], vec!["mid.txt"]),
            make_rule("b", vec!["mid.txt"], vec!["out.txt"]),
        ];

        let dag = WorkflowDag::from_rules(&rules).unwrap();
        assert_eq!(dag.dependencies("b").unwrap(), vec!["a"]);
        assert_eq!(dag.dependents("a").unwrap(), vec!["b"]);
        assert!(dag.dependencies("a").unwrap().is_empty());
        assert!(dag.dependents("b").unwrap().is_empty());
    }

    #[test]
    fn dot_export() {
        let rules = vec![
            make_rule("a", vec!["in.txt"], vec!["mid.txt"]),
            make_rule("b", vec!["mid.txt"], vec!["out.txt"]),
        ];

        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let dot = dag.to_dot();
        assert!(dot.contains("digraph"));
        // Node labels should use the rule name, not the Rust Debug representation.
        assert!(dot.contains("\"a\"") || dot.contains("label = \"a\""));
        assert!(
            !dot.contains("DagNode"),
            "DOT output should not contain Rust struct names"
        );
        assert!(
            !dot.contains("rule_index"),
            "DOT output should not expose internal fields"
        );
    }

    #[test]
    fn has_producer() {
        let rules = vec![make_rule("a", vec!["in.txt"], vec!["out.txt"])];

        let dag = WorkflowDag::from_rules(&rules).unwrap();
        assert!(dag.has_producer("out.txt"));
        assert!(!dag.has_producer("nonexistent.txt"));
    }

    #[test]
    fn rule_not_found() {
        let rules = vec![make_rule("a", vec![], vec!["out.txt"])];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        assert!(dag.dependencies("nonexistent").is_err());
    }

    #[test]
    fn parallel_groups_linear() {
        let rules = vec![
            make_rule("a", vec!["in.txt"], vec!["mid1.txt"]),
            make_rule("b", vec!["mid1.txt"], vec!["mid2.txt"]),
            make_rule("c", vec!["mid2.txt"], vec!["out.txt"]),
        ];

        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let groups = dag.parallel_groups().unwrap();
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0], vec!["a"]);
        assert_eq!(groups[1], vec!["b"]);
        assert_eq!(groups[2], vec!["c"]);
    }

    #[test]
    fn parallel_groups_diamond() {
        let rules = vec![
            make_rule("source", vec!["raw.txt"], vec!["a.txt", "b.txt"]),
            make_rule("left", vec!["a.txt"], vec!["left.txt"]),
            make_rule("right", vec!["b.txt"], vec!["right.txt"]),
            make_rule("merge", vec!["left.txt", "right.txt"], vec!["final.txt"]),
        ];

        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let groups = dag.parallel_groups().unwrap();
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0], vec!["source"]);
        assert_eq!(groups[1], vec!["left", "right"]);
        assert_eq!(groups[2], vec!["merge"]);
    }

    #[test]
    fn parallel_groups_independent() {
        let rules = vec![
            make_rule("a", vec!["x.txt"], vec!["a.txt"]),
            make_rule("b", vec!["y.txt"], vec!["b.txt"]),
        ];

        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let groups = dag.parallel_groups().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0], vec!["a", "b"]);
    }

    #[test]
    fn dag_metrics_linear() {
        let rules = vec![
            make_rule("a", vec![], vec!["a.txt"]),
            make_rule("b", vec!["a.txt"], vec!["b.txt"]),
            make_rule("c", vec!["b.txt"], vec!["c.txt"]),
        ];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let m = dag.metrics().unwrap();
        assert_eq!(m.node_count, 3);
        assert_eq!(m.max_depth, 3);
        assert_eq!(m.max_width, 1);
    }

    #[test]
    fn dag_metrics_wide() {
        let rules = vec![
            make_rule("a", vec![], vec!["a.txt"]),
            make_rule("b", vec![], vec!["b.txt"]),
            make_rule("c", vec![], vec!["c.txt"]),
        ];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let m = dag.metrics().unwrap();
        assert_eq!(m.node_count, 3);
        assert_eq!(m.max_depth, 1);
        assert_eq!(m.max_width, 3);
    }

    #[test]
    fn orphan_rules_all_independent() {
        let rules = vec![
            make_rule("a", vec!["x.txt"], vec!["a.txt"]),
            make_rule("b", vec!["y.txt"], vec!["b.txt"]),
        ];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let mut orphans = dag.orphan_rules();
        orphans.sort();
        assert_eq!(orphans, vec!["a", "b"]);
    }

    #[test]
    fn orphan_rules_none_when_connected() {
        let rules = vec![
            make_rule("a", vec!["in.txt"], vec!["mid.txt"]),
            make_rule("b", vec!["mid.txt"], vec!["out.txt"]),
        ];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        assert!(dag.orphan_rules().is_empty());
    }

    #[test]
    fn orphan_rules_mixed() {
        let rules = vec![
            make_rule("a", vec!["in.txt"], vec!["mid.txt"]),
            make_rule("b", vec!["mid.txt"], vec!["out.txt"]),
            make_rule("orphan", vec!["external.txt"], vec!["standalone.txt"]),
        ];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        assert_eq!(dag.orphan_rules(), vec!["orphan"]);
    }

    #[test]
    fn parallel_groups_single_node() {
        let rules = vec![make_rule("only", vec!["in.txt"], vec!["out.txt"])];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let groups = dag.parallel_groups().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0], vec!["only"]);
    }

    #[test]
    fn detect_output_collisions_none() {
        let r1 = crate::rule::Rule {
            name: "align".to_string(),
            output: vec!["aligned/{sample}.bam".to_string()],
            ..Default::default()
        };
        let r2 = crate::rule::Rule {
            name: "sort".to_string(),
            output: vec!["sorted/{sample}.bam".to_string()],
            ..Default::default()
        };
        let warnings = WorkflowDag::detect_output_collisions(&[r1, r2]);
        assert!(warnings.is_empty());
    }

    #[test]
    fn detect_output_collisions_found() {
        let r1 = crate::rule::Rule {
            name: "caller_a".to_string(),
            output: vec!["{sample}.vcf".to_string()],
            ..Default::default()
        };
        let r2 = crate::rule::Rule {
            name: "caller_b".to_string(),
            output: vec!["{sample}.vcf".to_string()],
            ..Default::default()
        };
        let warnings = WorkflowDag::detect_output_collisions(&[r1, r2]);
        assert!(!warnings.is_empty());
    }

    #[test]
    fn stress_test_large_dag() {
        let rules: Vec<crate::rule::Rule> = (0..1000)
            .map(|i| {
                let input = if i == 0 {
                    vec!["input.txt".to_string()]
                } else {
                    vec![format!("step_{}.out", i - 1)]
                };
                crate::rule::Rule {
                    name: format!("step_{}", i),
                    input,
                    output: vec![format!("step_{}.out", i)],
                    shell: Some(format!("process step_{}", i)),
                    ..Default::default()
                }
            })
            .collect();
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        assert_eq!(dag.node_count(), 1000);
        let order = dag.execution_order().unwrap();
        assert_eq!(order.len(), 1000);
        assert_eq!(order[0], "step_0");
        assert_eq!(order[999], "step_999");
    }

    #[test]
    fn dag_metrics_display() {
        let metrics = DagMetrics {
            node_count: 10,
            edge_count: 12,
            max_depth: 5,
            max_width: 3,
            critical_path_length: 5,
            parallel_group_count: 3,
        };
        let s = metrics.to_string();
        assert!(s.contains("depth=5"));
        assert!(s.contains("width=3"));
    }

    // ---- Tests for depends_on edges -----------------------------------------

    #[test]
    fn depends_on_creates_edge() {
        let mut rule_a = make_rule("setup", vec![], vec![]);
        rule_a.shell = Some("echo setup".to_string());

        let mut rule_b = make_rule("align", vec!["input.fq"], vec!["output.bam"]);
        rule_b.depends_on = vec!["setup".to_string()];

        let dag = WorkflowDag::from_rules(&[rule_a, rule_b]).unwrap();
        assert_eq!(dag.edge_count(), 1);
        let order = dag.execution_order().unwrap();
        assert_eq!(order[0], "setup");
        assert_eq!(order[1], "align");
    }

    #[test]
    fn dag_with_file_and_depends_on_edges() {
        // step1 produces mid.txt, step2 consumes it (file edge)
        // step2 also explicitly depends_on init (explicit edge)
        let init = make_rule("init", vec![], vec![]);
        let step1 = make_rule("step1", vec!["input.txt"], vec!["mid.txt"]);
        let mut step2 = make_rule("step2", vec!["mid.txt"], vec!["output.txt"]);
        step2.depends_on = vec!["init".to_string()];

        let dag = WorkflowDag::from_rules(&[init, step1, step2]).unwrap();
        // 1 file-based edge (step1→step2) + 1 depends_on edge (init→step2)
        assert_eq!(dag.edge_count(), 2);
        let order = dag.execution_order().unwrap();
        // step2 must come last
        assert_eq!(order.last().unwrap(), "step2");
    }

    // ---- critical_path tests ------------------------------------------------

    #[test]
    fn critical_path_linear() {
        let rules = vec![
            make_rule("a", vec!["in.txt"], vec!["mid1.txt"]),
            make_rule("b", vec!["mid1.txt"], vec!["mid2.txt"]),
            make_rule("c", vec!["mid2.txt"], vec!["out.txt"]),
        ];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let path = dag.critical_path().unwrap();
        assert_eq!(path, vec!["a", "b", "c"]);
    }

    #[test]
    fn critical_path_diamond() {
        let rules = vec![
            make_rule("source", vec!["raw.txt"], vec!["a.txt", "b.txt"]),
            make_rule("left", vec!["a.txt"], vec!["left.txt"]),
            make_rule("right", vec!["b.txt"], vec!["right.txt"]),
            make_rule("merge", vec!["left.txt", "right.txt"], vec!["final.txt"]),
        ];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let path = dag.critical_path().unwrap();
        // The critical path has 3 nodes: source → (left or right) → merge
        assert_eq!(path.len(), 3);
        assert_eq!(path[0], "source");
        assert_eq!(path[2], "merge");
    }

    #[test]
    fn critical_path_independent_rules() {
        let rules = vec![
            make_rule("a", vec!["x.txt"], vec!["a.txt"]),
            make_rule("b", vec!["y.txt"], vec!["b.txt"]),
        ];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let path = dag.critical_path().unwrap();
        // No dependencies, so the critical path is a single node
        assert_eq!(path.len(), 1);
    }

    #[test]
    fn critical_path_single_node() {
        let rules = vec![make_rule("only", vec!["in.txt"], vec!["out.txt"])];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let path = dag.critical_path().unwrap();
        assert_eq!(path, vec!["only"]);
    }

    #[test]
    fn dot_clustered_output() {
        let rules = vec![
            make_rule("source", vec!["raw.txt"], vec!["a.txt", "b.txt"]),
            make_rule("left", vec!["a.txt"], vec!["left.txt"]),
            make_rule("right", vec!["b.txt"], vec!["right.txt"]),
            make_rule("merge", vec!["left.txt", "right.txt"], vec!["final.txt"]),
        ];

        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let dot = dag.to_dot_clustered().unwrap();
        assert!(dot.contains("digraph workflow"));
        assert!(dot.contains("cluster_0"));
        assert!(dot.contains("cluster_1"));
        assert!(dot.contains("cluster_2"));
        assert!(dot.contains("Level 0"));
        assert!(dot.contains("\"source\""));
        assert!(dot.contains("\"merge\""));
    }

    #[test]
    fn cycle_detection_shows_path() {
        // Create a cycle: a -> b -> c -> a
        let rules = vec![
            make_rule("a", vec!["c.txt"], vec!["a.txt"]),
            make_rule("b", vec!["a.txt"], vec!["b.txt"]),
            make_rule("c", vec!["b.txt"], vec!["c.txt"]),
        ];

        let result = WorkflowDag::from_rules(&rules);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        // Should show the cycle path with arrows
        assert!(
            err_msg.contains('→'),
            "error should show cycle path with arrows: {}",
            err_msg
        );
        // Should mention at least two of the cycle nodes
        let mentions_a = err_msg.contains("a");
        let mentions_b = err_msg.contains("b");
        let mentions_c = err_msg.contains("c");
        assert!(
            [mentions_a, mentions_b, mentions_c]
                .iter()
                .filter(|&&x| x)
                .count()
                >= 2,
            "error should mention multiple cycle nodes: {}",
            err_msg
        );
    }

    // ---- ASCII output tests --------------------------------------------------

    #[test]
    fn ascii_output_basic() {
        let rules = vec![
            make_rule("a", vec!["in.txt"], vec!["mid.txt"]),
            make_rule("b", vec!["mid.txt"], vec!["out.txt"]),
        ];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let ascii = dag.to_ascii().unwrap();
        assert!(ascii.contains("Workflow DAG"));
        assert!(ascii.contains("Level 0"));
        assert!(ascii.contains("a"));
        assert!(ascii.contains("b"));
        assert!(ascii.contains("Critical path"));
    }

    #[test]
    fn ascii_output_parallel() {
        let rules = vec![
            make_rule("source", vec!["raw.txt"], vec!["a.txt", "b.txt"]),
            make_rule("left", vec!["a.txt"], vec!["left.txt"]),
            make_rule("right", vec!["b.txt"], vec!["right.txt"]),
            make_rule("merge", vec!["left.txt", "right.txt"], vec!["final.txt"]),
        ];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let ascii = dag.to_ascii().unwrap();
        assert!(ascii.contains("parallel"));
        assert!(ascii.contains("left"));
        assert!(ascii.contains("right"));
        assert!(ascii.contains("merge"));
    }

    #[test]
    fn ascii_tree_output() {
        let rules = vec![
            make_rule("a", vec!["in.txt"], vec!["mid.txt"]),
            make_rule("b", vec!["mid.txt"], vec!["out.txt"]),
        ];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let tree = dag.to_ascii_tree().unwrap();
        assert!(tree.contains("Workflow Graph"));
        assert!(tree.contains("1."));
        assert!(tree.contains("2."));
        assert!(tree.contains("a"));
        assert!(tree.contains("b"));
    }
}
