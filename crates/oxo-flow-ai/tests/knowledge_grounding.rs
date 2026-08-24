//! Knowledge grounding-accuracy guard (eval benchmark, tool layer).
//!
//! The deterministic half of the AI tool-layer evaluation: well-known
//! tools must resolve in the embedded knowledge base with plausible
//! versions, aliases and exact names must both hit, and fake tools must
//! resolve to nothing. This runs in CI without any AI provider — the
//! LLM-facing half lives in `eval/` (gold CSVs + capture/runner scripts).
//!
//! Version assertions are deliberately *shape checks*, not exact pins:
//! the monthly refresh pipeline legitimately bumps versions, so the guard
//! must stay green across refreshes while still catching data loss,
//! mangled entries, or an empty table.

use oxo_flow_ai::knowledge::bioconda::{get_tool, search_tools};
use oxo_flow_ai::knowledge::registry::{RegistryHit, get_registry_tool, search_registry};

/// A curated sample of widely used tools that must exist in the embedded
/// bioconda table.
const WELL_KNOWN_TOOLS: &[&str] = &[
    "fastp",
    "bwa-mem2",
    "star",
    "samtools",
    "gatk4",
    "ensembl-vep",
    "fastqc",
    "bedtools",
    "multiqc",
    "spades",
    "kraken2",
    "salmon",
    "subread",
    "picard",
    "bismark",
    // R/Bioconductor analysis packages (TOOL_ALLOWLIST in
    // scripts/refresh-knowledge/refresh_bioconda.py must keep these in
    // the table — they are the primary tools of their domains).
    "r-seurat",
    "bioconductor-dada2",
    "bioconductor-deseq2",
];

fn plausible_semver(version: &str) -> bool {
    let mut parts = version.split('.');
    let first = match parts.next() {
        Some(f) => f,
        None => return false,
    };
    first.parse::<u32>().is_ok()
        && parts.all(|p| {
            // Trailing segments may be build suffixes like "11b" (star)
            // — the leading numeric part must parse.
            let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
            !digits.is_empty()
        })
}

#[test]
fn well_known_tools_resolve_with_plausible_versions() {
    for name in WELL_KNOWN_TOOLS {
        let tool = get_tool(name)
            .unwrap_or_else(|| panic!("{name} must exist in the embedded bioconda table"));
        assert!(
            !tool.version.is_empty() && plausible_semver(&tool.version),
            "{name} has an implausible version {:?}",
            tool.version
        );
        assert!(
            !tool.summary.is_empty(),
            "{name} must carry a non-empty summary"
        );
    }
}

#[test]
fn exact_name_search_finds_the_tool() {
    // The same names must be discoverable through the search path an AI
    // agent's lookup_tool uses, not only through the exact get_tool path.
    for name in WELL_KNOWN_TOOLS {
        let hits = search_tools(name, 10);
        assert!(
            hits.iter().any(|t| t.name == *name),
            "search_tools({name:?}) must return {name} itself"
        );
    }
}

#[test]
fn fake_tools_resolve_to_nothing() {
    // Negative samples: an AI grounded in this table must not invent tools.
    for fake in ["bwa_mem4", "rnaseq_ultra_aligner", "fastq_super_cleaner"] {
        assert!(
            get_tool(fake).is_none(),
            "fake tool {fake} must not exist in the bioconda table"
        );
        assert!(
            get_registry_tool(fake).is_none(),
            "fake tool {fake} must not exist in the merged registry"
        );
    }
}

#[test]
fn merged_registry_resolves_registered_sources() {
    // The merged registry (lookup_tool's fallback) must resolve a
    // commercial tool and an nf-core module name.
    match get_registry_tool("cellranger").unwrap_or_else(|| {
        panic!("cellranger must resolve in the merged registry (commercial table)")
    }) {
        RegistryHit::Commercial(tool) => {
            assert!(!tool.name.is_empty());
        }
        _ => panic!("cellranger must resolve as a commercial tool"),
    }

    // nf-core module names use underscores (bwa_mem); the registry must
    // resolve them from the nf-core table.
    let found = get_registry_tool("bwa_mem")
        .unwrap_or_else(|| panic!("nf-core module bwa_mem must resolve in the merged registry"));
    assert!(
        matches!(found, RegistryHit::NfCore(_)),
        "bwa_mem must resolve as an nf-core module"
    );
}

#[test]
fn registry_search_finds_commercial_tools_by_purpose_word() {
    // Purpose-style queries (the AI-facing path) must surface the tool:
    // a search for "basecall" should find dorado in the merged registry.
    let hits = search_registry("basecall", 20);
    let names: Vec<&str> = hits
        .iter()
        .map(|h| match h {
            RegistryHit::NfCore(m) => m.name.as_str(),
            RegistryHit::Commercial(c) => c.name.as_str(),
            RegistryHit::BioTools(b) => b.name.as_str(),
        })
        .collect();
    assert!(
        names.iter().any(|n| n.contains("dorado")),
        "search_registry(\"basecall\") must surface dorado, got {names:?}"
    );
}
