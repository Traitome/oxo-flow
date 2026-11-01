//! Knowledge system — domain knowledge for AI agents.
//!
//! The knowledge system provides:
//! - Built-in bioinformatics tool reference tables
//! - Error pattern matching for diagnosis
//! - Best practice rules for workflow validation
//! - Context assembly for different agent scenarios

/// Shared ranked-name search used by every embedded knowledge table:
/// exact match → name prefix → name substring → summary keyword, capped at
/// `limit`. Single source so a ranking tweak lands once (the five
/// hand-copied implementations had already begun drifting in caps).
pub(crate) fn ranked_name_search<'a, T>(
    entries: impl IntoIterator<Item = &'a T>,
    query: &str,
    limit: usize,
    name_of: impl Fn(&T) -> &str,
    summary_of: impl Fn(&T) -> &str,
) -> Vec<&'a T> {
    let q = query.to_lowercase();
    let (mut exact, mut prefix, mut substring, mut summary_hits) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for entry in entries {
        let name = name_of(entry).to_lowercase();
        if name == q {
            exact.push(entry);
        } else if name.starts_with(&q) {
            prefix.push(entry);
        } else if name.contains(&q) {
            substring.push(entry);
        } else if summary_of(entry).to_lowercase().contains(&q) {
            summary_hits.push(entry);
        }
    }
    exact
        .into_iter()
        .chain(prefix)
        .chain(substring)
        .chain(summary_hits)
        .take(limit)
        .collect()
}

pub mod bioconda;
pub mod builtin;
pub mod meta;
pub mod pipeline_graph;
pub mod registry;
pub mod skillgraph_docs;
pub mod skills;
