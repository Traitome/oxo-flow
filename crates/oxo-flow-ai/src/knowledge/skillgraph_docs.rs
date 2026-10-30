//! Embedded full SKILL.md documentation for pipeline skills.
//!
//! One record per skill-graph node (`{"i": "<skill-id>", "doc": "<markdown>"}`)
//! produced by `scripts/refresh-knowledge/refresh_skillgraph.py` alongside
//! the compact pipeline graph. The docs carry the complete protocol —
//! Triggers, Related Skills, Required Tools, Workflow, caveats.
//!
//! Lookup is bridged through [`super::pipeline_graph::find_node`], so a
//! skill id, a display name, or a unique substring all resolve to the same
//! doc. Embedded at build time and queried on demand — never injected
//! wholesale into prompts.

use std::sync::LazyLock;

use serde::Deserialize;

use super::pipeline_graph;

/// Full-text JSONL: one {i, doc} record per skill.
const DOCS_RAW: &str = include_str!("skillgraph_docs.jsonl");

#[derive(Deserialize)]
struct RawDoc {
    i: String,
    doc: String,
}

/// One embedded skill document.
#[derive(Debug, Clone)]
pub struct SkillDoc {
    pub id: &'static str,
    pub doc: &'static str,
}

struct DocDb {
    docs: Vec<SkillDoc>,
}

impl DocDb {
    fn new() -> Self {
        let mut docs: Vec<SkillDoc> = Vec::new();
        // serde stream, not line splitting — doc bodies contain escaped
        // newlines and quotes, but records are one JSON value per line.
        let de = serde_json::Deserializer::from_str(DOCS_RAW);
        for rec in de.into_iter::<RawDoc>() {
            let Ok(rec) = rec else { continue };
            docs.push(SkillDoc {
                id: Box::leak(rec.i.into_boxed_str()),
                doc: Box::leak(rec.doc.into_boxed_str()),
            });
        }
        docs.sort_by(|a, b| a.id.cmp(b.id));
        Self { docs }
    }
}

static DOCS: LazyLock<DocDb> = LazyLock::new(DocDb::new);

/// Number of embedded skill documents.
pub fn doc_count() -> usize {
    DOCS.docs.len()
}

/// Total size of the embedded documentation in bytes.
pub fn doc_chars() -> usize {
    DOCS.docs.iter().map(|d| d.doc.len()).sum()
}

/// Resolve a query to a doc via the pipeline-graph node index (id, name,
/// or unique substring, case-insensitive). Returns `None` for unknown
/// skills AND for graph nodes without a doc (currently only `shared`).
pub fn get_doc(query: &str) -> Option<&'static SkillDoc> {
    let id = pipeline_graph::find_node(query)?.id.clone();
    DOCS.docs.iter().find(|d| d.id == id)
}

/// First markdown heading's text, used as the doc title in listings.
fn doc_title(doc: &str) -> &str {
    doc.lines()
        .find(|l| l.starts_with("# "))
        .map(|l| l[2..].trim())
        .unwrap_or("")
}

/// Format one doc for an AI prompt: the header echoes the RESOLVED id, so
/// a substring-fallback match is visible and the model can catch a
/// wrong-skill hit. The size is reported in bytes (same measure as the
/// freshness record counts) rather than an inflated char count.
pub fn format_doc(doc: &SkillDoc) -> String {
    format!(
        "Skill doc for `{}` ({} bytes)\n\n{}",
        doc.id,
        doc.doc.len(),
        doc.doc
    )
}

/// Compact index of all embedded docs (id — title, one line each).
pub fn list_docs() -> String {
    let mut s = format!(
        "{} embedded skill docs ({} KB). Get one with lookup_doc {{\"skill\":\"<id>\"}}:\n",
        DOCS.docs.len(),
        doc_chars() / 1024
    );
    for d in &DOCS.docs {
        s.push_str(&format!("- {} — {}\n", d.id, doc_title(d.doc)));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docs_are_embedded() {
        let n = doc_count();
        assert!(n >= 70, "expected 70+ embedded skill docs, got {n}");
    }

    #[test]
    fn every_doc_id_is_a_graph_node() {
        for d in &DOCS.docs {
            assert!(
                pipeline_graph::find_node(d.id).is_some(),
                "doc id '{}' is not a pipeline-graph node",
                d.id
            );
        }
    }

    #[test]
    fn shared_node_has_no_doc() {
        // Every graph node except `shared` has a doc; `shared` must resolve
        // as a node but yield a friendly no-doc result, never a panic.
        assert!(pipeline_graph::find_node("shared").is_some());
        assert!(get_doc("shared").is_none());
    }

    #[test]
    fn get_doc_is_case_insensitive() {
        let lower = get_doc("variant-calling").expect("variant-calling has a doc");
        let upper = get_doc("VARIANT-CALLING").expect("lookup is case-insensitive");
        assert_eq!(lower.id, upper.id);
    }

    #[test]
    fn get_doc_resolves_display_name() {
        let by_name = get_doc("Variant Calling").expect("display name resolves");
        let by_id = get_doc("variant-calling").expect("id resolves");
        assert_eq!(by_id.id, by_name.id);
    }

    #[test]
    fn get_doc_unknown_is_none() {
        assert!(get_doc("not-a-real-skill").is_none());
    }

    #[test]
    fn format_doc_echoes_resolved_id_and_body() {
        let doc = get_doc("variant-calling").expect("variant-calling has a doc");
        let text = format_doc(doc);
        assert!(text.starts_with(&format!("Skill doc for `{}`", doc.id)));
        assert!(
            text.contains(doc.doc),
            "formatted output must carry the full body"
        );
    }

    #[test]
    fn largest_doc_is_bounded() {
        // Token-safety contract: get_doc returns one full doc. If a future
        // refresh inflates a doc past this bound, fail loudly and revisit.
        let max = DOCS.docs.iter().map(|d| d.doc.len()).max().unwrap_or(0);
        assert!(max < 20_000, "largest embedded doc is {max} chars");
    }

    #[test]
    fn list_docs_covers_all_docs() {
        let text = list_docs();
        assert!(
            text.contains("variant-calling"),
            "index should list known ids"
        );
        assert!(
            text.contains("Get one with lookup_doc"),
            "index should teach usage"
        );
        let lines = text.lines().count();
        assert!(
            lines > doc_count(),
            "one line per doc plus header, got {lines}"
        );
    }
}
