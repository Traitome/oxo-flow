//! Embedded bioinformatics pipeline knowledge graph.
//!
//! 78 workflow skills and 470 data-flow transitions (skill A → skill B,
//! annotated with the data types that pass between them: BAM, VCF, FASTQ,
//! COUNT_MATRIX, etc.) from the Pipette.bio SkillGraph — each edge backed
//! by literature paper counts.
//!
//! The graph answers "what feeds into X?" and "how do I get from A to B?"
//! for pipeline design. It is embedded at build time and queried on demand
//! — never injected wholesale into prompts.

use std::collections::HashMap;
use std::sync::LazyLock;

/// Compact JSONL: skill nodes {i,n,t,o} + edges {f,t,d,p}.
const GRAPH_RAW: &str = include_str!("pipeline_graph.jsonl");

/// One skill node in the pipeline graph.
#[derive(Debug, Clone)]
pub struct GraphNode {
    pub id: String,
    pub name: String,
    pub tools: String,
    pub overview: String,
}

/// One data-flow transition between skills.
#[derive(Debug, Clone)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    /// Data types flowing between the skills (e.g. "BAM, CRAM").
    pub data_types: String,
    /// Number of literature papers backing the edge.
    pub papers: u64,
}

struct Graph {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
}

impl Graph {
    fn new() -> Self {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        for line in GRAPH_RAW.lines() {
            let Ok(rec) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if rec.get("i").is_some() {
                nodes.push(GraphNode {
                    id: rec["i"].as_str().unwrap_or("").to_string(),
                    name: rec["n"].as_str().unwrap_or("").to_string(),
                    tools: rec["t"].as_str().unwrap_or("").to_string(),
                    overview: rec["o"].as_str().unwrap_or("").to_string(),
                });
            } else {
                edges.push(GraphEdge {
                    from: rec["f"].as_str().unwrap_or("").to_string(),
                    to: rec["t"].as_str().unwrap_or("").to_string(),
                    data_types: rec["d"].as_str().unwrap_or("").to_string(),
                    papers: rec["p"].as_u64().unwrap_or(0),
                });
            }
        }
        Self { nodes, edges }
    }
}

static GRAPH: LazyLock<Graph> = LazyLock::new(Graph::new);

/// Number of skill nodes and transitions in the embedded graph.
pub fn graph_stats() -> (usize, usize) {
    (GRAPH.nodes.len(), GRAPH.edges.len())
}

/// Look up a skill node by ID (case-insensitive) or name fragment.
pub fn find_node(query: &str) -> Option<&'static GraphNode> {
    let q = query.to_lowercase();
    GRAPH
        .nodes
        .iter()
        .find(|n| n.id.to_lowercase() == q || n.name.to_lowercase() == q)
        .or_else(|| {
            GRAPH
                .nodes
                .iter()
                .find(|n| n.id.to_lowercase().contains(&q) || n.name.to_lowercase().contains(&q))
        })
}

/// Upstream transitions (skills that feed into this one).
pub fn upstream(skill_id: &str) -> Vec<&'static GraphEdge> {
    let id = find_node(skill_id)
        .map(|n| n.id.as_str())
        .unwrap_or(skill_id);
    GRAPH.edges.iter().filter(|e| e.to == id).collect()
}

/// Downstream transitions (skills this one feeds into).
pub fn downstream(skill_id: &str) -> Vec<&'static GraphEdge> {
    let id = find_node(skill_id)
        .map(|n| n.id.as_str())
        .unwrap_or(skill_id);
    GRAPH.edges.iter().filter(|e| e.from == id).collect()
}

