//! Workflow-stage inference for metro-map diagram generation.
//!
//! Assigns each rule a *stage* (a named analysis phase such as "qc", "align",
//! "quantify") that becomes a colored "metro line" in the exported diagram.
//! Stages come from two sources, in priority order:
//!
//! 1. **Explicit**: the rule's first `tags` entry, normalized through
//!    [`canonical_stage`]. Unknown tags are kept verbatim (lowercased) so a
//!    custom stage still produces its own line.
//! 2. **Inferred**: keyword matching against the rule's `shell`/`script`
//!    commands (the same idea as `WorkflowDomain::detect`, at rule
//!    granularity).
//!
//! Rules that match nothing fall back to `"generic"`.

use crate::rule::Rule;

/// Canonical stage names in the order the palette assigns colors.
const PALETTE: &[(&str, &str, &str)] = &[
    // (stage, display name, color)
    ("qc", "Read QC", "#4C78A8"),
    ("trim", "Read Trimming", "#F58518"),
    ("align", "Alignment", "#54A24B"),
    ("quantify", "Quantification", "#E45756"),
    ("variant", "Variant Calling", "#B279A2"),
    ("annotate", "Annotation", "#72B7B2"),
    ("merge", "Merge / Combine", "#9D755D"),
    ("report", "Reporting", "#F2CF5B"),
    ("generic", "Analysis", "#79706E"),
];

/// Fallback colors for custom (non-canonical) stages, selected by stable hash.
const EXTRA_COLORS: &[&str] = &[
    "#9C755F", "#BAB0AC", "#FF9DA7", "#D4A6C8", "#86BCB6", "#59A14F", "#EDC948", "#8CD17D",
    "#B6992D", "#6E6E6E",
];

/// `(stage, keyword)` pairs for shell/script inference, matched against the
/// lowercased command text. Order matters: more specific signals (a variant
/// caller) win over generic ones (an aligner), mirroring `WorkflowDomain`.
const KEYWORDS: &[(&str, &str)] = &[
    // Reporting / aggregation — must precede QC so MultiQC is not misread as QC.
    ("report", "multiqc"),
    // Variant calling (specific callers before the broad "gatk").
    ("variant", "haplotypecaller"),
    ("variant", "mutect2"),
    ("variant", "freebayes"),
    ("variant", "strelka"),
    ("variant", "vardict"),
    ("variant", "bcftools call"),
    ("variant", "bcftools mpileup"),
    ("variant", "gatk"),
    // Annotation.
    ("annotate", "snpeff"),
    ("annotate", "snpsift"),
    ("annotate", "annovar"),
    ("annotate", "vep"),
    // Quantification.
    ("quantify", "featurecounts"),
    ("quantify", "htseq"),
    ("quantify", "salmon"),
    ("quantify", "kallisto"),
    ("quantify", "rsem"),
    ("quantify", "stringtie"),
    // Alignment.
    ("align", "bwa"),
    ("align", "star"),
    ("align", "hisat2"),
    ("align", "minimap2"),
    ("align", "bowtie"),
    ("align", "tophat"),
    // Trimming.
    ("trim", "trimmomatic"),
    ("trim", "trim_galore"),
    ("trim", "cutadapt"),
    ("trim", "fastp"),
    // QC.
    ("qc", "fastqc"),
    ("qc", "fastq_screen"),
    // Merge / concatenation (substring " cat"/" merge"/" concat" to avoid
    // matching "scatter" or "concatenate" noise).
    ("merge", "samtools merge"),
    ("merge", "samtools cat"),
    ("merge", "bcftools merge"),
    ("merge", "bcftools concat"),
    ("merge", "picard merge"),
];

/// Normalize a user-supplied tag into a canonical stage name.
///
/// Recognized synonyms collapse onto the canonical set so that a tag like
/// `alignment` and inferred `align` share one line. Unrecognized tags are
/// returned lowercased as custom stages.
pub fn canonical_stage(tag: &str) -> String {
    let t = tag.trim().to_lowercase();
    let canonical = match t.as_str() {
        "qc" | "quality" | "quality_control" | "quality-control" | "read_qc" | "readqc"
        | "fastqc" => "qc",
        "trim" | "trimming" | "trimmer" => "trim",
        "align" | "alignment" | "mapping" | "map" => "align",
        "quant" | "quantify" | "quantification" | "count" | "counting" | "expression"
        | "counts" => "quantify",
        "variant" | "variant_calling" | "variant-calling" | "calling" | "snv" | "germline"
        | "somatic" => "variant",
        "annotate" | "annotation" | "annovar" => "annotate",
        "merge" | "combine" | "gather" | "concatenate" | "concat" => "merge",
        "report" | "reporting" | "summary" | "multiqc" => "report",
        "generic" | "analysis" => "generic",
        _ => return t,
    };
    canonical.to_string()
}

