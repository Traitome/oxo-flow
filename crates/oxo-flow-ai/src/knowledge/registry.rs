//! Extended tool registry — knowledge tables beyond the Bioconda CLI DB.
//!
//! Loaded lazily (at first use) from JSONL files embedded at build time:
//!
//!   - nfcore_modules.jsonl   — nf-core module table: pinned Bioconda
//!     versions, licenses, bio.tools identifiers, reference DOIs.
//!   - commercial_tools.jsonl — commercial / closed-source CLIs (10x,
//!     Illumina, NVIDIA, ONT, ...) with version provenance.
//!   - biotools_overlay.jsonl — bio.tools descriptions, EDAM topics and
//!     operations, licenses and DOIs for the tools the other tables cover.
//!   - edam_terms.jsonl       — EDAM ontology terms (topic_* / operation_*)
//!     referenced by the biotools overlay.
//!
//! Each table exposes count / exact lookup / fuzzy search / formatting in
//! the same shape as [`super::bioconda`]; the merged
//! [`format_registry_results`] feeds the `lookup_tool` fallback so agents
//! get nf-core and commercial information even when Bioconda has no match.

use std::sync::LazyLock;

const NFCORE_RAW: &str = include_str!("nfcore_modules.jsonl");
const COMMERCIAL_RAW: &str = include_str!("commercial_tools.jsonl");
const BIOTOOLS_RAW: &str = include_str!("biotools_overlay.jsonl");
const EDAM_RAW: &str = include_str!("edam_terms.jsonl");

/// One nf-core module record.
#[derive(Debug, Clone)]
pub struct NfCoreModule {
    pub name: String,
    /// Bioconda pins from the module's environment.yml ("fastp=1.3.6").
    pub versions: Vec<String>,
    pub summary: String,
    pub license: String,
    /// bio.tools identifier, e.g. "biotools:fastp" ("" when absent).
    pub biotools_id: String,
    pub doi: String,
}

/// One commercial tool record.
#[derive(Debug, Clone)]
pub struct CommercialTool {
    pub name: String,
    pub version: String,
    pub summary: String,
    /// github-releases | scrape | pin | manual.
    pub source: String,
    /// UTC date of the last automatic check.
    pub checked_at: String,
    pub auto: bool,
    pub url: String,
    pub note: String,
}

/// One bio.tools overlay record (description / topic / operation metadata).
#[derive(Debug, Clone)]
pub struct BioToolsOverlay {
    pub name: String,
    pub description: String,
    pub license: String,
    pub homepage: String,
    /// EDAM topic ids, e.g. ["topic_0091"].
    pub topics: Vec<String>,
    /// EDAM operation ids, e.g. ["operation_2403"].
    pub operations: Vec<String>,
    pub dois: Vec<String>,
}

/// One EDAM term (topic_* or operation_* class).
#[derive(Debug, Clone)]
pub struct EdamTerm {
    pub uri: String,
    pub label: String,
    pub definition: String,
}

