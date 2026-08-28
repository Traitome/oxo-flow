//! Deterministic scientific-preflight checks for workflow designs.
//!
//! These are well-established constraints from authoritative tool
//! documentation (GATK best practices, subread) that can be detected from
//! the workflow definition itself — no execution required. They are most
//! valuable for pilot runs: a subset that would fail *scientifically*
//! (e.g. VQSR trained on 2 samples) is caught before hours are wasted.
//!
//! The checks are deliberately small and evidence-backed; AI commands use
//! these findings to produce plain-language explanations.

use crate::config::WorkflowConfig;

/// A scientific-design issue detected in a workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScientificWarning {
    /// Stable identifier, e.g. "SCI-VQSR-COHORT".
    pub code: String,
    /// Rule the issue applies to.
    pub rule: String,
    /// What is wrong, in plain language.
    pub message: String,
    /// Concrete remediation.
    pub suggestion: String,
}

/// GATK recommends a training cohort of ~30 samples for VQSR (whole
/// genomes or exomes); below that, hard filtering is the documented
/// alternative.
pub const MIN_VQSR_SAMPLES: usize = 30;

/// Total sample count in workflow order (deduplicated): pairs when the
/// workflow is pair-based, otherwise the union of all sample groups.
pub fn count_samples(config: &WorkflowConfig) -> usize {
    if !config.pairs.is_empty() {
        return config.pairs.len();
    }
    let mut seen: Vec<&str> = Vec::new();
    for group in &config.sample_groups {
        for sample in &group.samples {
            if !seen.contains(&sample.as_str()) {
                seen.push(sample);
            }
        }
    }
    seen.len()
}

/// Whether the shell references a featureCounts strandness flag
/// (`-s`, `-s0/1/2`, or `--stranded[=...]`). The default is unstranded.
fn has_strand_flag(shell: &str) -> bool {
    shell.split_whitespace().any(|token| {
        token == "-s"
            || token == "--stranded"
            || token.starts_with("--stranded=")
            || (token.starts_with("-s")
                && token.len() >= 3
                && token[2..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit()))
    })
}

