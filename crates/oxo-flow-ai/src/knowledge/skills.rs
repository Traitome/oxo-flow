//! Embedded bioinformatics skills library.
//!
//! 562 curated Agent Skills (SKILL.md standard) from the GPTomics/bioSkills
//! project, embedded at build time. Each skill is a domain-specific
//! procedure: tool commands, parameters, caveats, and AI-agent guidance
//! for one bioinformatics task.
//!
//! The library is queried by domain (e.g. "rna-seq", "variant-calling")
//! to inject relevant expertise into workflow-generation prompts, and
//! exposed to agents via the `lookup_skill` tool.

use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// Compact JSONL: {name, description, domain, tool_type, primary_tool, preview}.
const SKILLS_RAW: &str = include_str!("skills_index.jsonl");

/// One embedded skill record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRecord {
    pub name: String,
    pub description: String,
    pub domain: String,
    #[serde(default)]
    pub tool_type: String,
    #[serde(default)]
    pub primary_tool: String,
    #[serde(default)]
    pub preview: String,
}

/// In-memory skill database parsed once at first use.
static SKILL_DB: LazyLock<Vec<SkillRecord>> = LazyLock::new(|| {
    SKILLS_RAW
        .lines()
        .filter_map(|line| serde_json::from_str::<SkillRecord>(line).ok())
        .collect()
});

/// Total number of embedded skills.
pub fn skill_count() -> usize {
    SKILL_DB.len()
}

/// List all skill domains with their skill counts.
pub fn list_domains() -> Vec<(String, usize)> {
    let mut domains: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for s in SKILL_DB.iter() {
        *domains.entry(s.domain.clone()).or_default() += 1;
    }
    domains.into_iter().collect()
}

/// Search skills by domain, name, description, or primary tool keyword.
/// Returns skills ranked by: exact name > domain match > description hit.
pub fn search_skills(query: &str, limit: usize) -> Vec<&'static SkillRecord> {
    let q = query.to_lowercase();
    let mut domain_hits: Vec<&SkillRecord> = Vec::new();
    let mut name_hits: Vec<&SkillRecord> = Vec::new();
    let mut text_hits: Vec<&SkillRecord> = Vec::new();

    for skill in SKILL_DB.iter() {
        let domain = skill.domain.to_lowercase();
        let name = skill.name.to_lowercase();
        let desc = skill.description.to_lowercase();
        let tool = skill.primary_tool.to_lowercase();

        if name.contains(&q) || tool.contains(&q) {
            name_hits.push(skill);
        } else if domain.contains(&q) {
            domain_hits.push(skill);
        } else if desc.contains(&q) {
            text_hits.push(skill);
        }
    }

    name_hits
        .into_iter()
        .chain(domain_hits)
        .chain(text_hits)
        .take(limit)
        .collect()
}

/// Get all skills in a domain (e.g. "variant-calling").
pub fn skills_in_domain(domain: &str) -> Vec<&'static SkillRecord> {
    SKILL_DB
        .iter()
        .filter(|s| s.domain.eq_ignore_ascii_case(domain))
        .collect()
}

/// Format skills for injection into an AI prompt.
pub fn format_skills(query: &str, limit: usize) -> String {
    let results = search_skills(query, limit);
    if results.is_empty() {
        return format!("No embedded skills matched '{}'.", query);
    }
    let mut s = format!(
        "Relevant bioinformatics skills for '{}' (of {} embedded):\n",
        query,
        skill_count()
    );
    for skill in results {
        let tool = if skill.primary_tool.is_empty() {
            "—".to_string()
        } else {
            skill.primary_tool.clone()
        };
        s.push_str(&format!(
            "- [{}] {} — {}\n  primary_tool: {}\n  {}\n",
            skill.domain, skill.name, skill.description, tool, skill.preview
        ));
    }
    s
}