static NFCORE_DB: LazyLock<Vec<NfCoreModule>> = LazyLock::new(|| {
    NFCORE_RAW
        .lines()
        .filter_map(|line| {
            let rec: serde_json::Value = serde_json::from_str(line).ok()?;
            Some(NfCoreModule {
                name: rec.get("n")?.as_str()?.to_string(),
                versions: rec
                    .get("v")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                summary: rec
                    .get("t")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
                license: rec
                    .get("license")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
                biotools_id: rec
                    .get("biotools_id")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
                doi: rec
                    .get("doi")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect()
});

static COMMERCIAL_DB: LazyLock<Vec<CommercialTool>> = LazyLock::new(|| {
    COMMERCIAL_RAW
        .lines()
        .filter_map(|line| {
            let rec: serde_json::Value = serde_json::from_str(line).ok()?;
            Some(CommercialTool {
                name: rec.get("n")?.as_str()?.to_string(),
                version: rec
                    .get("v")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                summary: rec
                    .get("t")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
                source: rec
                    .get("source")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
                checked_at: rec
                    .get("checked_at")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
                auto: rec.get("auto").and_then(|a| a.as_bool()).unwrap_or(false),
                url: rec
                    .get("url")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
                note: rec
                    .get("note")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect()
});

static BIOTOOLS_DB: LazyLock<Vec<BioToolsOverlay>> = LazyLock::new(|| {
    BIOTOOLS_RAW
        .lines()
        .filter_map(|line| {
            let rec: serde_json::Value = serde_json::from_str(line).ok()?;
            Some(BioToolsOverlay {
                name: rec.get("n")?.as_str()?.to_string(),
                description: rec
                    .get("description")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
                license: rec
                    .get("license")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
                homepage: rec
                    .get("homepage")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
                topics: string_list(rec.get("topic")),
                operations: string_list(rec.get("operation")),
                dois: string_list(rec.get("doi")),
            })
        })
        .collect()
});

static EDAM_DB: LazyLock<Vec<EdamTerm>> = LazyLock::new(|| {
    EDAM_RAW
        .lines()
        // '#'-prefixed attribution header lines are dropped by the JSON parse.
        .filter_map(|line| {
            let rec: serde_json::Value = serde_json::from_str(line).ok()?;
            Some(EdamTerm {
                uri: rec.get("uri")?.as_str()?.to_string(),
                label: rec.get("label")?.as_str()?.to_string(),
                definition: rec
                    .get("definition")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect()
});

fn string_list(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// ── nf-core modules ────────────────────────────────────────────────────────

/// Total number of nf-core modules in the embedded table.
pub fn nfcore_count() -> usize {
    NFCORE_DB.len()
}

/// Exact-name module lookup (case-insensitive).
pub fn get_nfcore_module(name: &str) -> Option<&'static NfCoreModule> {
    let needle = name.to_lowercase();
    NFCORE_DB.iter().find(|m| m.name.to_lowercase() == needle)
}

/// Fuzzy search over module names and summaries, ranked by name match.
pub fn search_nfcore(query: &str, limit: usize) -> Vec<&'static NfCoreModule> {
    let q = query.to_lowercase();
    let mut exact: Vec<&NfCoreModule> = Vec::new();
    let mut prefix: Vec<&NfCoreModule> = Vec::new();
    let mut substring: Vec<&NfCoreModule> = Vec::new();
    let mut summary_hits: Vec<&NfCoreModule> = Vec::new();
    for module in NFCORE_DB.iter() {
        let name = module.name.to_lowercase();
        if name == q {
            exact.push(module);
        } else if name.starts_with(&q) {
            prefix.push(module);
        } else if name.contains(&q) {
            substring.push(module);
        } else if module.summary.to_lowercase().contains(&q) {
            summary_hits.push(module);
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

// ── commercial tools ───────────────────────────────────────────────────────

/// Total number of commercial tools in the embedded table.
pub fn commercial_count() -> usize {
    COMMERCIAL_DB.len()
}

/// Exact-name commercial lookup (case-insensitive).
pub fn get_commercial_tool(name: &str) -> Option<&'static CommercialTool> {
    let needle = name.to_lowercase();
    COMMERCIAL_DB
        .iter()
        .find(|t| t.name.to_lowercase() == needle)
}

/// Fuzzy search over commercial tool names and summaries.
pub fn search_commercial(query: &str, limit: usize) -> Vec<&'static CommercialTool> {
    let q = query.to_lowercase();
    let mut exact: Vec<&CommercialTool> = Vec::new();
    let mut prefix: Vec<&CommercialTool> = Vec::new();
    let mut substring: Vec<&CommercialTool> = Vec::new();
    let mut summary_hits: Vec<&CommercialTool> = Vec::new();
    for tool in COMMERCIAL_DB.iter() {
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
    exact
        .into_iter()
        .chain(prefix)
        .chain(substring)
        .chain(summary_hits)
        .take(limit)
        .collect()
}

// ── bio.tools overlay ──────────────────────────────────────────────────────

/// Total number of bio.tools overlay records.
pub fn biotools_count() -> usize {
    BIOTOOLS_DB.len()
}

/// Exact-name overlay lookup (case-insensitive).
pub fn get_biotools_overlay(name: &str) -> Option<&'static BioToolsOverlay> {
    let needle = name.to_lowercase();
    BIOTOOLS_DB.iter().find(|t| t.name.to_lowercase() == needle)
}

/// Fuzzy search over overlay names and descriptions.
pub fn search_biotools(query: &str, limit: usize) -> Vec<&'static BioToolsOverlay> {
    let q = query.to_lowercase();
    let mut exact: Vec<&BioToolsOverlay> = Vec::new();
    let mut prefix: Vec<&BioToolsOverlay> = Vec::new();
    let mut substring: Vec<&BioToolsOverlay> = Vec::new();
    let mut text_hits: Vec<&BioToolsOverlay> = Vec::new();
    for tool in BIOTOOLS_DB.iter() {
        let name = tool.name.to_lowercase();
        if name == q {
            exact.push(tool);
        } else if name.starts_with(&q) {
            prefix.push(tool);
        } else if name.contains(&q) {
            substring.push(tool);
        } else if tool.description.to_lowercase().contains(&q) {
            text_hits.push(tool);
        }
    }
    exact
        .into_iter()
        .chain(prefix)
        .chain(substring)
        .chain(text_hits)
        .take(limit)
        .collect()
}

// ── EDAM terms ─────────────────────────────────────────────────────────────

/// Total number of EDAM terms.
pub fn edam_count() -> usize {
    EDAM_DB.len()
}

/// Exact EDAM term lookup by URI ("topic_XXXX").
pub fn get_edam_term(uri: &str) -> Option<&'static EdamTerm> {
    EDAM_DB.iter().find(|t| t.uri == uri)
}

/// Fuzzy search over EDAM labels and definitions.
pub fn search_edam(query: &str, limit: usize) -> Vec<&'static EdamTerm> {
    let q = query.to_lowercase();
    let mut exact: Vec<&EdamTerm> = Vec::new();
    let mut prefix: Vec<&EdamTerm> = Vec::new();
    let mut substring: Vec<&EdamTerm> = Vec::new();
    let mut text_hits: Vec<&EdamTerm> = Vec::new();
    for term in EDAM_DB.iter() {
        let label = term.label.to_lowercase();
        if label == q {
            exact.push(term);
        } else if label.starts_with(&q) {
            prefix.push(term);
        } else if label.contains(&q) {
            substring.push(term);
        } else if term.definition.to_lowercase().contains(&q) {
            text_hits.push(term);
        }
    }
    exact
        .into_iter()
        .chain(prefix)
        .chain(substring)
        .chain(text_hits)
        .take(limit)
        .collect()
}

// ── Merged registry lookup (lookup_tool fallback) ──────────────────────────

/// One merged registry hit: nf-core module, commercial tool, or bio.tools
/// overlay record.
#[derive(Debug, Clone)]
pub enum RegistryHit {
    NfCore(&'static NfCoreModule),
    Commercial(&'static CommercialTool),
    BioTools(&'static BioToolsOverlay),
}

/// Total registry size across all tables.
pub fn registry_count() -> usize {
    nfcore_count() + commercial_count() + biotools_count()
}

/// Exact merged lookup (case-insensitive): commercial first, then nf-core,
/// then the bio.tools overlay.
pub fn get_registry_tool(name: &str) -> Option<RegistryHit> {
    if let Some(tool) = get_commercial_tool(name) {
        return Some(RegistryHit::Commercial(tool));
    }
    if let Some(module) = get_nfcore_module(name) {
        return Some(RegistryHit::NfCore(module));
    }
    get_biotools_overlay(name).map(RegistryHit::BioTools)
}

/// Merged fuzzy search across the three name-bearing tables.
pub fn search_registry(query: &str, limit: usize) -> Vec<RegistryHit> {
    let mut hits: Vec<RegistryHit> = Vec::new();
    for tool in search_commercial(query, limit) {
        hits.push(RegistryHit::Commercial(tool));
    }
    for module in search_nfcore(query, limit) {
        hits.push(RegistryHit::NfCore(module));
    }
    for tool in search_biotools(query, limit) {
        hits.push(RegistryHit::BioTools(tool));
    }
    hits.truncate(limit);
    hits
}

/// Render one registry hit as a bullet line for AI prompts.
pub fn format_registry_hit(hit: &RegistryHit) -> String {
    match hit {
        RegistryHit::NfCore(m) => {
            let versions = if m.versions.is_empty() {
                "(no pinned versions)".to_string()
            } else {
                m.versions.join(", ")
            };
            format!(
                "- [nf-core] {} — {} [pinned: {}; license: {}]",
                m.name,
                if m.summary.is_empty() {
                    "(no description)"
                } else {
                    &m.summary
                },
                versions,
                if m.license.is_empty() {
                    "unknown"
                } else {
                    &m.license
                },
            )
        }
        RegistryHit::Commercial(t) => {
            let version = if t.version.is_empty() {
                "(version unknown)".to_string()
            } else {
                t.version.clone()
            };
            format!(
                "- [commercial] {} {} — {} [source: {}; {}]",
                t.name,
                version,
                if t.summary.is_empty() {
                    "(no description)"
                } else {
                    &t.summary
                },
                t.source,
                if t.note.is_empty() {
                    "no note"
                } else {
                    &t.note
                },
            )
        }
        RegistryHit::BioTools(b) => format!(
            "- [bio.tools] {} — {} [license: {}]",
            b.name,
            if b.description.is_empty() {
                "(no description)"
            } else {
                &b.description
            },
            if b.license.is_empty() {
                "unknown"
            } else {
                &b.license
            },
        ),
    }
}

/// Format merged registry search results for AI prompts.
///
/// Returns an empty string when nothing matched, so callers can chain this
/// after the Bioconda fallback without producing noise.
pub fn format_registry_results(query: &str, limit: usize) -> String {
    let hits = search_registry(query, limit);
    if hits.is_empty() {
        return String::new();
    }
    let mut s = format!(
        "Also found {} of {} registry records (nf-core + commercial + bio.tools) matching '{}':\n",
        hits.len(),
        registry_count(),
        query
    );
    for hit in &hits {
        s.push_str(&format_registry_hit(hit));
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfcore_database_is_embedded() {
        assert!(
            nfcore_count() >= 900,
            "expected 900+ modules, got {}",
            nfcore_count()
        );
    }

    #[test]
    fn nfcore_exact_lookup_works() {
        // fastp is the reference module in the nf-core test suite.
        let fastp = get_nfcore_module("fastp").expect("fastp should be in the table");
        assert!(!fastp.summary.is_empty());
    }

    #[test]
    fn nfcore_search_by_name_fragment() {
        let results = search_nfcore("flagstat", 5);
        assert!(!results.is_empty());
    }

    #[test]
    fn commercial_lookup_has_versions() {
        let cellranger = get_commercial_tool("cellranger").expect("cellranger should be tracked");
        assert!(
            !cellranger.version.is_empty(),
            "cellranger version should be set"
        );
    }

    #[test]
    fn commercial_unknown_query_returns_empty() {
        assert!(search_commercial("zzzznonexistent", 5).is_empty());
    }

    #[test]
    fn biotools_overlay_is_embedded() {
        assert!(
            biotools_count() >= 1000,
            "expected 1000+ overlay records, got {}",
            biotools_count()
        );
    }

    #[test]
    fn edam_terms_are_embedded() {
        assert!(
            edam_count() >= 700,
            "expected 700+ EDAM terms, got {}",
            edam_count()
        );
        // operation_2403 is "Sequence analysis" in current EDAM.
        let seq_analysis = get_edam_term("operation_2403").expect("operation_2403 should exist");
        assert_eq!(seq_analysis.label, "Sequence analysis");
    }

    #[test]
    fn edam_header_lines_are_dropped() {
        // '#'-prefixed CC BY-SA attribution lines must not produce terms.
        assert!(EDAM_DB.iter().all(|t| !t.uri.is_empty()));
    }

    #[test]
    fn merged_lookup_finds_commercial_first() {
        let hit = get_registry_tool("dorado").expect("dorado should resolve");
        assert!(matches!(hit, RegistryHit::Commercial(_)));
    }

    #[test]
    fn merged_lookup_finds_nfcore() {
        let hit = get_registry_tool("fastp").expect("fastp should resolve");
        assert!(matches!(hit, RegistryHit::NfCore(_)));
    }

    #[test]
    fn merged_format_returns_empty_on_no_match() {
        assert!(format_registry_results("zzzznonexistent", 5).is_empty());
    }

    #[test]
    fn merged_format_is_readable() {
        let out = format_registry_results("fastp", 3);
        assert!(out.contains("fastp"), "{out}");
        assert!(out.contains("[nf-core]"), "{out}");
    }
}
