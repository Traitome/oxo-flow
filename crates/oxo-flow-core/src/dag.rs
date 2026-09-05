#![allow(deprecated)]
//! DAG (Directed Acyclic Graph) engine for workflow execution.
//!
//! Constructs a DAG from workflow rules by matching rule outputs to downstream
//! rule inputs. Provides topological sorting, cycle detection, and DOT format
//! export for visualization.

use crate::error::{OxoFlowError, Result};
use crate::rule::{FilePatterns, Rule};
use petgraph::algo::toposort;
use petgraph::dot::{Config, Dot};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::NodeRef;
use regex::Regex;
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

    /// Map from output file pattern to EVERY rule that produces it — two
    /// rules may declare the same output string (shared staging/bins
    /// directories, multi-tool fan-ins). Collapsing to one producer
    /// silently dropped the other's exact-match edges.
    output_to_node: HashMap<String, Vec<NodeIndex>>,
}

impl WorkflowDag {
    /// Build a DAG from a list of rules.
    ///
    /// Edges are created by matching rule outputs to downstream rule inputs.
    /// Returns an error if a cycle is detected or if duplicate rule names exist.
    #[must_use = "building a DAG returns a Result that must be used"]
    pub fn from_rules(rules: &[Rule]) -> Result<Self> {
        Self::from_rules_with_config(rules, &HashMap::new())
    }

    /// Build a DAG from a list of rules, expanding `{config.x}` placeholders
    /// in input/output paths against the provided config values before edge
    /// matching.
    ///
    /// Rules frequently express the same logical path through different
    /// config keys (`{config.umap_n_neighbors}` vs `{config.leiden_n_neighbors}`),
    /// or the same key at different nesting — without expansion those inputs
    /// never match any producer and the edge is silently lost (live evidence:
    /// the unsupervised workflow scheduled every leiden rule before its
    /// umap_graph producer).
    #[must_use = "building a DAG returns a Result that must be used"]
    pub fn from_rules_with_config(
        rules: &[Rule],
        config_values: &HashMap<String, String>,
    ) -> Result<Self> {
        let expand = |path: &str| {
            crate::executor::expand_to_fixed_point(path, config_values, |value| value.to_owned())
        };
        let mut graph = DiGraph::new();
        let mut name_to_node = HashMap::new();
        let mut output_to_node: HashMap<String, Vec<NodeIndex>> = HashMap::new();
        // Strings claimed via `output_pattern` (issue #296). They register
        // producer-side for exact matching, but are EXCLUDED from the
        // template matchers below: a raw pattern like `refs/{build}/bt2.gz`
        // regex-matches arbitrary concrete inputs (`refs/legacy/bt2.gz`)
        // that no dataflow connects, and the fabricated edges can serialize
        // unrelated rules or fabricate cycles.
        let mut pattern_claims: HashSet<String> = HashSet::new();

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

            // Register outputs — every producer, not just the last (a
            // shared output string must link ALL of its producers),
            // config-expanded so inputs referencing the same path through
            // a different config key still match.
            for output in &rule.output {
                output_to_node.entry(expand(output)).or_default().push(node);
            }
            // Issue #296: an output_pattern is a producer-side declaration
            // too. Registering it lets pattern-only producers (no `output`
            // entries) link their consumers: exact matches at template
            // level, and the expand_inputs pass below matches consumer
            // patterns against the baked instance patterns.
            if let Some(ref op) = rule.output_pattern {
                let claimed = expand(op);
                pattern_claims.insert(claimed.clone());
                output_to_node.entry(claimed).or_default().push(node);
            }
        }

        // Step 2: Add edges based on input/output matching.
        //
        // Beyond exact template-level string matching, real workflows (wave-2
        // community ports) contain inputs that only resolve to producer
        // outputs after expansion:
        // - `expand_inputs` materializes concrete paths at build time, but
        //   producer outputs may still be template-level
        //   (`variants/NA12878.g.vcf.gz` vs `variants/{sample}.g.vcf.gz`);
        // - glob inputs (`mapped/*.bam`) never exact-match concrete outputs;
        // - directory inputs (`data`, `data/`) depend on every producer
        //   writing anywhere under the directory.
        //
        // All inference below is string-based (the DAG has no workdir) and
        // strictly best-effort: anything that cannot be resolved keeps the
        // legacy behavior (no edge), never an error.
        let producer_outputs: Vec<(String, NodeIndex)> = output_to_node
            .iter()
            .flat_map(|(output, nodes)| nodes.iter().map(|&n| (output.clone(), n)))
            .collect();
        // Pre-compile one matcher per template-level output (e.g.
        // `variants/{sample}.g.vcf.gz` → `^variants/(?P<sample>\S+)\.g\.vcf\.gz$`).
        // Outputs referencing `{config.x}` cannot compile a valid regex group
        // name — those are skipped (None) and simply never match.
        // Output_pattern claims stay out (see `pattern_claims`).
        let template_matchers: Vec<(String, Option<Regex>)> = producer_outputs
            .iter()
            .filter(|(output, _)| !pattern_claims.contains(output))
            .map(|(output, _)| {
                let matcher = if output.contains('{') {
                    crate::wildcard::pattern_to_regex(output).ok()
                } else {
                    None
                };
                (output.clone(), matcher)
            })
            .collect();