/// BFS shortest path between two skills, returning (node_id, data_types_in)
/// for each hop.
pub fn find_path(from: &str, to: &str) -> Option<Vec<(String, String, u64)>> {
    let from_node = find_node(from)?;
    let to_id = find_node(to)?.id.clone();

    // Build adjacency
    let mut adj: HashMap<&str, Vec<&GraphEdge>> = HashMap::new();
    for e in &GRAPH.edges {
        adj.entry(e.from.as_str()).or_default().push(e);
    }

    let mut queue = std::collections::VecDeque::new();
    let mut prev: HashMap<String, (String, String, u64)> = HashMap::new();
    let mut visited = std::collections::HashSet::new();
    queue.push_back(from_node.id.clone());
    visited.insert(from_node.id.clone());

    while let Some(cur) = queue.pop_front() {
        if cur == to_id {
            // Reconstruct path
            let mut path = vec![(to_id.clone(), String::new(), 0)];
            let mut c = &path[0].0;
            while let Some((p, dt, papers)) = prev.get(c) {
                path.push((p.clone(), dt.clone(), *papers));
                c = p;
            }
            path.reverse();
            // Fix data types: each hop's data type is the edge from i to i+1
            let mut fixed = Vec::new();
            for i in 0..path.len() {
                if i + 1 < path.len() {
                    let edge = GRAPH
                        .edges
                        .iter()
                        .find(|e| e.from == path[i].0 && e.to == path[i + 1].0);
                    if let Some(e) = edge {
                        fixed.push((path[i].0.clone(), e.data_types.clone(), e.papers));
                    } else {
                        fixed.push(path[i].clone());
                    }
                } else {
                    fixed.push(path[i].clone());
                }
            }
            return Some(fixed);
        }
        if let Some(neighbors) = adj.get(cur.as_str()) {
            for e in neighbors {
                if visited.insert(e.to.clone()) {
                    prev.insert(e.to.clone(), (cur.clone(), e.data_types.clone(), e.papers));
                    queue.push_back(e.to.clone());
                }
            }
        }
    }
    None
}

/// Format graph query results for AI prompts.
pub fn format_transitions(skill_id: &str, direction: &str) -> String {
    let node = match find_node(skill_id) {
        Some(n) => n,
        None => return format!("Skill '{}' not found in the pipeline graph.", skill_id),
    };
    let mut s = format!(
        "## {} (`{}`) — {} tools\n{}\n\n",
        node.name, node.id, node.tools, node.overview
    );
    let edges = match direction {
        "upstream" => upstream(&node.id),
        "downstream" => downstream(&node.id),
        _ => {
            let mut both = upstream(&node.id);
            both.extend(downstream(&node.id));
            both
        }
    };
    if edges.is_empty() {
        s.push_str("No transitions found.\n");
        return s;
    }
    let label = match direction {
        "upstream" => "Upstream (feeds in)",
        "downstream" => "Downstream (feeds out)",
        _ => "Transitions",
    };
    s.push_str(&format!("{}:\n", label));
    for e in edges {
        let name = find_node(&e.to)
            .map(|n| n.name.clone())
            .unwrap_or_else(|| e.to.clone());
        s.push_str(&format!(
            "- {} (`{}`) via [{}] — {} papers\n",
            name, e.to, e.data_types, e.papers
        ));
    }
    s
}

/// Format a shortest-path result.
pub fn format_path(from: &str, to: &str) -> String {
    match find_path(from, to) {
        None => format!("No pipeline path found from '{}' to '{}'.", from, to),
        Some(path) => {
            let mut s = format!("Pipeline path: {} → {} ({} steps)\n", from, to, path.len());
            for (node_id, data_types, papers) in &path {
                let name = find_node(node_id)
                    .map(|n| n.name.clone())
                    .unwrap_or_else(|| node_id.clone());
                if data_types.is_empty() {
                    s.push_str(&format!("  {name}\n"));
                } else {
                    s.push_str(&format!("  {name} —({data_types}, {papers} papers)→\n"));
                }
            }
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_is_embedded() {
        let (nodes, edges) = graph_stats();
        assert!(nodes >= 70, "expected 70+ nodes, got {nodes}");
        assert!(edges >= 400, "expected 400+ edges, got {edges}");
    }

    #[test]
    fn node_lookup() {
        assert!(find_node("variant-calling").is_some());
        assert!(find_node("Variant Calling").is_some());
    }

    #[test]
    fn transitions_exist() {
        let down = downstream("wgs-alignment");
        assert!(!down.is_empty());
        assert!(down.iter().any(|e| e.to == "variant-calling"));
    }

    #[test]
    fn path_wgs_to_variant() {
        let path = find_path("wgs-alignment", "variant-calling");
        assert!(path.is_some());
        let path = path.unwrap();
        assert_eq!(path.first().unwrap().0, "wgs-alignment");
        assert_eq!(path.last().unwrap().0, "variant-calling");
    }

    #[test]
    fn path_annotated_with_data_types() {
        let path = find_path("wgs-alignment", "variant-calling").unwrap();
        assert!(path[0].1.contains("BAM") || path[0].1.contains("CRAM"));
    }
}
