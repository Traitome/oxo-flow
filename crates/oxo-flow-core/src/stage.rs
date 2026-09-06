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
use std::collections::HashSet;

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
///
/// Okabe-Ito (colour-blind safe) values plus two legacy light tones. The
/// entries are mutually distinct so several custom-stage lines on one map
/// stay separable (live: community enrichment has eight custom stage
/// lines; the previous palette clustered them in the brown/pink family).
const EXTRA_COLORS: &[&str] = &[
    "#E69F00", "#56B4E9", "#009E73", "#F0E442", "#0072B2", "#D55E00", "#CC79A7", "#8CD17D",
    "#B6992D", "#999999",
];

/// `(rule-name prefix, canonical stage)` pairs for ported workflows whose
/// rules are namespaced as `module::rule` (e.g. `alignment::star_align`).
/// The module is the workflow author's own grouping — the strongest signal
/// after an explicit tag. Unknown prefixes fall through to keyword
/// inference rather than becoming custom stages (module names are
/// workflow-internal; the palette should stay canonical).
const PREFIX_STAGES: &[(&str, &str)] = &[
    ("fastq_qc", "qc"),
    ("bam_qc", "qc"),
    ("qc", "qc"),
    ("alignment", "align"),
    ("align", "align"),
    ("trim", "trim"),
    ("quantification", "quantify"),
    ("quant", "quantify"),
    ("variant", "variant"),
    ("annotate", "annotate"),
    ("annotation", "annotate"),
    ("merge", "merge"),
    ("report", "report"),
    ("multiqc", "report"),
];

/// `(stage, keyword, tool display)` pairs for shell/script inference, matched
/// against the lowercased command text. Order matters: more specific signals
/// (a variant caller) win over generic ones (an aligner), mirroring
/// `WorkflowDomain`. The tool display is the curated process name used when
/// the metro export groups rules into process-level stations (granularity
/// `process`): several rules driven by the same tool collapse into one
/// station named after the tool, the nf-core transit-map idiom
/// (`samtools sort`/`samtools index`/… → one "SAMtools" stop).
const KEYWORDS: &[(&str, &str, &str)] = &[
    // Reporting / aggregation — must precede QC so MultiQC is not misread as QC.
    ("report", "multiqc", "MultiQC"),
    // Variant calling (specific callers before the broad "gatk").
    ("variant", "haplotypecaller", "HaplotypeCaller"),
    ("variant", "mutect2", "Mutect2"),
    ("variant", "freebayes", "freeBayes"),
    ("variant", "strelka", "Strelka"),
    ("variant", "vardict", "VarDict"),
    ("variant", "bcftools call", "BCFtools"),
    ("variant", "bcftools mpileup", "BCFtools"),
    ("variant", "gatk", "GATK"),
    // Alignment-adjacent BAM processing (MarkDuplicates is the hub rule
    // most downstream stages consume — staging it as align keeps the
    // stage flow acyclic, live: community rnaseq).
    ("align", "picard", "Picard"),
    // Annotation.
    ("annotate", "snpeff", "SnpEff"),
    ("annotate", "snpsift", "SnpSift"),
    ("annotate", "annovar", "ANNOVAR"),
    ("annotate", "vep", "VEP"),
    // Quantification.
    ("quantify", "featurecounts", "featureCounts"),
    ("quantify", "htseq", "HTSeq"),
    ("quantify", "salmon quant", "Salmon"),
    ("quantify", "kallisto", "Kallisto"),
    ("quantify", "rsem", "RSEM"),
    ("quantify", "stringtie", "StringTie"),
    // Alignment.
    ("align", "bwa", "BWA"),
    ("align", "star ", "STAR"),
    ("align", "samtools sort", "SAMtools"),
    ("align", "samtools index", "SAMtools"),
    ("align", "samtools stats", "SAMtools"),
    ("align", "samtools flagstat", "SAMtools"),
    ("align", "samtools idxstats", "SAMtools"),
    ("align", "hisat2", "HISAT2"),
    ("align", "minimap2", "minimap2"),
    ("align", "bowtie", "Bowtie2"),
    ("align", "tophat", "TopHat"),
    // Trimming.
    ("trim", "trimmomatic", "Trimmomatic"),
    ("trim", "trim_galore", "Trim Galore!"),
    ("trim", "cutadapt", "cutadapt"),
    ("trim", "fastp", "fastp"),
    // QC.
    ("qc", "fastqc", "FastQC"),
    ("qc", "fastq_screen", "FastQ Screen"),
    ("qc", "qualimap", "Qualimap"),
    ("qc", "fq_lint", "fq lint"),
    ("qc", "fqlint", "fq lint"),
    // Merge / concatenation (substring " cat"/" merge"/" concat" to avoid
    // matching "scatter" or "concatenate" noise).
    ("merge", "samtools merge", "SAMtools"),
    ("merge", "bcftools merge", "BCFtools"),
    ("merge", "bcftools concat", "BCFtools"),
    ("merge", "picard merge", "Picard"),
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

    // 2. Rule-name prefix (module namespace, e.g. `alignment::star_align`).
    if let Some((prefix, _)) = rule.name.split_once("::")
        && let Some((_, stage)) = PREFIX_STAGES.iter().find(|(p, _)| *p == prefix)
    {
        return (*stage).to_string();
    }

    // 3. Command keyword inference.
    if let Some((stage, _, _)) = match_keyword(rule) {
        return stage.to_string();
    }

    // 4. Fallback.
    "generic".to_string()
}