        for rule in rules {
            let consumer_node = name_to_node[&rule.name];

            // Declared directory inputs (`FilePatterns::Dir`) — the input
            // iterator yields the directory path; prefix-match it against
            // every producer output, optionally restricted by the filter glob.
            let declared_dir: Option<(String, Option<String>)> = match &rule.input {
                FilePatterns::Dir { path, pattern } => Some((expand(path), pattern.clone())),
                _ => None,
            };

            // String-based inference for one input path. All steps are
            // strictly best-effort: anything unresolvable keeps the legacy
            // behavior (no edge), never an error.
            let infer = |input: &str,
                         graph: &mut DiGraph<DagNode, ()>,
                         declared_dir: Option<&(String, Option<String>)>| {
                // Config placeholders are expanded before matching so the
                // same logical path expressed through different config keys
                // still connects (see from_rules_with_config).
                let input = expand(input);
                // 1. Exact template-level match (legacy behavior, kept first).
                //    Every producer declaring the same output string links —
                //    shared-directory outputs must order ALL writers before
                //    any consumer of the directory.
                if let Some(producers) = output_to_node.get(&input) {
                    for &producer_node in producers {
                        add_edge_dedup(graph, producer_node, consumer_node);
                    }
                }

                if let Some((dir_path, filter)) = declared_dir {
                    // 2. Declared directory: any output under the directory is
                    //    a dependency. Multiple producers → all edges
                    //    (conservative correctness).
                    let base = dir_path.trim_end_matches('/');
                    let prefix = format!("{base}/");
                    let filter_re = filter.as_deref().and_then(glob_pattern_to_regex);
                    for (output, producer_node) in &producer_outputs {
                        if let Some(suffix) = output.strip_prefix(&prefix)
                            && filter_re.as_ref().is_none_or(|re| re.is_match(suffix))
                        {
                            add_edge_dedup(graph, *producer_node, consumer_node);
                        }
                    }
                } else if has_glob_chars(&input) {
                    // 3. Glob input (`mapped/*.bam`): compile the glob and
                    //    match it against producer outputs. A glob that
                    //    cannot be compiled (unbalanced bracket, …) keeps the
                    //    legacy behavior — no edges, no error.
                    if let Some(glob_re) = glob_pattern_to_regex(&input) {
                        for (output, producer_node) in &producer_outputs {
                            if glob_re.is_match(output) {
                                add_edge_dedup(graph, *producer_node, consumer_node);
                            }
                        }
                    }
                } else if !input.contains('{') {
                    // 4. Concrete path (no engine wildcards): try
                    //    template-level producer outputs first
                    //    (`variants/{sample}.g.vcf.gz` covers
                    //    `variants/NA12878.g.vcf.gz`), then the directory
                    //    heuristic for extension-less inputs.
                    for (output, matcher) in &template_matchers {
                        if let Some(re) = matcher
                            && re.is_match(&input)
                            && let Some(producers) = output_to_node.get(output)
                        {
                            for &producer_node in producers {
                                add_edge_dedup(graph, producer_node, consumer_node);
                            }
                        }
                    }
                    if looks_like_directory(&input) {
                        let base = input.trim_end_matches('/');
                        let prefix = format!("{base}/");
                        for (output, producer_node) in &producer_outputs {
                            if output.starts_with(&prefix) {
                                add_edge_dedup(graph, *producer_node, consumer_node);
                            }
                        }
                    }
                }
                // If no producer found, the input is assumed to be a source file
            };

            for input in rule.input.iter() {
                infer(input, &mut graph, declared_dir.as_ref());
            }

            // expand_inputs patterns declare dataflow that only materializes
            // after wildcard expansion — the runtime path injects the
            // resolved paths into rule.input and the edges above catch them,
            // but the TEMPLATE-level graph (`graph -f dot`, the catalog's
            // source) never runs expansion. Without this pass every
            // expand_inputs dependency is invisible and multiqc-style
            // aggregators look disconnected from their contributors.
            // Patterns are matched RAW (wildcards preserved), which lines up
            // exactly with producer template outputs; patterns that only
            // match after variable substitution fall through to the
            // template-matcher in step 4 (best-effort).
            for exp_input in &rule.expand_inputs {
                infer(&exp_input.pattern, &mut graph, None);
            }

            // Step 2b: Add edges for explicit depends_on (deduplicated
            // against edges already inferred from input/output matching).
            for dep_name in &rule.depends_on {
                if let Some(&dep_node) = name_to_node.get(dep_name) {
                    add_edge_dedup(&mut graph, dep_node, consumer_node);
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
                Err(OxoFlowError::CycleDetected { details: path_str })
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
                let cycle_start = stack.iter().position(|&n| n == neighbor).unwrap_or(0);
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
                Err(OxoFlowError::CycleDetected { details: path_str })
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

    /// Returns rule names in topological order for a subset of target rules and
    /// all of their transitive dependencies.
    ///
    /// This enables running only part of a workflow — similar to `make <target>`
    /// or `just <recipe>`. The returned list always includes the specified
    /// targets **and every upstream rule they transitively depend on**, in a
    /// valid execution order.
    ///
    /// If `targets` is empty the full execution order is returned (same as
    /// [`Self::execution_order`]).
    ///
    /// # Errors
    ///
    /// Returns [`OxoFlowError::RuleNotFound`] if any target name does not exist
    /// in the DAG.
    #[must_use = "execution ordering returns a Result that must be used"]
    /// Returns the topological execution order for a subset of target rules.
    ///
    /// This includes the targets themselves and all their transitive upstream
    /// dependencies.
    pub fn execution_order_for_targets(&self, targets: &[&str]) -> Result<Vec<String>> {
        if targets.is_empty() {
            return self.execution_order();
        }

        // Validate all target names first, with prefix matching for convenience
        let mut resolved_targets: Vec<String> = Vec::new();
        for &target in targets {
            if self.name_to_node.contains_key(target) {
                resolved_targets.push(target.to_string());
            } else {
                // Try prefix matching: find all rules whose names start with the target
                let matches: Vec<&String> = self
                    .name_to_node
                    .keys()
                    .filter(|name| name.starts_with(target))
                    .collect();
                if matches.is_empty() {
                    // Collect available rule base names (before _expansion suffix)
                    let base_names: Vec<&str> =
                        self.name_to_node.keys().map(|s| s.as_str()).collect();
                    return Err(OxoFlowError::RuleNotFound {
                        name: target.to_string(),
                        available_rules: base_names.into_iter().map(String::from).collect(),
                    });
                }
                for m in matches {
                    resolved_targets.push(m.clone());
                }
            }
        }

        // Transitive dependency collection using BFS/DFS
        let mut included: HashSet<NodeIndex> = HashSet::new();
        let mut stack: Vec<NodeIndex> = resolved_targets
            .iter()
            .map(|t| self.name_to_node.get(t.as_str()).copied().unwrap())
            .collect();

        while let Some(node) = stack.pop() {
            if included.insert(node) {
                for dep in self
                    .graph
                    .neighbors_directed(node, petgraph::Direction::Incoming)
                {
                    if !included.contains(&dep) {
                        stack.push(dep);
                    }
                }
            }
        }

        // Optimization: Use a cached or pre-calculated topological order if possible.
        // For now, we still calculate it, but we filter it efficiently.
        let full_order = self.topological_order()?;

        Ok(full_order
            .into_iter()
            .filter(|n| {
                self.name_to_node
                    .get(&n.name)
                    .map(|&idx| included.contains(&idx))
                    .unwrap_or(false)
            })
            .map(|n| n.name.clone())
            .collect())
    }

    /// [`Self::execution_order_for_targets`] with a `skip` set: the
    /// closure traverses the INSTANTIATED DAG (issue #247) — nodes whose
    /// `when` evaluated false never enter the execution set, and the
    /// closure does not expand through them (their upstream exists only
    /// to feed them).
    ///
    /// Dead-node propagation: a surviving node with a pruned DAG parent is
    /// un-runnable (its input comes from a variant that never executes —
    /// the executor would fail it on the missing file), so it is pruned
    /// too, transitively to a fixpoint.
    ///
    /// - A when-false node that is also an explicit target is REPORTED via
    ///   the returned `Vec` of skipped target names (the caller warns) and
    ///   excluded from the order: `-t <name>` on a never-executing variant
    ///   must say so, not silently plan a run that cannot produce output.
    /// - A when-false non-target producer is silently pruned together with
    ///   its upstream — the mutual-exclusion variant that did not match
    ///   contributes nothing.
    ///
    /// Prefix matching resolves entries exactly like the unfiltered method;
    /// the filter applies AFTER resolution, so an entry that matches only
    /// pruned variants reports every pruned match by instance name.
    pub fn execution_order_for_targets_skipping(
        &self,
        targets: &[&str],
        skip: &HashSet<String>,
    ) -> Result<(Vec<String>, Vec<String>)> {
        if targets.is_empty() {
            return Ok((self.execution_order()?, Vec::new()));
        }

        // Resolve targets (same validation + prefix matching as the
        // unfiltered method).
        let mut resolved_targets: Vec<String> = Vec::new();
        for &target in targets {
            if self.name_to_node.contains_key(target) {
                resolved_targets.push(target.to_string());
            } else {
                let matches: Vec<&String> = self
                    .name_to_node
                    .keys()
                    .filter(|name| name.starts_with(target))
                    .collect();
                if matches.is_empty() {
                    let base_names: Vec<&str> =
                        self.name_to_node.keys().map(|s| s.as_str()).collect();
                    return Err(OxoFlowError::RuleNotFound {
                        name: target.to_string(),
                        available_rules: base_names.into_iter().map(String::from).collect(),
                    });
                }
                for m in matches {
                    resolved_targets.push(m.clone());
                }
            }
        }

        // Dead-node propagation to a fixpoint: a node whose parent is
        // pruned cannot receive its input, so it is pruned as well (the
        // executor fails such a rule on the missing file — surfacing that
        // at plan time is the point of the instantiated-DAG closure).
        let mut pruned: HashSet<String> = skip.clone();
        loop {
            let mut grew = false;
            for idx in self.graph.node_indices() {
                let name = &self.graph[idx].name;
                if pruned.contains(name) {
                    continue;
                }
                let has_pruned_parent = self
                    .graph
                    .neighbors_directed(idx, petgraph::Direction::Incoming)
                    .any(|dep| pruned.contains(&self.graph[dep].name));
                if has_pruned_parent && pruned.insert(name.clone()) {
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }

        let skipped_targets: Vec<String> = resolved_targets
            .iter()
            .filter(|t| pruned.contains(*t))
            .cloned()
            .collect();

        // Closure over SURVIVING nodes only: start from the non-pruned
        // targets, expand incoming edges but do not include or traverse
        // through pruned nodes.
        let mut included: HashSet<NodeIndex> = HashSet::new();
        let mut stack: Vec<NodeIndex> = resolved_targets
            .iter()
            .filter(|t| !pruned.contains(*t))
            .map(|t| self.name_to_node.get(t.as_str()).copied().unwrap())
            .collect();

        while let Some(node) = stack.pop() {
            let name = self.graph[node].name.clone();
            if pruned.contains(&name) {
                continue;
            }
            if included.insert(node) {
                for dep in self
                    .graph
                    .neighbors_directed(node, petgraph::Direction::Incoming)
                {
                    if !included.contains(&dep) {
                        stack.push(dep);
                    }
                }
            }
        }

        let full_order = self.topological_order()?;
        let order = full_order
            .into_iter()
            .filter(|n| {
                self.name_to_node
                    .get(&n.name)
                    .map(|&idx| included.contains(&idx))
                    .unwrap_or(false)
            })
            .map(|n| n.name.clone())
            .collect();
        Ok((order, skipped_targets))
    }

    /// Returns the direct dependencies (upstream rules) for a given rule.
    #[must_use = "querying dependencies returns a Result that must be used"]
    pub fn dependencies(&self, rule_name: &str) -> Result<Vec<String>> {
        let node = self
            .name_to_node
            .get(rule_name)
            .ok_or(OxoFlowError::RuleNotFound {
                name: rule_name.to_string(),
                available_rules: self.name_to_node.keys().cloned().collect(),
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
                available_rules: self.name_to_node.keys().cloned().collect(),
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

    /// Export the DAG as a plain Mermaid `graph LR` definition.
    ///
    /// Unlike [`Self::to_metro`], this emits standard Mermaid only (no
    /// `%%metro` directives), so it renders directly on GitHub, in VS Code,
    /// and in any Mermaid-compatible renderer without nf-metro.
    pub fn to_mermaid(&self) -> String {
        let mut out = String::from("graph LR\n");
        for node in self.nodes_by_rule_index() {
            out.push_str(&format!(
                "    n{}[\"{}\"]\n",
                node.index(),
                sanitize_mermaid_label(&self.graph[node].name)
            ));
        }
        for (src, dst) in self.sorted_edges() {
            out.push_str(&format!("    n{} --> n{}\n", src.index(), dst.index()));
        }
        out
    }

    /// Export the DAG as an nf-metro "metro map" definition.
    ///
    /// Output is a Mermaid `graph LR` subset extended with `%%metro`
    /// directives (colored lines and stage sections), renderable to a
    /// transit-map-style SVG by
    /// [`nf-metro`](https://github.com/seqeralabs/nf-metro):
    ///
    /// ```text
    /// oxo-flow graph workflow.oxoflow -f metro -o workflow.mmd
    /// nf-metro render workflow.mmd -o workflow.svg
    /// ```
    ///
    /// Each rule becomes a station (its `module::` prefix stripped — the
    /// section already names the group); each dependency becomes an edge
    /// carrying the *source* rule's stage line. Stages are inferred per rule
    /// (see [`crate::stage`]) and become colored lines that flow through
    /// multiple sections — the nf-core transit-map structure — while sections
    /// follow the workflow's module namespaces in data-flow order.
    ///
    pub fn to_metro(&self, rules: &[Rule]) -> Result<String> {
        let nodes = self.nodes_by_rule_index();
        let edges = self.sorted_edges();

        // Two orthogonal groupings, mirroring nf-core's transit maps:
        // - the LINE is the rule's stage (tags → module prefix → shell
        //   keywords → generic) — the colored track an edge flows along;
        // - the SECTION is the workflow's own module namespace
        //   (`module::rule`), falling back to the stage for prefixless
        //   rules. Sections follow the workflow file's data-flow order, so
        //   the section graph is acyclic by construction — no cycle
        //   demotion (which previously flooded "generic" on real
        //   pipelines, live: community rnaseq).
        let mut node_stage: HashMap<NodeIndex, String> = HashMap::new();
        let mut node_section: HashMap<NodeIndex, String> = HashMap::new();
        for node in &nodes {
            let stage = rules
                .get(self.graph[*node].rule_index)
                .map(crate::stage::detect_stage)
                .unwrap_or_else(|| "generic".to_string());
            let section = self.graph[*node]
                .name
                .split_once("::")
                .map(|(prefix, _)| prefix.to_string())
                .unwrap_or_else(|| stage.clone());
            node_stage.insert(*node, stage);
            node_section.insert(*node, section);
        }

        // Sections and stages in first-appearance order (deterministic).
        // A section's display comes from the module it groups, or from the
        // stage itself for prefixless rules (custom stages keep their
        // sanitized raw name).
        let mut sections: Vec<(String, String)> = Vec::new();
        for node in &nodes {
            let section = &node_section[node];
            if sections.iter().any(|(s, _)| s == section) {
                continue;
            }
            let display = self.graph[*node]
                .name
                .split_once("::")
                .map(|(prefix, _)| crate::stage::module_display(prefix))
                .unwrap_or_else(|| crate::stage::stage_display(&node_stage[node]));
            sections.push((section.clone(), display));
        }
        let mut stages: Vec<String> = Vec::new();
        for node in &nodes {
            let stage = &node_stage[node];
            if !stages.iter().any(|s| s == stage) {
                stages.push(stage.clone());
            }
        }

        let mut out = String::new();

        // Line definitions.
        for stage in &stages {
            out.push_str(&format!(
                "%%metro line: {} | {} | {}\n",
                sanitize_metro_id(stage),
                sanitize_mermaid_text(&crate::stage::stage_display(stage)),
                crate::stage::stage_color(stage),
            ));
        }
        out.push('\n');
        out.push_str("graph LR\n");

        let multi_section = sections.len() > 1;
        let inner_indent = if multi_section { "        " } else { "    " };

        // Stations grouped into flow-ordered sections; intra-section edges
        // inside their section, inter-section edges after every `end`
        // (nf-metro rule).
        let mut inter_section_edges: Vec<(NodeIndex, NodeIndex)> = Vec::new();

        for (section, display) in &sections {
            if multi_section {
                out.push_str(&format!(
                    "    subgraph {} [{}]\n",
                    sanitize_metro_id(section),
                    sanitize_mermaid_text(display)
                ));
            }

            for node in nodes.iter().filter(|n| &node_section[n] == section) {
                out.push_str(&format!(
                    "{inner_indent}n{}[\"{}\"]\n",
                    node.index(),
                    sanitize_mermaid_label(metro_station_label(&self.graph[*node].name))
                ));
            }

            for &(src, dst) in &edges {
                if node_section[&src] != *section {
                    continue;
                }
                if node_section[&dst] == *section {
                    // Intra-section edge — inside this section.
                    out.push_str(&format!(
                        "{inner_indent}n{} -->|{}| n{}\n",
                        src.index(),
                        sanitize_metro_id(&node_stage[&src]),
                        dst.index()
                    ));
                } else {
                    // Edge leaving this section — emit after all sections.
                    inter_section_edges.push((src, dst));
                }
            }

            if multi_section {
                out.push_str("    end\n");
            }
        }

        // Inter-section edges.
        for (src, dst) in inter_section_edges {
            out.push_str(&format!(
                "    n{} -->|{}| n{}\n",
                src.index(),
                sanitize_metro_id(&node_stage[&src]),
                dst.index()
            ));
        }

        Ok(out)
    }

    /// Node indices ordered by the rule's position in the workflow file —
    /// deterministic output for tests and diffs.
    fn nodes_by_rule_index(&self) -> Vec<NodeIndex> {
        let mut nodes: Vec<NodeIndex> = self.graph.node_indices().collect();
        nodes.sort_by_key(|&n| self.graph[n].rule_index);
        nodes
    }

    /// Edge endpoints ordered by `(source rule index, target rule index)`.
    fn sorted_edges(&self) -> Vec<(NodeIndex, NodeIndex)> {
        let mut edges: Vec<(NodeIndex, NodeIndex)> = self
            .graph
            .edge_indices()
            .map(|e| self.graph.edge_endpoints(e).expect("edge endpoints exist"))
            .collect();
        edges.sort_by_key(|&(s, d)| (self.graph[s].rule_index, self.graph[d].rule_index));
        edges
    }

    /// Returns the name of the first rule producing an output pattern, if
    /// any. (Shared output strings may have several producers — see
    /// `producers_of`.)
    #[must_use]
    pub fn producer_of(&self, output: &str) -> Option<&str> {
        self.output_to_node
            .get(output)
            .and_then(|nodes| nodes.first())
            .map(|&node| self.graph[node].name.as_str())
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

/// Add a producer → consumer edge unless one already exists.
///
/// Input matching can reach the same producer through several paths (exact
/// match, glob, directory prefix, template pattern, `depends_on`) — parallel
/// edges would corrupt edge counts and metrics. A rule never depends on
/// itself: write-back-into-input-directory patterns (e.g. a quast/multiqc
/// summary that reads a directory and writes its report into it) make the
/// directory-prefix inference match the rule's own output — self-edges are
/// dropped rather than reported as cycles (live evidence: mag's
/// `concat_quast` reads `QC/` and writes `QC/quast_bin_summary.tsv`).
fn add_edge_dedup(graph: &mut DiGraph<DagNode, ()>, from: NodeIndex, to: NodeIndex) {
    if from != to && graph.find_edge(from, to).is_none() {
        graph.add_edge(from, to, ());
    }
}

/// Literal glob characters — distinct from `{engine}` wildcards
/// (`crate::wildcard::has_wildcards` only matches braces).
fn has_glob_chars(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?') || pattern.contains('[')
}

/// Convert a filesystem glob (`mapped/*.bam`, `reads/S?.fq`) into a regex.
///
/// `*` and `?` never cross `/` (glob semantics). Character classes `[...]`
/// pass through with the glob `!` negation translated to regex `^`. Returns
/// `None` for patterns that cannot compile — callers then keep the legacy
/// behavior (no edge).
fn glob_pattern_to_regex(pattern: &str) -> Option<Regex> {
    let mut re = String::with_capacity(pattern.len() + 8);
    re.push('^');
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => {
                re.push_str("[^/]*");
                i += 1;
            }
            '?' => {
                re.push_str("[^/]");
                i += 1;
            }
            '[' => {
                i += 1;
                if i < chars.len() && chars[i] == '!' {
                    re.push('^');
                    i += 1;
                }
                let mut closed = false;
                while i < chars.len() && chars[i] != ']' {
                    re.push(chars[i]);
                    i += 1;
                }
                if i < chars.len() {
                    closed = true;
                    i += 1;
                }
                re.push(']');
                if !closed {
                    return None; // unbalanced bracket — unresolvable
                }
            }
            c => {
                re.push_str(&regex::escape(&c.to_string()));
                i += 1;
            }
        }
    }
    re.push('$');
    Regex::new(&re).ok()
}

/// File extensions that mark a concrete input path as a file, not a
/// directory. Anything else — no extension at all, or an unknown one — is
/// treated as a directory candidate so that directory inputs form edges to
/// every producer writing under them. Missing an edge causes a race; an
/// extra conservative edge only serializes execution slightly.
const KNOWN_FILE_EXTENSIONS: &[&str] = &[
    "fa", "fasta", "fq", "fastq", "txt", "tsv", "csv", "json", "yaml", "yml", "toml", "bam", "sam",
    "cram", "vcf", "bcf", "bed", "gff", "gff3", "gtf", "gbff", "dict", "bai", "fai", "tbi", "csi",
    "png", "pdf", "html", "htm", "log", "md", "rst", "zip", "tar", "gz", "bz2", "xz", "zst",
];

/// Heuristic: does this concrete input path refer to a directory?
///
/// A declared [`FilePatterns::Dir`] input is always a directory; plain
/// strings fall back to this check: an explicit trailing slash, or a last
/// path component without a known file extension.
fn looks_like_directory(path: &str) -> bool {
    if path.ends_with('/') {
        return true;
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return false;
    }
    let basename = trimmed.rsplit('/').next().unwrap_or(trimmed);
    if basename == "." || basename == ".." {
        return false;
    }
    match basename.rfind('.') {
        None => true, // no extension at all → directory candidate
        Some(pos) => {
            let ext = &basename[pos + 1..];
            !KNOWN_FILE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
        }
    }
}

/// Sanitize a stage name into a safe nf-metro/Mermaid identifier (line ID or
/// section ID): non-alphanumeric characters become underscores.
fn sanitize_metro_id(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        out = "stage".to_string();
    }
    out
}

/// Sanitize text that appears inside a Mermaid quoted node label
/// (`["text"]`). Replaces syntax meta-characters with safe, readable
/// alternatives so output renders reliably in both standard Mermaid and
/// nf-metro (which does not unescape HTML entities).
fn sanitize_mermaid_label(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '"' => '\'',
            '[' => '(',
            ']' => ')',
            '|' => '/',
            '\n' | '\r' => ' ',
            c => c,
        })
        .collect()
}

/// Station label for the metro map: strip the `module::` namespace prefix —
/// the section already names the group, so "alignment::star_align" reads
/// "star_align" (nf-core transit-map labels are short process names).
fn metro_station_label(name: &str) -> &str {
    name.split_once("::").map_or(name, |(_, rest)| rest)
}

/// Sanitize text that appears in metro `%%metro line` directives or
/// `subgraph` titles. Brackets are stripped to spaces because subgraph
/// titles do not tolerate parentheses, while pipes are replaced with
/// slashes so the field separators stay intact.
fn sanitize_mermaid_text(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '"' => '\'',
            '[' | ']' => ' ',
            '|' => '/',
            '\n' | '\r' => ' ',
            c => c,
        })
        .collect()
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
    /// This produces more visually informative output than [`Self::to_dot()`], with:
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
                let parent_name = &self.graph[parent].name;
                match parent_d.cmp(&best_depth) {
                    std::cmp::Ordering::Greater => {
                        best_depth = parent_d;
                        best_parent = Some(parent);
                    }
                    std::cmp::Ordering::Equal => {
                        // Deterministic tiebreaker: prefer alphabetically first name.
                        if let Some(ref current_best) = best_parent
                            && parent_name < &self.graph[*current_best].name
                        {
                            best_parent = Some(parent);
                        }
                    }
                    std::cmp::Ordering::Less => {}
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

                // Get dependencies for this rule (deduplicated).
                let mut deps = self.dependencies(rule)?;
                deps.sort();
                deps.dedup();
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
            let mut deps = self.dependencies(rule_name)?;
            deps.sort();
            deps.dedup();
            let dep_str = if deps.is_empty() {
                " ──●".to_string()
            } else {
                format!(" ──● [{}]", deps.join(", "))
            };

            // Draw position indicator
            output.push_str(&format!("{:3}. {}{}\n", i + 1, rule_name, dep_str));

            // Show downstream if exists (deduplicated)
            let mut downstream = self.dependents(rule_name)?;
            downstream.sort();
            downstream.dedup();
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
    fn execution_order_for_targets_leaf_only() {
        // a -> b -> c; targeting "c" should return all three.
        let rules = vec![
            make_rule("a", vec!["in.txt"], vec!["mid1.txt"]),
            make_rule("b", vec!["mid1.txt"], vec!["mid2.txt"]),
            make_rule("c", vec!["mid2.txt"], vec!["out.txt"]),
        ];

        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let order = dag.execution_order_for_targets(&["c"]).unwrap();
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    #[test]
    fn execution_order_for_targets_mid_rule() {
        // a -> b -> c; targeting "b" should return only ["a", "b"].
        let rules = vec![
            make_rule("a", vec!["in.txt"], vec!["mid1.txt"]),
            make_rule("b", vec!["mid1.txt"], vec!["mid2.txt"]),
            make_rule("c", vec!["mid2.txt"], vec!["out.txt"]),
        ];

        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let order = dag.execution_order_for_targets(&["b"]).unwrap();
        assert_eq!(order, vec!["a", "b"]);
    }

    #[test]
    fn execution_order_for_targets_multiple() {
        // Diamond: source -> left, source -> right -> merge
        // Targeting ["left", "right"] should include source + left + right (not merge).
        let rules = vec![
            make_rule("source", vec!["raw.txt"], vec!["a.txt", "b.txt"]),
            make_rule("left", vec!["a.txt"], vec!["left.txt"]),
            make_rule("right", vec!["b.txt"], vec!["right.txt"]),
            make_rule("merge", vec!["left.txt", "right.txt"], vec!["final.txt"]),
        ];

        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let order = dag.execution_order_for_targets(&["left", "right"]).unwrap();
        assert!(order.contains(&"source".to_string()));
        assert!(order.contains(&"left".to_string()));
        assert!(order.contains(&"right".to_string()));
        assert!(!order.contains(&"merge".to_string()));
        // source must come before left and right
        let source_pos = order.iter().position(|s| s == "source").unwrap();
        let left_pos = order.iter().position(|s| s == "left").unwrap();
        let right_pos = order.iter().position(|s| s == "right").unwrap();
        assert!(source_pos < left_pos);
        assert!(source_pos < right_pos);
    }

    #[test]
    fn execution_order_for_targets_empty_returns_all() {
        let rules = vec![
            make_rule("a", vec!["in.txt"], vec!["mid.txt"]),
            make_rule("b", vec!["mid.txt"], vec!["out.txt"]),
        ];

        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let full = dag.execution_order().unwrap();
        let targeted = dag.execution_order_for_targets(&[]).unwrap();
        assert_eq!(full, targeted);
    }

    #[test]
    fn execution_order_for_targets_unknown_target() {
        let rules = vec![make_rule("a", vec![], vec!["out.txt"])];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let result = dag.execution_order_for_targets(&["nonexistent"]);
        assert!(result.is_err());
    }

    #[test]
    fn execution_order_for_targets_skipping_prunes_when_false_producers() {
        // Issue #247: the closure traverses the INSTANTIATED DAG — a
        // when-false producer is pruned, its upstream (which only feeds
        // it) is not pulled in, and consumers of the pruned variant's
        // output are dead too (their input can never exist).
        //
        // raw -> source -> a.txt -> trim_star (when-false) -.
        //                \-> b.txt -> trim_hisat (when-true) --> merge
        let rules = vec![
            make_rule("raw", vec!["in.txt"], vec!["src.txt"]),
            make_rule("source", vec!["src.txt"], vec!["a.txt", "b.txt"]),
            make_rule("trim_star", vec!["a.txt"], vec!["ts.txt"]),
            make_rule("trim_hisat", vec!["b.txt"], vec!["th.txt"]),
            make_rule("merge", vec!["ts.txt", "th.txt"], vec!["final.txt"]),
        ];
        let dag = WorkflowDag::from_rules(&rules).unwrap();

        let mut skip = std::collections::HashSet::new();
        skip.insert("trim_star".to_string());

        // Targeting the surviving variant's chain: raw -> source ->
        // trim_hisat, nothing from the dead branch.
        let (order, skipped_targets) = dag
            .execution_order_for_targets_skipping(&["trim_hisat"], &skip)
            .unwrap();
        assert!(skipped_targets.is_empty());
        assert!(!order.contains(&"trim_star".to_string()), "{order:?}");
        assert!(order.contains(&"trim_hisat".to_string()), "{order:?}");
        assert!(order.contains(&"source".to_string()), "{order:?}");
        assert!(order.contains(&"raw".to_string()), "{order:?}");

        // Targeting merge (reads BOTH variants' outputs): the star branch
        // is dead, so merge can never receive its ts.txt input — it is
        // dead-propagated and reported as a skipped target.
        let (order, skipped_targets) = dag
            .execution_order_for_targets_skipping(&["merge"], &skip)
            .unwrap();
        assert_eq!(skipped_targets, vec!["merge".to_string()]);
        assert!(order.is_empty(), "merge cannot run without ts.txt");
    }

    #[test]
    fn execution_order_for_targets_skipping_reports_when_false_target() {
        // A target that names a never-executing variant is reported (not
        // silently planned) and excluded together with its upstream.
        let rules = vec![
            make_rule("raw", vec!["in.txt"], vec!["src.txt"]),
            make_rule("trim_star", vec!["src.txt"], vec!["ts.txt"]),
            make_rule("trim_hisat", vec!["src.txt"], vec!["th.txt"]),
        ];
        let dag = WorkflowDag::from_rules(&rules).unwrap();

        let mut skip = std::collections::HashSet::new();
        skip.insert("trim_star".to_string());

        let (order, skipped_targets) = dag
            .execution_order_for_targets_skipping(&["trim_star"], &skip)
            .unwrap();
        assert_eq!(skipped_targets, vec!["trim_star".to_string()]);
        assert!(order.is_empty(), "only the skipped target was requested");

        // A prefix that matches both variants keeps only the survivor.
        let (order, skipped_targets) = dag
            .execution_order_for_targets_skipping(&["trim"], &skip)
            .unwrap();
        assert_eq!(skipped_targets, vec!["trim_star".to_string()]);
        assert!(order.contains(&"trim_hisat".to_string()));
        assert!(order.contains(&"raw".to_string()));
        assert!(!order.contains(&"trim_star".to_string()));
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
            output: vec!["aligned/{sample}.bam".to_string()].into(),
            ..Default::default()
        };
        let r2 = crate::rule::Rule {
            name: "sort".to_string(),
            output: vec!["sorted/{sample}.bam".to_string()].into(),
            ..Default::default()
        };
        let warnings = WorkflowDag::detect_output_collisions(&[r1, r2]);
        assert!(warnings.is_empty());
    }

    #[test]
    fn detect_output_collisions_found() {
        let r1 = crate::rule::Rule {
            name: "caller_a".to_string(),
            output: vec!["{sample}.vcf".to_string()].into(),
            ..Default::default()
        };
        let r2 = crate::rule::Rule {
            name: "caller_b".to_string(),
            output: vec!["{sample}.vcf".to_string()].into(),
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
                    input: crate::rule::FilePatterns::List(input),
                    output: vec![format!("step_{}.out", i)].into(),
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

    // ---- Post-expansion edge inference (wave 2-1) --------------------------
    //
    // Producer outputs may be template-level (`variants/{sample}.g.vcf.gz`)
    // while consumer inputs are concrete — expand_inputs expands lists at
    // workflow-build time, before per-instance wildcard expansion — so exact
    // string matching alone misses the dependency. Glob and directory inputs
    // never matched at all. The rules below pin the inference semantics.

    #[test]
    fn expand_inputs_concrete_input_matches_template_output() {
        // Producer output stays template-level (no sample_groups declared;
        // expand_inputs is driven by a config list), consumer input is the
        // concrete path expand_inputs materialized.
        let producer = make_rule("call_gvcf", vec![], vec!["variants/{sample}.g.vcf.gz"]);
        let consumer = make_rule(
            "combine_gvcfs",
            vec!["variants/NA12878.g.vcf.gz"],
            vec!["variants/cohort.g.vcf.gz"],
        );
        let dag = WorkflowDag::from_rules(&[producer, consumer]).unwrap();
        assert_eq!(
            dag.dependencies("combine_gvcfs").unwrap(),
            vec!["call_gvcf"]
        );
    }

    #[test]
    fn shared_output_string_links_every_producer() {
        // Two rules declaring the SAME output path (shared bins/staging
        // directory) must BOTH order the consumer — collapsing to the last
        // producer silently dropped the other's edge (mag binning module).
        let p1 = make_rule("metabat2_spades", vec![], vec!["{config.out_dir}/bins"]);
        let p2 = make_rule("metabat2_megahit", vec![], vec!["{config.out_dir}/bins"]);
        let consumer = make_rule("seqkit", vec!["{config.out_dir}/bins"], vec!["stats.tsv"]);
        let dag = WorkflowDag::from_rules(&[p1, p2, consumer]).unwrap();
        let mut deps = dag.dependencies("seqkit").unwrap();
        deps.sort();
        assert_eq!(deps, vec!["metabat2_megahit", "metabat2_spades"]);
        // producer_of returns one of the two.
        assert!(
            dag.producer_of("{config.out_dir}/bins") == Some("metabat2_megahit")
                || dag.producer_of("{config.out_dir}/bins") == Some("metabat2_spades")
        );
    }

    #[test]
    fn expand_inputs_pattern_links_consumer_at_template_level() {
        // The catalog renders the TEMPLATE-level graph (no expansion), so
        // dataflow declared only via expand_inputs (multiqc-style
        // aggregators) must still form edges there. Patterns carry the same
        // wildcard literals as producer outputs, so raw-string matching
        // lines them up exactly.
        let producer = make_rule(
            "fastqc_raw",
            vec![],
            vec!["{config.out_dir}/fastqc/raw/{sample}_raw_1_fastqc.zip"],
        );
        let mut consumer = make_rule("multiqc", vec![], vec!["multiqc_report.html"]);
        consumer.expand_inputs.push(crate::rule::ExpandConfig {
            pattern: "{config.out_dir}/fastqc/raw/{sample}_raw_1_fastqc.zip".to_string(),
            variables: [("sample".to_string(), "config.samples_list".to_string())]
                .into_iter()
                .collect(),
        });
        let dag = WorkflowDag::from_rules(&[producer, consumer]).unwrap();
        assert_eq!(dag.dependencies("multiqc").unwrap(), vec!["fastqc_raw"]);
    }

    #[test]
    fn output_pattern_registers_producer_side_for_template_matching() {
        // Issue #296: the output_pattern itself is a producer-side
        // declaration. A consumer's raw expand_inputs pattern carrying the
        // same wildcard literals exact-matches it — this is the edge the
        // template-level graph (the catalog's source) must show for
        // pattern-only producers.
        let mut producer = make_rule("assemble", vec![], vec![]);
        producer.output_pattern = Some("results/{assembler}/part.txt".to_string());
        let mut consumer = make_rule("summarize", vec![], vec!["results/summary.txt"]);
        consumer.expand_inputs.push(crate::rule::ExpandConfig {
            pattern: "results/{assembler}/part.txt".to_string(),
            variables: HashMap::new(),
        });
        let dag = WorkflowDag::from_rules(&[producer, consumer]).unwrap();
        assert_eq!(dag.dependencies("summarize").unwrap(), vec!["assemble"]);
    }

    #[test]
    fn expand_inputs_consumer_matches_baked_output_pattern_precisely() {
        // Issue #296: a [[values]]-fanned output_pattern producer bakes its
        // pattern per instance, and the expansion pass materializes a
        // values consumer's expand_inputs pattern into ITS OWN concrete
        // paths (the spades instance never sees megahit files). The baked
        // producer pattern must be registered producer-side so the
        // concrete consumer input exact-matches it — and only it: the
        // per-value edge precision pinned by #268 item 1 holds.
        let mut p1 = make_rule("assemble_assembler_spades", vec![], vec![]);
        p1.output_pattern = Some("results/spades/part.txt".to_string());
        let mut p2 = make_rule("assemble_assembler_megahit", vec![], vec![]);
        p2.output_pattern = Some("results/megahit/part.txt".to_string());
        let consumer = make_rule(
            "summarize_assembler_spades",
            vec!["results/spades/part.txt"],
            vec!["results/summary_spades.txt"],
        );
        let dag = WorkflowDag::from_rules(&[p1, p2, consumer]).unwrap();
        assert_eq!(
            dag.dependencies("summarize_assembler_spades").unwrap(),
            vec!["assemble_assembler_spades"]
        );
        // producer_of attributes the produced file to its pattern owner.
        assert_eq!(
            dag.producer_of("results/spades/part.txt"),
            Some("assemble_assembler_spades")
        );
    }

    #[test]
    fn output_pattern_claims_never_template_match_concrete_inputs() {
        // Issue #296 review finding: a raw output_pattern registered
        // producer-side must exact-match only. It must NOT join the
        // template matchers — `refs/{build}/bt2.gz` regex-matches any
        // concrete `refs/<x>/bt2.gz` input, and the fabricated edge can
        // serialize unrelated rules or fabricate a cycle.
        let mut producer = make_rule("index_ref", vec![], vec![]);
        producer.output_pattern = Some("refs/{build}/bt2.gz".to_string());
        let consumer = make_rule("align_legacy", vec!["refs/legacy/bt2.gz"], vec!["aln.bam"]);
        let dag = WorkflowDag::from_rules(&[producer, consumer]).unwrap();
        assert!(
            dag.dependencies("align_legacy").unwrap().is_empty(),
            "a pattern claim must not claim a concrete input it does not produce"
        );
    }

    #[test]
    fn glob_input_links_all_matching_producers() {
        let p1 = make_rule("align_s1", vec![], vec!["mapped/S1.bam"]);
        let p2 = make_rule("align_s2", vec![], vec!["mapped/S2.bam"]);
        let consumer = make_rule("merge", vec!["mapped/*.bam"], vec!["merged.bam"]);
        let dag = WorkflowDag::from_rules(&[p1, p2, consumer]).unwrap();
        let mut deps = dag.dependencies("merge").unwrap();
        deps.sort();
        assert_eq!(deps, vec!["align_s1", "align_s2"]);
    }

    #[test]
    fn glob_input_question_mark_matches_single_char() {
        let p = make_rule("produce", vec![], vec!["reads/S1_R1.fastq.gz"]);
        let c = make_rule("consume", vec!["reads/S?_R1.fastq.gz"], vec!["out.txt"]);
        let dag = WorkflowDag::from_rules(&[p, c]).unwrap();
        assert_eq!(dag.dependencies("consume").unwrap(), vec!["produce"]);
    }

    #[test]
    fn directory_input_links_all_producers_under_dir() {
        let p1 = make_rule("prep_a", vec![], vec!["data/a.txt"]);
        let p2 = make_rule("prep_b", vec![], vec!["data/sub/b.txt"]);
        let consumer = make_rule("analyze", vec!["data"], vec!["out.txt"]);
        let dag = WorkflowDag::from_rules(&[p1, p2, consumer]).unwrap();
        let mut deps = dag.dependencies("analyze").unwrap();
        deps.sort();
        assert_eq!(deps, vec!["prep_a", "prep_b"]);
    }

    #[test]
    fn directory_input_trailing_slash_is_directory() {
        let p = make_rule("produce", vec![], vec!["out/x.txt"]);
        let c = make_rule("consume", vec!["out/"], vec!["final.txt"]);
        let dag = WorkflowDag::from_rules(&[p, c]).unwrap();
        assert_eq!(dag.dependencies("consume").unwrap(), vec!["produce"]);
    }

    #[test]
    fn declared_dir_input_with_filter_links_only_matching_outputs() {
        use crate::rule::FilePatterns;
        let p1 = make_rule("prep_txt", vec![], vec!["data/a.txt"]);
        let p2 = make_rule("prep_bam", vec![], vec!["data/a.bam"]);
        let mut consumer = make_rule("analyze", vec![], vec!["out.txt"]);
        consumer.input = FilePatterns::Dir {
            path: "data".to_string(),
            pattern: Some("*.txt".to_string()),
        };
        let dag = WorkflowDag::from_rules(&[p1, p2, consumer]).unwrap();
        assert_eq!(dag.dependencies("analyze").unwrap(), vec!["prep_txt"]);
    }

    #[test]
    fn template_level_edges_unaffected_by_new_inference() {
        let step1 = make_rule("step1", vec!["input.txt"], vec!["mid.txt"]);
        let step2 = make_rule("step2", vec!["mid.txt"], vec!["output.txt"]);
        let dag = WorkflowDag::from_rules(&[step1, step2]).unwrap();
        assert_eq!(dag.edge_count(), 1);
        assert_eq!(dag.dependencies("step2").unwrap(), vec!["step1"]);
    }

    #[test]
    fn depends_on_duplicate_of_file_edge_is_deduplicated() {
        // step2 consumes mid.txt (file edge) AND declares depends_on = ["step1"]:
        // exactly one edge must exist.
        let step1 = make_rule("step1", vec!["input.txt"], vec!["mid.txt"]);
        let mut step2 = make_rule("step2", vec!["mid.txt"], vec!["output.txt"]);
        step2.depends_on = vec!["step1".to_string()];
        let dag = WorkflowDag::from_rules(&[step1, step2]).unwrap();
        assert_eq!(dag.edge_count(), 1);
        assert_eq!(dag.dependencies("step2").unwrap(), vec!["step1"]);
    }

    #[test]
    fn unresolvable_glob_keeps_old_behavior_no_edge() {
        // Unbalanced bracket — the glob cannot compile. Legacy behavior:
        // no edge, no error.
        let p = make_rule("prod", vec![], vec!["data/a.txt"]);
        let c = make_rule("cons", vec!["data/[.txt"], vec!["out.txt"]);
        let dag = WorkflowDag::from_rules(&[p, c]).unwrap();
        assert!(dag.dependencies("cons").unwrap().is_empty());
    }

    #[test]
    fn config_placeholder_paths_connect_when_values_align() {
        // The same logical path expressed through different config keys
        // (unsupervised: umap_n_neighbors vs leiden_n_neighbors) must form
        // an edge once both expand to the same string.
        let prod = make_rule(
            "umap",
            vec![],
            vec!["results/{config.umap_metric}_{config.umap_n_neighbors}_graph.pickle"],
        );
        let cons = make_rule(
            "leiden",
            vec!["results/{config.leiden_metric}_{config.leiden_n_neighbors}_graph.pickle"],
            vec!["out.csv"],
        );
        let values = HashMap::from([
            ("config.umap_metric".to_string(), "euclidean".to_string()),
            ("config.umap_n_neighbors".to_string(), "15".to_string()),
            ("config.leiden_metric".to_string(), "euclidean".to_string()),
            ("config.leiden_n_neighbors".to_string(), "15".to_string()),
        ]);
        let dag =
            WorkflowDag::from_rules_with_config(&[prod.clone(), cons.clone()], &values).unwrap();
        assert_eq!(dag.dependencies("leiden").unwrap(), vec!["umap"]);

        // Different values → no edge.
        let divergent = HashMap::from([
            ("config.umap_metric".to_string(), "euclidean".to_string()),
            ("config.umap_n_neighbors".to_string(), "15".to_string()),
            ("config.leiden_metric".to_string(), "cosine".to_string()),
            ("config.leiden_n_neighbors".to_string(), "15".to_string()),
        ]);
        let dag =
            WorkflowDag::from_rules_with_config(&[prod.clone(), cons.clone()], &divergent).unwrap();
        assert!(dag.dependencies("leiden").unwrap().is_empty());
    }

    #[test]
    fn write_back_into_input_dir_does_not_create_a_self_edge() {
        // A summary rule reads a directory and writes its report INTO it
        // (mag's concat_quast: input `QC/`, output `QC/quast_bin_summary.tsv`).
        // The directory-prefix inference must not make the rule depend on
        // itself — that would surface as a bogus cycle.
        let rule = make_rule(
            "concat_quast",
            vec!["{config.out_dir}/GenomeBinning/QC"],
            vec!["{config.out_dir}/GenomeBinning/QC/quast_bin_summary.tsv"],
        );
        let values = HashMap::from([("config.out_dir".to_string(), "results".to_string())]);
        let dag = WorkflowDag::from_rules_with_config(&[rule], &values).unwrap();
        assert!(dag.dependencies("concat_quast").unwrap().is_empty());
    }

    #[test]
    fn file_like_input_gets_no_directory_edges() {
        // Known extension → file semantics → exact edge only, no prefix edges.
        let p1 = make_rule("prod_a", vec![], vec!["refs/genome.fa"]);
        let p2 = make_rule("prod_b", vec![], vec!["refs/genes.gtf"]);
        let c = make_rule("cons", vec!["refs/genome.fa"], vec!["out.txt"]);
        let dag = WorkflowDag::from_rules(&[p1, p2, c]).unwrap();
        assert_eq!(dag.dependencies("cons").unwrap(), vec!["prod_a"]);
    }

    #[test]
    fn concrete_input_matches_template_output_across_dir() {
        // Template wildcard covers an intermediate directory too.
        let p = make_rule("prep", vec![], vec!["libraries/{lib}/reads.fq"]);
        let c = make_rule("quant", vec!["libraries/L1/reads.fq"], vec!["quant.txt"]);
        let dag = WorkflowDag::from_rules(&[p, c]).unwrap();
        assert_eq!(dag.dependencies("quant").unwrap(), vec!["prep"]);
    }

    #[test]
    fn non_matching_concrete_input_creates_no_edges() {
        // Concrete input that neither exact-matches nor fits any template
        // output, and looks like a file (known extension): no edges.
        let p = make_rule("prod", vec![], vec!["results/final.bam"]);
        let c = make_rule("cons", vec!["results/other.txt"], vec!["out.txt"]);
        let dag = WorkflowDag::from_rules(&[p, c]).unwrap();
        assert!(dag.dependencies("cons").unwrap().is_empty());
    }

    // ---- Mermaid / metro export tests -------------------------------------

    #[test]
    fn mermaid_export_has_nodes_and_edges() {
        let rules = vec![
            make_rule("a", vec!["in.txt"], vec!["mid.txt"]),
            make_rule("b", vec!["mid.txt"], vec!["out.txt"]),
        ];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let mmd = dag.to_mermaid();
        assert!(mmd.starts_with("graph LR\n"));
        assert!(mmd.contains("n0[\"a\"]"));
        assert!(mmd.contains("n1[\"b\"]"));
        assert!(mmd.contains("n0 --> n1"));
        // Plain Mermaid must not carry %%metro directives.
        assert!(!mmd.contains("%%metro"));
    }

    #[test]
    fn metro_export_single_stage_is_flat() {
        // `make_rule` uses `echo <name>` shells — no stage keywords — so both
        // rules collapse to the generic line and no section is emitted.
        let rules = vec![
            make_rule("a", vec!["in.txt"], vec!["mid.txt"]),
            make_rule("b", vec!["mid.txt"], vec!["out.txt"]),
        ];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let mmd = dag.to_metro(&rules).unwrap();
        assert!(mmd.contains("%%metro line: generic | Analysis | #79706E"));
        assert!(mmd.contains("graph LR"));
        assert!(mmd.contains("n0[\"a\"]"));
        assert!(mmd.contains("n0 -->|generic| n1"));
        assert!(!mmd.contains("subgraph"));
    }

    #[test]
    fn metro_export_module_namespaces_become_sections() {
        // `module::rule` namespaces form flow-ordered sections (the nf-core
        // structure), while the stage line flows through them; station
        // labels drop the redundant module prefix.
        let mut trim = make_rule("fastq_qc::trimgalore", vec!["reads.fq"], vec!["trimmed.fq"]);
        trim.shell = Some("trim_galore reads.fq".to_string());
        let mut align = make_rule(
            "alignment::star_align",
            vec!["trimmed.fq"],
            vec!["mapped.bam"],
        );
        align.shell = Some("STAR --runThreadN 8".to_string());
        let rules = vec![trim, align];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let mmd = dag.to_metro(&rules).unwrap();
        assert!(mmd.contains("subgraph fastq_qc [Read QC]"));
        assert!(mmd.contains("subgraph alignment [Alignment]"));
        // Station labels are the bare rule names (no `module::` prefix).
        assert!(mmd.contains("n0[\"trimgalore\"]"));
        assert!(mmd.contains("n1[\"star_align\"]"));
        // The cross-section edge carries the SOURCE rule's stage line.
        assert!(mmd.contains("n0 -->|qc| n1"));
    }

    #[test]
    fn metro_export_multi_stage_emits_sections() {
        let mut qc = make_rule("fastqc", vec!["reads.fq"], vec!["reads_fastqc.html"]);
        qc.shell = Some("fastqc reads.fq".to_string());
        let mut align = make_rule("align", vec!["reads.fq"], vec!["mapped.bam"]);
        align.shell = Some("bwa mem ref.fa reads.fq > mapped.sam".to_string());
        let rules = vec![qc, align];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let mmd = dag.to_metro(&rules).unwrap();
        assert!(mmd.contains("%%metro line: qc | Read QC | #4C78A8"));
        assert!(mmd.contains("%%metro line: align | Alignment | #54A24B"));
        assert!(mmd.contains("subgraph qc [Read QC]"));
        assert!(mmd.contains("subgraph align [Alignment]"));
    }

    #[test]
    fn metro_export_cross_stage_edge_follows_sections() {
        // qc → trim dependency: the edge must carry the source (qc) line and
        // appear after all `end` blocks (nf-metro's inter-section rule).
        let mut qc = make_rule("fastqc", vec!["reads.fq"], vec!["reads_fastqc.html"]);
        qc.shell = Some("fastqc reads.fq".to_string());
        let mut trim = make_rule("trim", vec!["reads_fastqc.html"], vec!["trimmed.fq"]);
        trim.shell = Some("fastp -i reads.fastq.gz".to_string());
        let rules = vec![qc, trim];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let mmd = dag.to_metro(&rules).unwrap();
        assert!(mmd.contains("n0 -->|qc| n1"));
        let last_end = mmd
            .rfind("    end\n")
            .expect("multi-stage map has sections");
        let edge_pos = mmd.rfind("-->|qc|").expect("cross-stage edge exists");
        assert!(
            edge_pos > last_end,
            "cross-stage edge must follow all `end` blocks:\n{mmd}"
        );
    }

    #[test]
    fn metro_export_explicit_tag_overrides_inference() {
        // The rule's `tags` categorize it as QC even though its shell says
        // `bwa` (an aligner) — explicit tags win over keyword inference.
        let mut rule = make_rule("align_as_qc", vec!["reads.fq"], vec!["out.bam"]);
        rule.shell = Some("bwa mem ref.fa reads.fq".to_string());
        rule.tags = vec!["qc".to_string()];
        let rules = vec![rule];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let mmd = dag.to_metro(&rules).unwrap();
        assert!(mmd.contains("%%metro line: qc | Read QC | #4C78A8"));
        assert!(!mmd.contains("%%metro line: align"));
    }

    #[test]
    fn mermaid_export_escapes_special_characters_in_rule_names() {
        let mut a = make_rule("a", vec![], vec!["mid.txt"]);
        a.name = "rule\"with\"quotes".to_string();
        let mut b = make_rule("b", vec!["mid.txt"], vec!["out.txt"]);
        b.name = "rule[bracket]".to_string();
        let rules = vec![a, b];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let mmd = dag.to_mermaid();
        // Double quotes must not terminate the label early.
        assert!(
            mmd.contains("n0[\"rule'with'quotes\"]"),
            "escaped rule name not found in:\n{mmd}"
        );
        // Brackets must not terminate the label early.
        assert!(
            mmd.contains("n1[\"rule(bracket)\"]"),
            "escaped bracket not found in:\n{mmd}"
        );
    }

    #[test]
    fn metro_export_escapes_special_characters_in_rule_and_stage_names() {
        let mut a = make_rule("a", vec![], vec!["mid.txt"]);
        a.name = "rule|with|pipes".to_string();
        a.tags = vec!["stage]bracket".to_string()];
        let mut b = make_rule("b", vec!["mid.txt"], vec!["out.txt"]);
        b.name = "rule\"quote\"".to_string();
        b.tags = vec!["stage|pipe".to_string()];
        let rules = vec![a, b];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let mmd = dag.to_metro(&rules).unwrap();
        // Line directive field separators must survive display names.
        assert!(
            mmd.contains("%%metro line: stage_bracket | stage bracket | "),
            "escaped stage bracket not in line directive:\n{mmd}"
        );
        assert!(
            mmd.contains("%%metro line: stage_pipe | stage/pipe | "),
            "escaped stage pipe not in line directive:\n{mmd}"
        );
        // Subgraph title brackets must not terminate early.
        assert!(
            mmd.contains("subgraph stage_bracket [stage bracket]"),
            "escaped subgraph title not found:\n{mmd}"
        );
        assert!(
            mmd.contains("subgraph stage_pipe [stage/pipe]"),
            "escaped subgraph pipe title not found:\n{mmd}"
        );
        // Node labels must escape quotes and pipes.
        assert!(
            mmd.contains("n0[\"rule/with/pipes\"]"),
            "escaped pipe in node label not found:\n{mmd}"
        );
        assert!(
            mmd.contains("n1[\"rule'quote'\"]"),
            "escaped quote in node label not found:\n{mmd}"
        );
    }

    #[test]
    fn metro_export_single_stage_indentation_is_consistent() {
        let rules = vec![
            make_rule("a", vec!["in.txt"], vec!["mid.txt"]),
            make_rule("b", vec!["mid.txt"], vec!["out.txt"]),
        ];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let mmd = dag.to_metro(&rules).unwrap();
        let lines: Vec<&str> = mmd.lines().collect();
        let station_line = lines
            .iter()
            .find(|l| l.contains("n0[\"a\"]"))
            .expect("station line exists");
        let edge_line = lines
            .iter()
            .find(|l| l.contains("n0 -->|generic| n1"))
            .expect("edge line exists");
        assert_eq!(
            station_line.len() - station_line.trim_start().len(),
            edge_line.len() - edge_line.trim_start().len(),
            "single-stage station and edge must share the same indentation:\n{mmd}"
        );
    }
}