/// Infer a rule's stage: explicit `tags` first, then shell/script keywords,
/// then `"generic"`.
pub fn detect_stage(rule: &Rule) -> String {
    // 1. Explicit tag (the rule's own categorization wins over inference).
    if let Some(tag) = rule.tags.first() {
        let t = tag.trim();
        if !t.is_empty() {
            return canonical_stage(t);
        }
    }

    // 2. Command keyword inference.
    let mut text = String::new();
    if let Some(shell) = &rule.shell {
        text.push_str(shell);
        text.push(' ');
    }
    if let Some(script) = &rule.script {
        text.push_str(script);
    }
    let text = text.to_lowercase();
    for (stage, keyword) in KEYWORDS {
        if text.contains(keyword) {
            return (*stage).to_string();
        }
    }

    // 3. Fallback.
    "generic".to_string()
}

/// Human-readable display name for a stage.
pub fn stage_display(stage: &str) -> String {
    PALETTE
        .iter()
        .find(|(s, _, _)| *s == stage)
        .map(|(_, display, _)| (*display).to_string())
        .unwrap_or_else(|| stage.to_string())
}

/// A stable color for a stage. Canonical stages use a fixed palette; custom
/// stages hash into the fallback palette so the same name always gets the
/// same color.
pub fn stage_color(stage: &str) -> &'static str {
    if let Some((_, _, color)) = PALETTE.iter().find(|(s, _, _)| *s == stage) {
        return color;
    }
    EXTRA_COLORS[stable_hash(stage) % EXTRA_COLORS.len()]
}

/// DJB2-style hash — deterministic across runs, so diagram colors are stable.
fn stable_hash(s: &str) -> usize {
    let mut h: usize = 5381;
    for b in s.bytes() {
        h = h.wrapping_mul(33).wrapping_add(usize::from(b));
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::{EnvironmentSpec, Resources};
    use std::collections::HashMap;

    fn rule(name: &str, shell: &str, tags: Vec<&str>) -> Rule {
        Rule {
            name: name.to_string(),
            shell: Some(shell.to_string()),
            tags: tags.into_iter().map(String::from).collect(),
            resources: Resources::default(),
            environment: EnvironmentSpec::default(),
            params: HashMap::new(),
            ..Default::default()
        }
    }

    #[test]
    fn explicit_tag_wins() {
        let r = rule("align_step", "bwa mem ref.fa in.fq > out.sam", vec!["qc"]);
        assert_eq!(detect_stage(&r), "qc");
    }

    #[test]
    fn tag_synonym_normalizes() {
        let r = rule("align_step", "bwa mem ref.fa in.fq", vec!["alignment"]);
        assert_eq!(detect_stage(&r), "align");
    }

    #[test]
    fn unknown_tag_kept_as_custom_stage() {
        let r = rule("impute_step", "impute2", vec!["imputation"]);
        assert_eq!(detect_stage(&r), "imputation");
    }

    #[test]
    fn shell_inference() {
        assert_eq!(detect_stage(&rule("a", "fastqc reads.fq", vec![])), "qc");
        assert_eq!(
            detect_stage(&rule("b", "bwa mem ref.fa r.fq", vec![])),
            "align"
        );
        assert_eq!(
            detect_stage(&rule("c", "featureCounts -a gtf -o c.txt", vec![])),
            "quantify"
        );
        assert_eq!(
            detect_stage(&rule("d", "gatk HaplotypeCaller -R ref", vec![])),
            "variant"
        );
        assert_eq!(detect_stage(&rule("e", "multiqc .", vec![])), "report");
    }

    #[test]
    fn no_signal_falls_back_to_generic() {
        let r = rule("mystery", "echo hello", vec![]);
        assert_eq!(detect_stage(&r), "generic");
    }

    #[test]
    fn canonical_colors_are_stable() {
        assert_eq!(stage_color("qc"), "#4C78A8");
        assert_eq!(stage_color("custom_thing"), stage_color("custom_thing"));
    }
}