/// The curated tool name for a rule, when its shell/script matches a known
/// tool keyword (`samtools index` → `Some("SAMtools")`). Rules without a
/// match stay individual stations in process-granularity metro exports.
/// Same matching order as [`detect_stage`] step 3.
pub fn detect_tool(rule: &Rule) -> Option<&'static str> {
    match_keyword(rule).map(|(_, _, tool)| tool)
}

/// First `(stage, keyword, tool)` match over the rule's shell/script text.
fn match_keyword(rule: &Rule) -> Option<(&'static str, &'static str, &'static str)> {
    let mut text = String::new();
    if let Some(shell) = &rule.shell {
        text.push_str(shell);
        text.push(' ');
    }
    if let Some(script) = &rule.script {
        text.push_str(script);
    }
    let text = text.to_lowercase();
    KEYWORDS
        .iter()
        .find(|(_, keyword, _)| text.contains(keyword))
        .copied()
}

/// Friendly section title for a module namespace (metro-map sections).
/// Known modules get curated titles; unknown ones are title-cased.
pub fn module_display(module: &str) -> String {
    let curated = match module {
        "fastq_qc" => "Read QC",
        "bam_qc" => "BAM QC",
        "alignment" => "Alignment",
        "quantification" => "Quantification",
        "prepare_genome" => "Reference preparation",
        "bigwig" => "Coverage tracks",
        "multiqc" | "report" => "Reporting",
        "trim" => "Trimming",
        "annotation" | "annotate" => "Annotation",
        "variant" => "Variant calling",
        "merge" => "Merge",
        _ => return title_case(module),
    };
    curated.to_string()
}

