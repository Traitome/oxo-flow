//! Embedded Bioconda CLI tool database.
//!
//! The full Bioconda channel metadata (6103 CLI tools, curated by the
//! oxo-call-extends project) is embedded into the binary at build time
//! via `include_str!` and exposed as a searchable database. AI agents
//! query it through the `lookup_tool` to find real tools, their purposes,
//! and their current Bioconda versions — instead of relying on the
//! model's (potentially outdated) training data alone.

use std::sync::LazyLock;

/// Compact JSONL: {name, version, text, subdirs} per tool.
const BIOCONDA_TOOLS_RAW: &str = include_str!("bioconda_tools.jsonl");

/// One Bioconda CLI tool record.
#[derive(Debug, Clone)]
pub struct BiocondaTool {
    pub name: String,
    pub version: String,
    pub summary: String,
    pub platforms: Vec<String>,
}

/// In-memory tool database parsed once at first use.
static BIOCONDA_DB: LazyLock<Vec<BiocondaTool>> = LazyLock::new(|| {
    BIOCONDA_TOOLS_RAW
        .lines()
        .filter_map(|line| {
            let rec: serde_json::Value = serde_json::from_str(line).ok()?;
            Some(BiocondaTool {
                name: rec.get("name")?.as_str()?.to_string(),
                version: rec
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                summary: rec
                    .get("text")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
                platforms: rec
                    .get("subdirs")
                    .and_then(|s| s.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect()
});

/// Total number of tools in the embedded database.
pub fn tool_count() -> usize {
    BIOCONDA_DB.len()
}

/// Exact-name lookup (case-insensitive).
pub fn get_tool(name: &str) -> Option<&'static BiocondaTool> {
    let needle = name.to_lowercase();
    BIOCONDA_DB.iter().find(|t| t.name.to_lowercase() == needle)
}

/// Fuzzy search: name substring OR summary keyword match.
/// Returns up to `limit` tools ranked by name-prefix match first.
pub fn search_tools(query: &str, limit: usize) -> Vec<&'static BiocondaTool> {
    let q = query.to_lowercase();
    let mut exact: Vec<&BiocondaTool> = Vec::new();
    let mut prefix: Vec<&BiocondaTool> = Vec::new();
    let mut substring: Vec<&BiocondaTool> = Vec::new();
    let mut summary_hits: Vec<&BiocondaTool> = Vec::new();

    for tool in BIOCONDA_DB.iter() {
        let name = tool.name.to_lowercase();
        if name == q {
            exact.push(tool);
        } else if name.starts_with(&q) {
            prefix.push(tool);
        } else if name.contains(&q) {
            substring.push(tool);
        } else if tool.summary.to_lowercase().contains(&q) {
            summary_hits.push(tool);
        }
    }

    let mut results = Vec::with_capacity(limit);
    for tool in exact
        .into_iter()
        .chain(prefix)
        .chain(substring)
        .chain(summary_hits)
    {
        if results.len() >= limit {
            break;
        }
        results.push(tool);
    }
    results
}

/// Format search results as a compact text block for AI prompts.
pub fn format_search_results(query: &str, limit: usize) -> String {
    let results = search_tools(query, limit);
    if results.is_empty() {
        return format!("No Bioconda CLI tools matched '{}'.", query);
    }
    let mut s = format!(
        "Found {} Bioconda CLI tool(s) matching '{}' (of {} total):\n",
        results.len(),
        query,
        tool_count()
    );
    for tool in results {
        let platform = if tool.platforms.is_empty() {
            "unknown".to_string()
        } else {
            tool.platforms.join(", ")
        };
        s.push_str(&format!(
            "- {} {} — {} [platforms: {}]\n",
            tool.name,
            tool.version,
            if tool.summary.is_empty() {
                "(no description)"
            } else {
                &tool.summary
            },
            platform
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_is_embedded() {
        assert!(
            tool_count() >= 6000,
            "expected 6000+ tools, got {}",
            tool_count()
        );
    }

    #[test]
    fn exact_lookup_works() {
        let star = get_tool("star").expect("STAR should be in the database");
        assert_eq!(star.name, "star");
        assert!(!star.version.is_empty());
    }

    #[test]
    fn case_insensitive_lookup() {
        assert!(get_tool("SAMTOOLS").is_some());
    }

    #[test]
    fn search_by_name_substring() {
        let results = search_tools("align", 5);
        assert!(!results.is_empty());
    }

    #[test]
    fn search_by_summary_keyword() {
        let results = search_tools("variant calling", 10);
        assert!(!results.is_empty());
    }

    #[test]
    fn unknown_query_returns_empty() {
        assert!(search_tools("zzzznonexistent", 5).is_empty());
    }

    #[test]
    fn format_results_is_readable() {
        let out = format_search_results("star", 3);
        assert!(out.contains("star"));
        assert!(out.contains("total)"));
    }
}
