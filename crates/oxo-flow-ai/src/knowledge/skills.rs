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
///
/// The query is tokenized and matched against the searchable fields
/// (name, domain, description, tool_type, primary_tool) with prefix
/// awareness. Ranking: rare terms first (IDF² sum), then the strongest
/// field hit (name/primary_tool > domain > description), then name.
/// Skills matching all searchable terms rank first; partial matches
/// backfill the remaining slots, so multi-term queries never die because
/// one term is missing everywhere. Queries are capped at
/// [`MAX_QUERY_TERMS`] tokens — the lexicographically first survive, since
/// term order carries no relevance signal.
pub fn search_skills(query: &str, limit: usize) -> Vec<&'static SkillRecord> {
    let mut terms = tokenize(query);
    // Pathological queries (model loops have emitted hundreds of repeated
    // lookup terms) cap the O(terms × corpus) work; terms are sorted, so
    // the lexicographically first survive — term order carries no
    // relevance signal.
    terms.truncate(MAX_QUERY_TERMS);
    if terms.is_empty() {
        return Vec::new();
    }

    // Document frequency per term over the whole corpus (prefix-aware
    // matching); terms that appear nowhere are agent hallucinations and are
    // dropped rather than killing the query (strict AND would return empty).
    // Each skill's haystack is needed twice — the document-frequency pass
    // and the scoring loop below; [`HAYSTACKS`] builds them once per
    // process.
    let dfs: Vec<usize> = terms
        .iter()
        .map(|t| {
            SKILL_DB
                .iter()
                .zip(HAYSTACKS.iter())
                .filter(|(_, h)| term_matches(t, h))
                .count()
        })
        .collect();
    let present: Vec<(&str, usize)> = terms
        .iter()
        .zip(&dfs)
        .filter(|(_, df)| **df > 0)
        .map(|(t, &df)| (t.as_str(), df))
        .collect();
    if present.is_empty() {
        return Vec::new();
    }

    // Primary score: Σ (ln(N/df))² over matched terms — rare terms dominate
    // generic ones ('bqsr' outweighs 'bam'). Secondary: strongest field hit
    // (name/primary_tool > domain > description).
    let n = SKILL_DB.len() as f64;
    let mut full: Vec<Candidate> = Vec::new();
    let mut partial: Vec<Candidate> = Vec::new();
    for (skill, hay) in SKILL_DB.iter().zip(HAYSTACKS.iter()) {
        let matched: Vec<(&str, usize)> = present
            .iter()
            .copied()
            .filter(|(t, _)| term_matches(t, hay))
            .collect();
        if matched.is_empty() {
            continue;
        }
        let primary: f64 = matched
            .iter()
            .map(|&(_, df)| (n / df as f64).ln().powi(2))
            .sum();
        let secondary = matched
            .iter()
            .map(|&(t, _)| field_weight(skill, t))
            .max()
            .unwrap_or(0);
        let candidate = Candidate {
            skill,
            primary,
            secondary,
        };
        if matched.len() == present.len() {
            full.push(candidate);
        } else {
            partial.push(candidate);
        }
    }

    // Full-AND matches always outrank partials: every additional matched
    // term adds a non-negative (ln(N/df))² to `primary`, so a full match's
    // primary is ≥ any partial's, with `secondary` (then name) breaking
    // ties. Merge the pools and sort once — partials backfill the slots the
    // full matches leave open instead of being dropped outright.
    let mut pool = full;
    pool.append(&mut partial);
    pool.sort_by(|a, b| {
        b.primary
            .partial_cmp(&a.primary)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.secondary.cmp(&a.secondary))
            .then(a.skill.name.cmp(&b.skill.name))
    });
    pool.into_iter().map(|c| c.skill).take(limit).collect()
}

/// One scored search result.
struct Candidate {
    skill: &'static SkillRecord,
    primary: f64,
    secondary: u8,
}

/// Upper bound on tokenized query terms handed to the O(terms × corpus)
/// scan; pathological multi-term queries (model lookup loops) truncate to
/// their lexicographically first 32.
const MAX_QUERY_TERMS: usize = 32;

/// Per-skill haystacks: SKILL_DB is a static table, so the lowercase
/// concatenations are built once per process instead of once per query
/// (lookup_skill runs in every agent round; ai_explain searches once per
/// rule plus its tool lookups).
static HAYSTACKS: std::sync::LazyLock<Vec<String>> =
    std::sync::LazyLock::new(|| SKILL_DB.iter().map(haystack).collect());

/// Lowercase, split on non-alphanumerics, drop 1-char tokens, dedupe.
fn tokenize(query: &str) -> Vec<String> {
    let mut terms: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(str::to_owned)
        .collect();
    terms.sort_unstable();
    terms.dedup();
    terms
}

/// Lowercased concatenation of all searchable fields.
fn haystack(skill: &SkillRecord) -> String {
    format!(
        "{} {} {} {} {}",
        skill.name.to_lowercase(),
        skill.domain.to_lowercase(),
        skill.description.to_lowercase(),
        skill.tool_type.to_lowercase(),
        skill.primary_tool.to_lowercase()
    )
}

/// Substring hit, or bidirectional prefix match between the term and a
/// haystack token when both are ≥5 chars (catches 'markduplicates' ↔
/// 'markdup', 'recalibration' ↔ 'recalibr' without flooding short tokens).
fn term_matches(term: &str, haystack: &str) -> bool {
    if haystack.contains(term) {
        return true;
    }
    if term.len() < 5 {
        return false;
    }
    haystack
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|tok| tok.len() >= 5)
        .any(|tok| tok.starts_with(term) || term.starts_with(tok))
}

