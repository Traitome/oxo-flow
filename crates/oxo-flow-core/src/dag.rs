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
use std::collections::HashMap;

/// A node in the workflow DAG, representing a single rule.
#[derive(Debug, Clone)]
pub struct DagNode {
    /// The rule name.
    pub name: String,

    /// Index into the original rule list.
    pub rule_index: usize,
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
    pub fn validate(&self) -> Result<()> {
        match toposort(&self.graph, None) {
            Ok(_) => Ok(()),
            Err(cycle) => {
                let node = &self.graph[cycle.node_id()];
                Err(OxoFlowError::CycleDetected {
                    details: format!("cycle involves rule '{}'", node.name),
                })
            }
        }
    }

    /// Returns the rules in topological order (respecting dependencies).
    pub fn topological_order(&self) -> Result<Vec<&DagNode>> {
        match toposort(&self.graph, None) {
            Ok(indices) => Ok(indices.iter().map(|&idx| &self.graph[idx]).collect()),
            Err(cycle) => {
                let node = &self.graph[cycle.node_id()];
                Err(OxoFlowError::CycleDetected {
                    details: format!("cycle involves rule '{}'", node.name),
                })
            }
        }
    }

    /// Returns rule names in topological order.
    pub fn execution_order(&self) -> Result<Vec<String>> {
        Ok(self
            .topological_order()?
            .into_iter()
            .map(|n| n.name.clone())
            .collect())
    }

    /// Returns the direct dependencies (upstream rules) for a given rule.
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
    pub fn to_dot(&self) -> String {
        format!(
            "{:?}",
            Dot::with_config(&self.graph, &[Config::EdgeNoLabel])
        )
    }

    /// Returns whether a given output pattern is produced by any rule.
    pub fn has_producer(&self, output: &str) -> bool {
        self.output_to_node.contains_key(output)
    }

    /// Returns groups of rule names that can execute in parallel.
    ///
    /// Each group contains rules whose dependencies have all been satisfied
    /// by rules in previous groups. This is computed by assigning each node
    /// a "depth" equal to the length of the longest path from any root node.
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
}