/// `fastq_qc` → `Fastq QC`, `91_wgs_callers` → `WGS Callers` (fallback for
/// unknown module names).
///
/// Leading numeric stage prefixes are dropped (`01_preprocessing` →
/// `Preprocessing`): they are file-organization noise on the map, not
/// flow meaning, and stay on the page's module table if the author wants
/// them. Known abbreviations normalize for readability — `qc` → `QC`,
/// `wgs` → `WGS`, `snv` → `SNV`, `cnv` → `CNV`, `vcf`/`maf` → `VCF`/`MAF`,
/// `rna`/`dna` → `RNA`/`DNA`, `sv` → `SV` — so a ported clindet map reads
/// `Somatic Callers + Germline + WGS Callers` instead of
/// `10 Somatic Callers + 20 Germline + 91 Wgs Callers`.
fn title_case(module: &str) -> String {
    let stripped = module
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '_' || c == '-' || c == '.');
    let word_case = stripped
        .split(['_', ' ', '-'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>();
    let normalized = word_case
        .iter()
        .map(|w| match w.to_ascii_lowercase().as_str() {
            "qc" => "QC",
            "wgs" => "WGS",
            "snv" => "SNV",
            "cnv" => "CNV",
            "vcf" => "VCF",
            "maf" => "MAF",
            "vcf2maf" => "VCF2MAF",
            "rna" => "RNA",
            "dna" => "DNA",
            "sv" => "SV",
            _ => w.as_str(),
        })
        .collect::<Vec<_>>();
    if normalized.is_empty() {
        module.to_string()
    } else {
        normalized.join(" ")
    }
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

/// Whether `stage` is a canonical palette stage (stable, table-driven
/// colour) rather than a workflow-specific custom stage.
pub fn is_canonical_stage(stage: &str) -> bool {
    PALETTE.iter().any(|(s, _, _)| *s == stage)
}

/// Colour for a stage line within one metro export.
///
/// Canonical stages keep their fixed palette entry. Custom stages start at
/// their stable-hash offset in the fallback palette and walk forward to the
/// first colour not yet used on this map, so a stage keeps its base colour
/// wherever possible while several custom lines on one map never share a
/// colour (live: community enrichment puts eight custom lines on one map,
/// where a bare hash modulo painted several of them the same colour).
pub fn metro_line_color(stage: &str, used: &mut HashSet<&'static str>) -> &'static str {
    if let Some((_, _, color)) = PALETTE.iter().find(|(s, _, _)| *s == stage) {
        // Canonical lanes keep their fixed colour and do not occupy slots in
        // the fallback walk — dense maps may share a hue with a canonical
        // lane (a problem of degree, never of correctness).
        return color;
    }
    let offset = stable_hash(stage) % EXTRA_COLORS.len();
    for k in 0..EXTRA_COLORS.len() {
        let color = EXTRA_COLORS[(offset + k) % EXTRA_COLORS.len()];
        if !used.contains(color) {
            used.insert(color);
            return color;
        }
    }
    // More custom lanes than fallback colours (live: community bgcflow at
    // rule granularity). Falling back to the stage's base colour is better
    // than aborting an otherwise-valid export.
    EXTRA_COLORS[offset % EXTRA_COLORS.len()]
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
    use std::collections::{HashMap, HashSet};

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
    fn rule_name_prefix_inference() {
        // Module namespaces map to canonical stages; the module is the
        // workflow author's own grouping (stronger than shell keywords).
        let r = rule("alignment::star_align", "echo unused", vec![]);
        assert_eq!(detect_stage(&r), "align");
        assert_eq!(
            detect_stage(&rule("fastq_qc::fq_lint", "echo", vec![])),
            "qc"
        );
        assert_eq!(
            detect_stage(&rule("quantification::salmon_quant", "echo", vec![])),
            "quantify"
        );
        // Unknown prefixes fall through to keyword inference, then generic.
        let r2 = rule("mystery::step", "echo nothing matches", vec![]);
        assert_eq!(detect_stage(&r2), "generic");
    }

    #[test]
    fn module_display_curated_and_fallback() {
        assert_eq!(module_display("fastq_qc"), "Read QC");
        assert_eq!(module_display("prepare_genome"), "Reference preparation");
        // Live: community ampliseq has a bare `report` module; the metro
        // section must use the stage display, not a bare "Report".
        assert_eq!(module_display("report"), "Reporting");
        assert_eq!(module_display("novel_module"), "Novel Module");
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
    fn detect_tool_returns_curated_display_name() {
        assert_eq!(
            detect_tool(&rule("a", "samtools sort in.bam", vec![])),
            Some("SAMtools")
        );
        assert_eq!(
            detect_tool(&rule("b", "STAR --genomeDir g", vec![])),
            Some("STAR")
        );
        assert_eq!(
            detect_tool(&rule("c", "multiqc .", vec![])),
            Some("MultiQC")
        );
        assert_eq!(detect_tool(&rule("d", "echo nothing", vec![])), None);
        // Tags/prefixes set the stage but are not tool identities.
        assert_eq!(detect_tool(&rule("e", "echo x", vec!["qc"])), None);
    }

    #[test]
    fn canonical_colors_are_stable() {
        assert_eq!(stage_color("qc"), "#4C78A8");
        assert_eq!(stage_color("custom_thing"), stage_color("custom_thing"));
    }

    #[test]
    fn module_display_reads_unknown_module_names_fluently() {
        // Ported workflows number their module files; the map must read
        // `91_wgs_callers` as `WGS Callers`, not raw file noise (live:
        // community clindet's overview map read "10 Somatic Callers +
        // 20 Germline + 91 Wgs Callers").
        assert_eq!(module_display("01_preprocessing"), "Preprocessing");
        assert_eq!(module_display("91_wgs_callers"), "WGS Callers");
        assert_eq!(module_display("80_cnv"), "CNV");
        assert_eq!(module_display("30_vcf_norm"), "VCF Norm");
        assert_eq!(module_display("60_vcf2maf"), "VCF2MAF");
        assert_eq!(module_display("00_common"), "Common");
        assert_eq!(module_display("70_unpaired"), "Unpaired");
        // Curated names and bare acronym modules still win.
        assert_eq!(module_display("fastq_qc"), "Read QC");
        assert_eq!(module_display("wgs"), "WGS");
    }

    #[test]
    fn metro_line_colors_never_collide_on_one_map() {
        // Eight custom stages on one export get eight distinct colours even
        // when their hash offsets coincide (the live community enrichment
        // case; a bare hash modulo painted several lines the same colour).
        let mut used = HashSet::new();
        let stages = [
            "databases",
            "great",
            "pycistarget",
            "gseapy",
            "plots",
            "aggregate",
            "visualize",
            "export",
        ];
        let colors: Vec<&str> = stages
            .iter()
            .map(|s| metro_line_color(s, &mut used))
            .collect();
        assert_eq!(colors.iter().collect::<HashSet<_>>().len(), colors.len());
    }

    #[test]
    fn metro_line_canonical_stages_keep_their_color() {
        let mut used = HashSet::new();
        assert_eq!(metro_line_color("qc", &mut used), "#4C78A8");
        assert_eq!(used.len(), 0, "canonical lanes take no fallback slot");
    }

    #[test]
    fn custom_stage_colour_pin_stays_stable() {
        // Pins the fallback walk's first assignment for one stage — a
        // palette shuffle or offset change must fail here, not silently
        // repaint regenerated maps.
        let mut used = HashSet::new();
        assert_eq!(metro_line_color("custom_tag", &mut used), "#8CD17D");
    }

    #[test]
    fn metro_line_more_custom_lanes_than_palette_never_aborts() {
        // A custom-stage-heavy map (live: community bgcflow, rule tier) must
        // export even when the fallback palette is exhausted — the last lane
        // repeats the stage's base colour instead of panicking.
        let mut used = HashSet::new();
        let mut colors = Vec::new();
        for i in 0..24 {
            colors.push(metro_line_color(&format!("custom_stage_{i}"), &mut used));
        }
        assert_eq!(colors.len(), 24);
    }
}