/// Field-match strength: 3 name/primary_tool, 2 domain, 1 description/other.
fn field_weight(skill: &SkillRecord, term: &str) -> u8 {
    let name = skill.name.to_lowercase();
    let tool = skill.primary_tool.to_lowercase();
    if name.contains(term) || tool.contains(term) {
        return 3;
    }
    if skill.domain.to_lowercase().contains(term) {
        return 2;
    }
    1
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
    fn multi_word_bam_preprocessing_query_finds_duplicate_and_bqsr_skills() {
        // Live-failing agent query: none of the terms appear verbatim in any
        // skill, so the old single-substring search returned 0 hits.
        let results = search_skills("bam preprocessing markduplicates bqsr recalibration", 5);
        assert!(
            !results.is_empty(),
            "multi-word query must not return zero hits"
        );
        let names: Vec<&str> = results.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"bio-duplicate-handling"),
            "bio-duplicate-handling should rank for duplicate-marking terms, got {names:?}"
        );
        assert!(
            names.contains(&"bio-gatk-variant-calling"),
            "bio-gatk-variant-calling should rank for BQSR terms, got {names:?}"
        );
    }

    #[test]
    fn multi_word_gatk_joint_genotyping_query_ranks_variant_calling_first() {
        // Live-failing agent query: gVCF/reference-confidence vocabulary that
        // only appears inside the GATK skill description.
        let results = search_skills(
            "gatk haplotypecaller gvcf joint genotyping reference confidence interval",
            5,
        );
        assert!(!results.is_empty());
        assert_eq!(
            results[0].name,
            "bio-gatk-variant-calling",
            "rare GATK-specific terms must rank the variant-calling skill first, got {:?}",
            results.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn multi_word_fastq_screen_query_finds_contamination_screening() {
        // Live-failing agent query: 'fastq' and 'screen' are both generic on
        // their own; only their combination should surface the QC skill.
        let results = search_skills("fastq screen contamination", 5);
        assert!(!results.is_empty());
        assert_eq!(
            results[0].name,
            "bio-read-qc-contamination-screening",
            "contamination-screening terms must rank the QC skill first, got {:?}",
            results.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn multi_word_rnaseq_de_query_finds_differential_expression() {
        // Generality check: a common multi-word intent (not one of the three
        // live failures) must still find its domain skill.
        let results = search_skills("rna-seq differential expression", 5);
        assert!(!results.is_empty());
        assert!(
            results
                .iter()
                .any(|s| s.domain == "differential-expression"),
            "a differential-expression skill should match, got {:?}",
            results.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn mixed_real_and_hallucinated_terms_still_return_hits() {
        // A hallucinated term (zero document frequency) must be dropped
        // rather than killing the query — live agents invent vocabulary.
        // The old whole-query substring search returned 0 hits here.
        let results = search_skills("bam zzzznotaword", 5);
        assert!(
            !results.is_empty(),
            "a hallucinated term must not zero out a query with a real term"
        );
        let names: Vec<&str> = results.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"bio-bam-statistics"),
            "the real term 'bam' should surface bam skills, got {names:?}"
        );
    }

    #[test]
    fn query_with_no_full_match_falls_back_to_best_partials() {
        // No embedded skill matches both 'kraken2' and 'mutect2', so the
        // search must fall back to the partial pool instead of returning
        // nothing — each rare term's best skill should surface.
        let results = search_skills("kraken2 mutect2", 5);
        let names: Vec<&str> = results.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"bio-workflows-somatic-variant-pipeline"),
            "the only mutect2 skill should surface via partial fallback, got {names:?}"
        );
        assert!(
            names.contains(&"bio-metagenomics-kraken"),
            "the main kraken2 skill should surface via partial fallback, got {names:?}"
        );
    }

    #[test]
    fn empty_whitespace_and_zero_limit_queries_return_empty() {
        assert!(search_skills("", 5).is_empty());
        assert!(search_skills("   \t\n ", 5).is_empty());
        assert!(search_skills("bam", 0).is_empty());
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

    #[test]
    fn partial_matches_backfill_after_full_matches() {
        // 'markduplicates' fully matches the duplicate-handling skill while
        // the bam-statistics skill only matches 'bam' — the partial must
        // still appear (backfilled after the fulls) instead of being
        // dropped, and rank below the full match.
        let results = search_skills("bam markduplicates", 5);
        let names: Vec<&str> = results.iter().map(|s| s.name.as_str()).collect();
        let dup = names
            .iter()
            .position(|n| *n == "bio-duplicate-handling")
            .unwrap_or_else(|| panic!("the full match must surface, got {names:?}"));
        let stats = names
            .iter()
            .position(|n| *n == "bio-bam-statistics")
            .unwrap_or_else(|| panic!("the partial match must be backfilled, got {names:?}"));
        assert!(
            dup < stats,
            "full match must outrank the backfilled partial: {names:?}"
        );
    }

    #[test]
    fn pathological_query_is_capped_without_losing_the_signal_term() {
        // A model lookup loop can emit hundreds of terms; the cap keeps the
        // lexicographically first 32, and 'markduplicates' sorts before
        // every 'zzz' filler so the real signal survives.
        let mut query = "markduplicates".to_string();
        for i in 0..100 {
            query.push_str(&format!(" zzznoise{i:03}"));
        }
        let results = search_skills(&query, 3);
        assert!(
            results.iter().any(|s| s.name == "bio-duplicate-handling"),
            "the signal term must survive the 32-term cap, got {:?}",
            results.iter().map(|s| s.name.as_str()).collect::<Vec<_>>()
        );
    }
}