/// Analyze a workflow for well-established scientific-design issues.
///
/// The sample count comes from the (possibly `--samples`-filtered)
/// config, so pilot runs are evaluated against the pilot cohort — the
/// scenario this preflight exists for.
pub fn analyze_scientific_constraints(config: &WorkflowConfig) -> Vec<ScientificWarning> {
    let mut warnings = Vec::new();
    let sample_count = count_samples(config);

    for rule in &config.rules {
        let Some(shell) = rule.shell.as_deref() else {
            continue;
        };
        // A rule whose `when` evaluates false under the effective config
        // will never run — diagnostics about how it would execute are
        // noise (issue #263: sarek's default-off Mutect2 templates fired
        // SCI-MUTECT2-TUMOR-ONLY on every dry-run). Baked per-instance
        // literals and absence-guard idioms re-evaluate identically here;
        // file_exists() resolves against the workflow root like everywhere
        // else. Expansion-time instance pruning already removed
        // non-instantiated instances — this covers template-level gates.
        if rule.when.as_deref().is_some_and(|when| {
            !crate::executor::process::evaluate_condition_with_wildcards_and_base_dir(
                when,
                &config.config,
                &std::collections::HashMap::new(),
                config.base_dir(),
            )
        }) {
            continue;
        }
        let shell_lower = shell.to_lowercase();

        if shell_lower.contains("variantrecalibrator") && sample_count < MIN_VQSR_SAMPLES {
            warnings.push(ScientificWarning {
                code: "SCI-VQSR-COHORT".into(),
                rule: rule.name.clone(),
                message: format!(
                    "VariantRecalibrator trains on the cohort, but only {sample_count} sample(s) \
                     are in this run — GATK recommends a minimum of ~{MIN_VQSR_SAMPLES}. \
                     The pilot will fail at this step for scientific reasons, not technical ones."
                ),
                suggestion: "stop the pilot before VQSR (-t <rule>) or use hard filtering \
                             (VariantFiltration with QD/FS/MQ) for small cohorts"
                    .into(),
            });
        }

        if shell_lower.contains("baserecalibrator") && !shell_lower.contains("--known-sites") {
            warnings.push(ScientificWarning {
                code: "SCI-BQSR-NO-KNOWN-SITES".into(),
                rule: rule.name.clone(),
                message: "BaseRecalibrator has no --known-sites resources — without them \
                          recalibration cannot model known variation."
                    .into(),
                suggestion: "supply dbSNP, Mills/1000G indels (and 1000G phase1 SNPs) for your \
                             reference build"
                    .into(),
            });
        }

        if shell_lower.contains("mutect2")
            && !shell_lower.contains("--normal")
            && !shell_lower.contains("--normal-sample")
        {
            warnings.push(ScientificWarning {
                code: "SCI-MUTECT2-TUMOR-ONLY".into(),
                rule: rule.name.clone(),
                message: "Mutect2 runs without a matched normal. GATK: tumor-only mode is \
                          supported but 'far inferior' to tumor-normal calling — a matched \
                          normal filters germline variants in a way population resources cannot."
                    .into(),
                suggestion: "pair each tumor with a matched normal; in tumor-only mode also \
                             supply --germline-resource and --panel-of-normals"
                    .into(),
            });
        }

        if shell_lower.contains("featurecounts") && !has_strand_flag(shell) {
            warnings.push(ScientificWarning {
                code: "SCI-FEATURECOUNTS-STRAND".into(),
                rule: rule.name.clone(),
                message: "featureCounts runs without an explicit strandness flag — the default \
                          is unstranded (-s 0), which miscounts stranded libraries."
                    .into(),
                suggestion: "set -s 2 (Illumina TruSeq/dUTP reverse) or -s 1 per your library \
                             protocol"
                    .into(),
            });
        }
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_rule(shell: &str) -> WorkflowConfig {
        let toml = format!(
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"r1\"\nshell = \"{shell}\"\n"
        );
        WorkflowConfig::parse(&toml).unwrap()
    }

    fn config_with_cohort(shell: &str, samples: &[&str]) -> WorkflowConfig {
        let samples_toml = samples
            .iter()
            .map(|s| format!("\"{s}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let toml = format!(
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[sample_groups]]\nname = \"cohort\"\nsamples = [{samples_toml}]\n\n[[rules]]\nname = \"r1\"\nshell = \"{shell}\"\n"
        );
        WorkflowConfig::parse(&toml).unwrap()
    }

    #[test]
    fn vqsr_warns_below_min_cohort() {
        let config = config_with_cohort(
            "gatk VariantRecalibrator -V variants.vcf.gz -O recal.vcf.gz",
            &["S1", "S2"],
        );
        let warnings = analyze_scientific_constraints(&config);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "SCI-VQSR-COHORT");
        assert!(warnings[0].message.contains("2 sample(s)"));
    }

    #[test]
    fn vqsr_silent_at_sufficient_cohort() {
        let samples: Vec<String> = (0..30).map(|i| format!("S{i}")).collect();
        let refs: Vec<&str> = samples.iter().map(String::as_str).collect();
        let config = config_with_cohort(
            "gatk VariantRecalibrator -V variants.vcf.gz -O recal.vcf.gz",
            &refs,
        );
        assert!(analyze_scientific_constraints(&config).is_empty());
    }

    #[test]
    fn bqsr_without_known_sites_warns() {
        let config = config_with_rule("gatk BaseRecalibrator -I in.bam -R ref.fa -O recal.table");
        let warnings = analyze_scientific_constraints(&config);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "SCI-BQSR-NO-KNOWN-SITES");

        let ok = config_with_rule(
            "gatk BaseRecalibrator -I in.bam -R ref.fa --known-sites dbsnp.vcf.gz -O recal.table",
        );
        assert!(analyze_scientific_constraints(&ok).is_empty());
    }

    #[test]
    fn mutect2_tumor_only_warns() {
        let config = config_with_rule("gatk Mutect2 -R ref.fa -I tumor.bam -O out.vcf.gz");
        let warnings = analyze_scientific_constraints(&config);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "SCI-MUTECT2-TUMOR-ONLY");

        let ok = config_with_rule(
            "gatk Mutect2 -R ref.fa -I tumor.bam -I normal.bam --normal-sample N1 -O out.vcf.gz",
        );
        assert!(analyze_scientific_constraints(&ok).is_empty());
    }

    #[test]
    fn when_gated_off_rules_produce_no_preflight_warnings() {
        // Issue #263: a somatic-caller rule gated off by config (the sarek
        // port's default) must not flood every dry-run with advice for a
        // rule that never executes in this run.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [config]
            call_mutect2 = false

            [[rules]]
            name = "mutect2_tumor_only"
            when = "config.call_mutect2"
            shell = "gatk Mutect2 -R ref.fa -I tumor.bam -O out.vcf.gz"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        assert!(analyze_scientific_constraints(&config).is_empty());

        // The gate ON: the same rule fires the diagnostic.
        let on = toml.replace("call_mutect2 = false", "call_mutect2 = true");
        let config = WorkflowConfig::parse(&on).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "SCI-MUTECT2-TUMOR-ONLY");

        // A true gate with base_dir-resolved file_exists stays evaluated.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("enable.marker"), b"x").unwrap();
        let wf = dir.path().join("wf.oxoflow");
        std::fs::write(
            &wf,
            "[workflow]
name = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"r1\"\nwhen = 'file_exists(\"enable.marker\")'\nshell = \"gatk Mutect2 -R ref.fa -I tumor.bam -O out.vcf.gz\"\n",
        )
        .unwrap();
        let config = WorkflowConfig::from_file(&wf).unwrap();
        assert_eq!(analyze_scientific_constraints(&config).len(), 1);
    }

    #[test]
    fn featurecounts_without_strand_warns() {
        let config = config_with_rule("featureCounts -a genes.gtf -o counts.txt aligned/S1.bam");
        let warnings = analyze_scientific_constraints(&config);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "SCI-FEATURECOUNTS-STRAND");

        for ok_shell in [
            "featureCounts -a genes.gtf -s 2 -o counts.txt aligned/S1.bam",
            "featureCounts -a genes.gtf -s2 -o counts.txt aligned/S1.bam",
            "featureCounts -a genes.gtf --stranded=reverse -o counts.txt aligned/S1.bam",
        ] {
            assert!(
                analyze_scientific_constraints(&config_with_rule(ok_shell)).is_empty(),
                "should not warn for {ok_shell}"
            );
        }
    }

    #[test]
    fn pair_based_workflow_counts_pairs() {
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[pairs]]
            pair_id = "P1"
            experiment = "T1"
            control = "N1"

            [[pairs]]
            pair_id = "P2"
            experiment = "T2"
            control = "N2"

            [[rules]]
            name = "r1"
            shell = "gatk VariantRecalibrator -V v.vcf.gz -O o.vcf.gz"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("2 sample(s)"));
    }
}