/// Map a free-text assay/domain description to matching skill domains.
/// Used by workflow generation to select relevant expertise.
pub fn domains_for_intent(intent: &str) -> Vec<String> {
    let lower = intent.to_lowercase();
    let mut matched = Vec::new();

    // Domain keyword → bioSkills domain mapping (uses the library's actual
    // domain names). A keyword may map to multiple domains.
    let keyword_map: &[(&str, &[&str])] = &[
        (
            "rna-quantification",
            &[
                "rna-seq",
                "rnaseq",
                "transcriptom",
                "quantif",
                "featurecounts",
                "salmon",
                "kallisto",
            ],
        ),
        (
            "read-alignment",
            &[
                "align", "bwa", "star", "hisat", "bowtie", "wgs", "wes", "bam", "fastq", "mapping",
            ],
        ),
        (
            "read-qc",
            &[
                "qc",
                "quality control",
                "fastqc",
                "multiqc",
                "fastp",
                "trim",
            ],
        ),
        (
            "differential-expression",
            &["deseq", "differential expression", "deg"],
        ),
        (
            "variant-calling",
            &["variant", "gatk", "mutect", "snp", "freebayes"],
        ),
        ("chip-seq", &["chip-seq", "chip seq", "peak"]),
        ("atac-seq", &["atac"]),
        ("genome-assembly", &["assembl", "spades", "canu", "megahit"]),
        ("metagenomics", &["metagenom", "16s", "kraken"]),
        (
            "single-cell",
            &["single-cell", "single cell", "scrna", "scanpy", "seurat"],
        ),
        ("spatial-transcriptomics", &["spatial", "visium"]),
        ("copy-number", &["copy number", "cnv"]),
        ("structural-biology", &["structural", "protein structure"]),
        ("genome-annotation", &["annotat", "vep", "snpeff"]),
        ("methylation-analysis", &["methyl", "bisulfite", "bs-seq"]),
        ("long-read-sequencing", &["nanopore", "pacbio", "long-read"]),
        (
            "pathway-analysis",
            &["pathway", "go term", "enrichment", "gsea"],
        ),
        ("small-rna-seq", &["mirna", "small rna"]),
        ("proteomics", &["proteom", "mass spec"]),
        ("phylogenetics", &["phylogen", "evolutionary tree"]),
    ];

    for (domain, keywords) in keyword_map {
        if keywords.iter().any(|k| lower.contains(k)) {
            // Only include if the domain actually exists in the library
            if SKILL_DB.iter().any(|s| s.domain == *domain) {
                matched.push((*domain).to_string());
            }
        }
    }
    matched
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_are_embedded() {
        assert!(
            skill_count() >= 500,
            "expected 500+ skills, got {}",
            skill_count()
        );
    }

    #[test]
    fn domains_are_listed() {
        let domains = list_domains();
        assert!(domains.iter().any(|(d, _)| d == "variant-calling"));
        assert!(domains.iter().any(|(d, _)| d == "rna-quantification"));
    }

    #[test]
    fn search_by_domain() {
        let results = search_skills("variant", 5);
        assert!(!results.is_empty());
    }

    #[test]
    fn search_by_tool() {
        let results = search_skills("samtools", 5);
        assert!(!results.is_empty());
    }

    #[test]
    fn intent_to_domains() {
        let domains = domains_for_intent("RNA-seq analysis with STAR and DESeq2");
        assert!(domains.iter().any(|d| d == "rna-quantification"));
        assert!(domains.iter().any(|d| d == "differential-expression"));

        let domains = domains_for_intent("somatic variant calling with Mutect2");
        assert!(domains.iter().any(|d| d == "variant-calling"));

        let domains = domains_for_intent("ChIP-seq peak calling with MACS2");
        assert!(domains.iter().any(|d| d == "chip-seq"));
    }

    #[test]
    fn unknown_query_empty() {
        assert!(search_skills("zzzznonexistentdomain", 5).is_empty());
    }

    #[test]
    fn intent_matching_broad_coverage() {
        let cases = [
            (
                "WGS germline variant calling with GATK HaplotypeCaller",
                vec!["variant-calling", "read-alignment"],
            ),
            (
                "RNA-seq quantification with salmon and DESeq2 differential expression",
                vec!["rna-quantification", "differential-expression"],
            ),
            (
                "16S metagenomics taxonomic classification with Kraken2",
                vec!["metagenomics"],
            ),
            (
                "scRNA-seq clustering with Scanpy and Seurat",
                vec!["single-cell"],
            ),
            (
                "Nanopore long-read genome assembly",
                vec!["long-read-sequencing", "genome-assembly"],
            ),
            (
                "DNA methylation analysis with Bismark",
                vec!["methylation-analysis"],
            ),
            (
                "Pathway enrichment analysis with GSEA",
                vec!["pathway-analysis"],
            ),
            ("ChIP-seq peak calling with MACS2", vec!["chip-seq"]),
        ];
        for (intent, expected) in cases {
            let got = domains_for_intent(intent);
            for exp in expected {
                assert!(
                    got.iter().any(|d| d == exp),
                    "intent '{intent}' should match domain '{exp}', got {:?}",
                    got
                );
            }
        }
    }

    #[test]
    fn intent_matching_no_false_positive() {
        // Unrelated text should match nothing
        let domains = domains_for_intent("hello world");
        assert!(domains.is_empty());
    }
}
