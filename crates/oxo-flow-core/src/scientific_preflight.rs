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
use crate::rule::Rule;
use regex::Regex;
use std::sync::LazyLock;

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

/// Whether a single shell token is a featureCounts strandness flag
/// (`-s`, `-s0/1/2`, or `--stranded[=...]`). The default is unstranded.
fn is_strand_flag(token: &str) -> bool {
    token == "-s"
        || token == "--stranded"
        || token.starts_with("--stranded=")
        || (token.starts_with("-s")
            && token.len() >= 3
            && token[2..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit()))
}

/// Whether a word may be skipped when locating the command word of a shell
/// segment: leading `VAR=value` env assignments or file-descriptor
/// redirections like `2>/dev/null`.
fn is_leading_noise(word: &str) -> bool {
    if let Some((name, _)) = word.split_once('=')
        && !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return true;
    }
    word.starts_with(|c: char| c.is_ascii_digit()) && word.contains('>')
}

/// Whether the shell *invokes* featureCounts in a segment that carries no
/// strandness flag. featureCounts must appear as a command word — first
/// token of a command segment (after `;`, `&&`/`||`/`&`, `|`, newline, or
/// `$(`), skipping env assignments and redirections, compared by basename
/// — not merely mentioned in an argument or path. Issue #441: a rule
/// passing its output *directory* `.../featurecounts` to a python helper
/// fired the strand warning, while the real featureCounts rule was
/// correctly flagged as silent.
fn featurecounts_without_strand(shell: &str) -> bool {
    // Join bash line continuations so a multi-line command stays one
    // segment, then neutralize `$(...)` openers as segment boundaries.
    let normalized = shell
        .replace("\\\r\n", " ")
        .replace("\\\n", " ")
        .replace("$(", ";");
    normalized
        .split([';', '\n', '\r', '|', '&', '`'])
        .any(|segment| {
            let Some(command) = segment.split_whitespace().find(|w| !is_leading_noise(w)) else {
                return false;
            };
            let base = command.rsplit('/').next().unwrap_or(command);
            if !base.eq_ignore_ascii_case("featurecounts") {
                return false;
            }
            !segment.split_whitespace().any(is_strand_flag)
        })
}

/// Whether a Mutect2 command declares a matched normal. GATK accepts both
/// the double-dash and single-dash spellings (`--normal` / `-normal`,
/// `--normal-sample` / `-normal-sample`); the single-dash form is common in
/// ported workflows, and reading it as tumor-only produced a false
/// `SCI-MUTECT2-TUMOR-ONLY` warning. Token-exact matching keeps
/// `--normal-lod` (a tumor-only quality threshold) from counting as a
/// matched normal.
fn has_normal_flag(shell: &str) -> bool {
    shell.split_whitespace().any(|token| {
        matches!(
            token.split('=').next().unwrap_or(token),
            "-normal" | "--normal" | "-normal-sample" | "--normal-sample"
        )
    })
}

/// Fan-out trigger vocabulary mirrored from `expand.rs`: the fixed pair
/// wildcards plus the group wildcards. Pair metadata keys are appended per
/// workflow, mirroring how `expand_wildcards` builds its own list.
const PAIR_WILDCARDS: &[&str] = &[
    "experiment",
    "control",
    "tumor",
    "normal",
    "pair_id",
    "experiment_type",
    "tumor_type",
];

const GROUP_WILDCARDS: &[&str] = &["group", "sample"];

/// Placeholder-reference finder: `{name}` / `{values.name}` / `{meta.col}`
/// style references in a text field, used to tell which side of a rule
/// (fan-out trigger vs declared output) carries the sample identity.
static PLACEHOLDER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{(\w+(?:\.\w+)?)\}").expect("valid placeholder regex"));

/// Whether the rule takes one of the early paths in `expand_wildcards`
/// that never multiply per fan-out dimension (issue #443 exclusion set).
/// `input_groups` rules take the groupTuple-style path — the discovered
/// group key is the instance's binding source and their outputs are keyed
/// by it. `output_pattern` producers are distinguished by their own fresh
/// wildcard: every instance's pattern instantiation writes a distinct path,
/// so a wildcard-free `output` list is expected there. Fresh-wildcard
/// *consumers* are deliberately NOT excluded: the deferral projects one
/// instance per producer-domain element, so placeholder-free outputs race
/// exactly like the bare-input shape (they trigger via `triggers_fresh`).
fn bypasses_group_pair_fanout(rule: &Rule) -> bool {
    !rule.input_groups.is_empty() || rule.output_pattern.is_some()
}

/// Issue #443: detect rules that fan out per sample (or pair, values combo,
/// or producer-domain element) but declare instance-independent outputs —
/// the shape of a MultiQC-style aggregation rule keyed by `{sample}` in its
/// inputs. Expansion creates N instances writing identical output paths;
/// run concurrently they race (FileExistsError / FileNotFoundError),
/// sequentially they waste N−1 duplicate runs and the winner is arbitrary.
///
/// Mirrors the fan-out trigger semantics of `expand_wildcards` exactly:
///
/// - pair/group fan-out keys on `all_text` (inputs + outputs + shell +
///   `when`) and requires the respective domain to exist;
/// - `[[values]]` activation scans the same texts PLUS the `expand_inputs`
///   patterns (`active_value_tables_for_rule`) and fans out orthogonally —
///   including values-only workflows with no sample domain;
/// - a reference to an `output_pattern` producer's fresh wildcard (same
///   expanded scan) defers the template and projects one instance per
///   producer-domain element — a fan-out source in its own right, so no
///   sample/values domain is required for the warning;
/// - `{sample}` confined to an `expand_inputs` pattern is NOT a trigger —
///   the pattern materializes into one instance (the aggregation idiom);
/// - input_groups rules and output_pattern producers are excluded, and
///   `when`-gated-off rules never run (issue #263).
fn detect_aggregation_races(config: &WorkflowConfig) -> Vec<ScientificWarning> {
    // Pair metadata keys are part of the fan-out vocabulary.
    let mut pair_wildcards: Vec<&str> = PAIR_WILDCARDS.to_vec();
    for pair in &config.pairs {
        for key in pair.metadata.keys() {
            if !pair_wildcards.contains(&key.as_str()) {
                pair_wildcards.push(key.as_str());
            }
        }
    }

    // Fresh wildcards declared by output_pattern producers — the deferral
    // registry expand.rs builds before scanning consumers.
    let fresh_wildcards: Vec<String> = config
        .rules
        .iter()
        .filter_map(|r| r.output_pattern.as_deref())
        .filter_map(|op| crate::wildcard::extract_wildcards(op).into_iter().next())
        .collect();

    let mut warnings = Vec::new();
    for rule in &config.rules {
        // `when`-gated-off rules never run — no diagnostic (issue #263).
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
        if bypasses_group_pair_fanout(rule) {
            continue;
        }

        // Mirror expand.rs's trigger texts. Pair/group fan-out keys on
        // `all_text` only; values-table activation and fresh-wildcard
        // consumer deferral scan `all_text` PLUS the expand_inputs
        // patterns. Group wildcards deliberately stay out of the
        // expand_texts scan: `{sample}` confined to an expand_inputs
        // pattern materializes into one instance, not per-sample fan-out.
        let all_text: Vec<&str> = rule
            .input
            .iter()
            .map(String::as_str)
            .chain(rule.output.iter().map(String::as_str))
            .chain(rule.shell.as_deref())
            .chain(rule.when.as_deref())
            .collect();
        let expand_texts: Vec<&str> = all_text
            .iter()
            .copied()
            .chain(rule.expand_inputs.iter().map(|e| e.pattern.as_str()))
            .collect();

        let triggers_pair = !config.pairs.is_empty()
            && all_text.iter().any(|t| {
                pair_wildcards
                    .iter()
                    .any(|w| t.contains(&format!("{{{w}}}")))
            });
        let triggers_group = !config.sample_groups.is_empty()
            && all_text.iter().any(|t| {
                GROUP_WILDCARDS
                    .iter()
                    .any(|w| t.contains(&format!("{{{w}}}")))
            });
        let triggers_values = config.values.iter().any(|v| {
            expand_texts.iter().any(|t| {
                t.contains(&format!("{{{}}}", v.name))
                    || t.contains(&format!("{{values.{}}}", v.name))
            })
        });
        let triggers_fresh = fresh_wildcards
            .iter()
            .any(|w| expand_texts.iter().any(|t| t.contains(&format!("{{{w}}}"))));
        if !(triggers_pair || triggers_group || triggers_values || triggers_fresh) {
            continue;
        }

        // Wildcard-free declared outputs → every expanded instance writes
        // the same paths.
        let outputs_have_wildcards = rule.output.iter().any(|o| PLACEHOLDER_RE.is_match(o));
        if outputs_have_wildcards {
            continue;
        }

        // Directory-output aggregation is a deliberate convergence shape,
        // not a race (live false positive: mag's convert_depths_{spades,
        // megahit}): each instance writes distinct per-sample files into
        // its own scratch dir and additively `mv`s them into the shared
        // output directory — instances never open the same path twice.
        // The directory part must be literal (no wildcard left in it, so
        // every instance targets the SAME dir) and must not be duplicated
        // as a whole by another fan-out wildcard.
        let has_dir_output = rule
            .output
            .iter()
            .any(|o| crate::dag::looks_like_directory(o) && !PLACEHOLDER_RE.is_match(o));
        if has_dir_output {
            continue;
        }

        warnings.push(ScientificWarning {
            code: "SCI-AGG-RACE".into(),
            rule: rule.name.clone(),
            message: "rule fans out per instance (a fan-out wildcard — {sample}/{group}, a pair \
                      wildcard, a [[values]] table, or an output_pattern producer's fresh \
                      wildcard — appears in its inputs/shell/when/expand_inputs) but its \
                      declared outputs contain no wildcard — expansion creates one instance \
                      per fan-out element, all writing the same output path(s): run \
                      concurrently they race (FileExistsError/missing-file crashes), \
                      sequentially they duplicate work N−1 times and the surviving result \
                      depends on scheduling order."
                .into(),
            suggestion: "remove the fan-out wildcard from this aggregation rule's inputs and \
                         reference the per-instance files via expand_inputs (e.g. \
                         expand_inputs = [{pattern = \"qc/{sample}/fastqc.html\"}]) or a \
                         glob/grouped input — the rule then runs once over all elements, or \
                         key its outputs by the wildcard so every instance writes a distinct \
                         path"
                .into(),
        });
    }
    warnings
}

/// Analyze a workflow for well-established scientific-design issues.
///
/// The sample count comes from the (possibly `--samples`-filtered)
/// config, so pilot runs are evaluated against the pilot cohort — the
/// scenario this preflight exists for.
pub fn analyze_scientific_constraints(config: &WorkflowConfig) -> Vec<ScientificWarning> {
    let mut warnings = Vec::new();
    let sample_count = count_samples(config);
    warnings.extend(detect_aggregation_races(config));

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

        if shell_lower.contains("mutect2") && !has_normal_flag(&shell_lower) {
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

        // Issue #441: only fire when featureCounts is actually *invoked*
        // (command-word position), not mentioned in an argument or path.
        // A config that already declares `strandedness = "unstranded"` is
        // honoring the default deliberately — the default IS `-s 0`, so
        // warning would push a correct design toward a wrong one. Gated by
        // condition, not `continue`: this sits mid-loop and a `continue`
        // would silently suppress every check appended after it.
        let strandness_suppressed = config.config.get("strandedness").is_some_and(|v| {
            v.as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case("unstranded"))
        });
        if shell_lower.contains("featurecounts")
            && !strandness_suppressed
            && featurecounts_without_strand(shell)
        {
            warnings.push(ScientificWarning {
                code: "SCI-FEATURECOUNTS-STRAND".into(),
                rule: rule.name.clone(),
                message: "featureCounts runs without an explicit strandness flag — the default \
                          is unstranded (-s 0), which miscounts stranded libraries."
                    .into(),
                suggestion: "determine your library's strandedness from evidence before \
                             choosing — run RSeQC infer_experiment.py on an alignment (or \
                             check the samplesheet's strandedness column); then set -s 1 \
                             (forward) or -s 2 (reverse/dUTP) per that evidence. Leave it \
                             unset only if the library is verified unstranded"
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

        // GATK also accepts the single-dash spellings (the form ported
        // workflows use, live: examples/gallery 14/15 `-normal CTRL_01`).
        for ok_shell in [
            "gatk Mutect2 -R ref.fa -I tumor.bam -normal N1 -O out.vcf.gz",
            "gatk Mutect2 -R ref.fa -I tumor.bam -normal-sample N1 -O out.vcf.gz",
        ] {
            assert!(
                analyze_scientific_constraints(&config_with_rule(ok_shell)).is_empty(),
                "a matched normal must suppress the tumor-only warning: {ok_shell}"
            );
        }

        // `--normal-lod` is a tumor-only quality threshold, not a matched
        // normal — it must keep warning.
        let lod =
            config_with_rule("gatk Mutect2 -R ref.fa -I tumor.bam --normal-lod 3.0 -O out.vcf.gz");
        let warnings = analyze_scientific_constraints(&lod);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "SCI-MUTECT2-TUMOR-ONLY");
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
        // Evidence-driven suggestion (issue #441): never prescribe a bare
        // "-s 2" — unstranded libraries are correct without the flag.
        assert!(!warnings[0].suggestion.contains("set -s 2"));

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
    fn featurecounts_path_mention_does_not_warn() {
        // Issue #441: bam_qc::biotype_multiqc passes its OUTPUT DIRECTORY
        // `.../featurecounts` to a python helper — featureCounts is never
        // invoked, so the strand check must stay silent.
        let config = config_with_rule(
            "python scripts/mqc_features_stat.py {input[0]} {input[1]} {sample} \
             {config.out_dir}/{config.aligner}/featurecounts",
        );
        assert!(analyze_scientific_constraints(&config).is_empty());

        // Same for the binary appearing as an argument of another tool,
        // possibly with a path prefix.
        let arg = config_with_rule("multiqc . --cl-config featurecounts/count.txt");
        assert!(analyze_scientific_constraints(&arg).is_empty());
    }

    #[test]
    fn featurecounts_invocation_forms_warn() {
        // Command-word forms that must still fire: direct, absolute path,
        // env assignment prefix, pipe segment, and a second command after
        // `;`. Each carries no strand flag.
        for shell in [
            "featureCounts -a genes.gtf -o counts.txt aligned/S1.bam",
            "/usr/local/bin/featureCounts -a genes.gtf -o counts.txt in.bam",
            "TMPDIR=/tmp featureCounts -a genes.gtf -o counts.txt in.bam",
            "samtools sort -o in.sorted.bam in.bam | featureCounts -o counts.txt",
            "mkdir -p out; featureCounts -a genes.gtf -o out/counts.txt in.bam",
        ] {
            let warnings = analyze_scientific_constraints(&config_with_rule(shell));
            assert_eq!(warnings.len(), 1, "should warn for {shell}");
            assert_eq!(warnings[0].code, "SCI-FEATURECOUNTS-STRAND");
        }

        // A strand flag in the invoking segment silences it — even when
        // the command continues with `&&`.
        let ok = config_with_rule(
            "featureCounts -a genes.gtf -s 2 -o counts.txt in.bam && gzip counts.txt",
        );
        assert!(analyze_scientific_constraints(&ok).is_empty());

        // The flag belongs to a different tool in the segment → still warns.
        let mixed = config_with_rule(
            "gatk PrintReads -s 2 -I in.bam -O out.bam; featureCounts -o counts.txt in.bam",
        );
        let warnings = analyze_scientific_constraints(&mixed);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "SCI-FEATURECOUNTS-STRAND");
    }

    #[test]
    fn featurecounts_unstranded_config_is_silent() {
        // Issue #441: the pilot's libraries ARE unstranded and the config
        // says so — the check must not push "-s 2" onto a correct design.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [config]
            strandedness = "unstranded"

            [[rules]]
            name = "counts"
            shell = "featureCounts -a genes.gtf -o counts.txt aligned/S1.bam"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        assert!(analyze_scientific_constraints(&config).is_empty());

        // A strandedness value that is NOT 'unstranded' (e.g. per-sample
        // metadata with an 'auto' fallback) keeps the check active.
        let auto = toml.replace("unstranded", "auto");
        let config = WorkflowConfig::parse(&auto).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "SCI-FEATURECOUNTS-STRAND");
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

    // ── Issue #443: aggregation-rule fan-out race (SCI-AGG-RACE) ─────────

    #[test]
    fn agg_rule_sample_inputs_wildcard_free_outputs_warns() {
        // The tutorial's racing shape: a MultiQC-style aggregation rule
        // keyed by {sample} in its inputs with wildcard-free outputs.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2", "S3"]

            [[rules]]
            name = "multiqc"
            input = ["qc/{sample}/fastqc.html"]
            output = ["report/multiqc.html"]
            shell = "multiqc -f qc -o report"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "SCI-AGG-RACE");
        assert_eq!(warnings[0].rule, "multiqc");
    }

    #[test]
    fn agg_race_silent_without_sample_domain() {
        // No [[sample_groups]]/[[pairs]] → nothing fans out; the rule
        // stays a single task and wildcard-free outputs are correct.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[rules]]
            name = "multiqc"
            input = ["qc/S1/fastqc.html"]
            output = ["report/multiqc.html"]
            shell = "multiqc -f qc -o report"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        assert!(analyze_scientific_constraints(&config).is_empty());
    }

    #[test]
    fn agg_race_silent_when_outputs_carry_sample() {
        // A per-sample rule (wildcards on BOTH sides) is the normal
        // fan-out shape — must never warn.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2"]

            [[rules]]
            name = "fastqc"
            input = ["raw/{sample}_R1.fastq.gz"]
            output = ["qc/{sample}/fastqc.html"]
            shell = "fastqc raw/{sample}_R1.fastq.gz -o qc/{sample}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        assert!(analyze_scientific_constraints(&config).is_empty());
    }

    #[test]
    fn agg_race_silent_for_corrected_aggregation_idiom() {
        // The docs' corrected form: a single-keyed input (no fan-out
        // wildcard) over explicitly-listed per-sample files.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2"]

            [[rules]]
            name = "multiqc"
            input = ["qc/S1/fastqc.html", "qc/S2/fastqc.html"]
            output = ["report/multiqc.html"]
            shell = "multiqc -f qc -o report"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        assert!(analyze_scientific_constraints(&config).is_empty());
    }

    #[test]
    fn agg_race_silent_for_expand_inputs_aggregation() {
        // The suggested remediation itself: per-sample fan-out producer +
        // an aggregation rule using expand_inputs (no bare {sample}).
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2"]

            [[rules]]
            name = "fastqc"
            input = ["raw/{sample}_R1.fastq.gz"]
            output = ["qc/{sample}/fastqc.html"]
            shell = "fastqc raw/{sample}_R1.fastq.gz -o qc/{sample}"

            [[rules]]
            name = "multiqc"
            input = ["placeholder.txt"]
            expand_inputs = [{ pattern = "qc/{sample}/fastqc.html" }]
            output = ["report/multiqc.html"]
            shell = "multiqc -f qc -o report"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        assert!(analyze_scientific_constraints(&config).is_empty());
    }

    #[test]
    fn agg_race_silent_for_input_groups_rule() {
        // input_groups rules take the groupTuple-style path and never
        // fan out on {sample} — their wildcard-free outputs are the point.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2"]

            [[rules]]
            name = "merge_lanes"
            input = ["placeholder.txt"]
            input_groups = [{ pattern = "raw/{sample}_L{lane}.fastq.gz", group_by = "sample" }]
            output = ["merged/{sample}.fastq.gz"]
            shell = "cat {input} > merged/{sample}.fastq.gz"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert!(
            warnings.iter().all(|w| w.code != "SCI-AGG-RACE"),
            "input_groups rules must not fire SCI-AGG-RACE: {warnings:?}"
        );
    }

    #[test]
    fn agg_race_output_pattern_producer_silent_consumer_with_keyed_output_silent() {
        // output_pattern producers keep their fresh wildcard unbound and
        // are distinguished by it — no race. A consumer whose outputs
        // carry the fresh wildcard writes a distinct path per projected
        // instance — no race either.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[rules]]
            name = "discover"
            output_pattern = "assemblies/{assembler}/contigs.fa"
            shell = "echo discovering"

            [[rules]]
            name = "assemble"
            input = ["placeholder.txt"]
            expand_inputs = [{ pattern = "assemblies/{assembler}/contigs.fa" }]
            output = ["assemblies/{assembler}/annotated.fa"]
            shell = "annotate {input} > {output}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert!(
            warnings.iter().all(|w| w.code != "SCI-AGG-RACE"),
            "keyed output_pattern consumer must not fire SCI-AGG-RACE: {warnings:?}"
        );
    }

    #[test]
    fn agg_race_output_pattern_consumer_placeholder_free_outputs_warns() {
        // The deferred consumer is projected one instance per
        // producer-domain element (expand.rs instantiate_output_pattern_
        // consumer); placeholder-free outputs make every projected
        // instance write the same path — the same race as the bare-input
        // shape, discovered-domain edition.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[rules]]
            name = "discover"
            output_pattern = "assemblies/{assembler}/contigs.fa"
            shell = "echo discovering"

            [[rules]]
            name = "assemble"
            input = ["placeholder.txt"]
            expand_inputs = [{ pattern = "assemblies/{assembler}/contigs.fa" }]
            output = ["assemblies/merged.fa"]
            shell = "cat {input} > {output}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(warnings[0].code, "SCI-AGG-RACE");
        assert_eq!(warnings[0].rule, "assemble");
    }

    #[test]
    fn agg_race_fires_via_shell_only_reference() {
        // The trigger set is inputs+outputs+shell+when: a {sample} only in
        // the shell also fans out (mirrors expand.rs's all_text).
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2"]

            [[rules]]
            name = "summary"
            input = ["done.marker"]
            output = ["summary.txt"]
            shell = "cat qc/{sample}/metrics.txt > summary.txt"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "SCI-AGG-RACE");
    }

    #[test]
    fn agg_race_fires_for_pair_wildcard_rules() {
        // Pair workflows fan out over pair wildcards too.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[pairs]]
            pair_id = "P1"
            experiment = "T1"
            control = "N1"

            [[rules]]
            name = "cnv_report"
            input = ["segments/{pair_id}.tsv"]
            output = ["report/cnv.html"]
            shell = "python render.py segments/{pair_id}.tsv report/cnv.html"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "SCI-AGG-RACE");
    }

    #[test]
    fn agg_race_silent_for_when_gated_off_rule() {
        // Issue #263 stance: a rule gated off never runs — no warning.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [config]
            make_report = false

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2"]

            [[rules]]
            name = "multiqc"
            input = ["qc/{sample}/fastqc.html"]
            output = ["report/multiqc.html"]
            when = "config.make_report"
            shell = "multiqc -f qc -o report"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert!(warnings.iter().all(|w| w.code != "SCI-AGG-RACE"));

        let on = toml.replace("make_report = false", "make_report = true");
        let config = WorkflowConfig::parse(&on).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert_eq!(
            warnings.iter().filter(|w| w.code == "SCI-AGG-RACE").count(),
            1
        );
    }

    #[test]
    fn agg_race_counts_meta_and_values_references_as_triggers() {
        // {meta.col} / {values.name} references also participate in the
        // fan-out vocabulary — a rule fanned on them with wildcard-free
        // outputs has the same race.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2"]

            [sample_groups.metadata]
            tissue = "tumor"

            [[values]]
            name = "assembler"
            values = ["spades", "megahit"]

            [[rules]]
            name = "assembled_report"
            input = ["asm/{assembler}/contigs.fa"]
            output = ["report/asm.html"]
            shell = "python quast_report.py asm/{assembler}/contigs.fa report/asm.html"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "SCI-AGG-RACE");
    }

    #[test]
    fn agg_race_values_only_workflow_warns() {
        // [[values]] fan-out works with no sample domain at all (the
        // engine's value-only branch ignores pairs/groups): a rule fanned
        // per assembler with wildcard-free outputs races exactly like the
        // sample shape.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[values]]
            name = "assembler"
            values = ["spades", "megahit"]

            [[rules]]
            name = "assembly_report"
            input = ["asm/{assembler}/stats.txt"]
            output = ["report/asm.html"]
            shell = "python quast_report.py asm/{assembler}/stats.txt report/asm.html"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(warnings[0].code, "SCI-AGG-RACE");
    }

    #[test]
    fn agg_race_silent_when_values_present_but_unreferenced() {
        // A [[values]] table existing somewhere is not a trigger by
        // itself: the rule must reference it.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[values]]
            name = "assembler"
            values = ["spades", "megahit"]

            [[rules]]
            name = "summary"
            input = ["stats/all.txt"]
            output = ["report/summary.html"]
            shell = "python render.py stats/all.txt report/summary.html"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        assert!(analyze_scientific_constraints(&config).is_empty());
    }

    #[test]
    fn agg_race_values_reference_confined_to_expand_inputs_warns() {
        // The engine activates a [[values]] table from expand_inputs
        // patterns too (active_value_tables_for_rule) and fans the rule
        // per combo — with wildcard-free outputs every instance writes
        // the same path. The bare-input scan must not miss it.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[values]]
            name = "assembler"
            values = ["spades", "megahit"]

            [[rules]]
            name = "assembly_report"
            input = ["done.marker"]
            expand_inputs = [{ pattern = "asm/{assembler}/stats.txt" }]
            output = ["report/asm.html"]
            shell = "python quast_report.py {input} report/asm.html"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(warnings[0].code, "SCI-AGG-RACE");
    }

    #[test]
    fn agg_race_silent_for_literal_dir_output_split_into_scratch() {
        // Live false positive (mag convert_depths_{spades,megahit}): a
        // per-sample rule writes distinct per-sample files in its own
        // scratch dir, then additively `mv`s them into a shared literal
        // output DIRECTORY. Instances never open the same path — the
        // dir-output convergence shape is safe, not a race.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2"]

            [[rules]]
            name = "convert_depths"
            input = ["depths/SPAdes-{sample}-depth.txt.gz"]
            output = ["maxbin2_abund/SPAdes"]
            shell = """
            mkdir -p .tmp/convert_depths_{sample} && cd .tmp/convert_depths_{sample}
            mkdir -p "$wd/maxbin2_abund/SPAdes"
            for f in SPAdes-{sample}-*.abund; do
                [ -e "$f" ] && mv "$f" "$wd/maxbin2_abund/SPAdes/"
            done
            cd "$wd" && rm -rf .tmp/convert_depths_{sample}
            """
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert!(
            warnings.iter().all(|w| w.code != "SCI-AGG-RACE"),
            "literal dir output with per-instance scratch split must not fire: {warnings:?}"
        );
    }

    #[test]
    fn agg_race_still_warns_for_file_output_fanned_per_sample() {
        // The suppression is scoped to literal dir outputs: a plain
        // wildcard-free FILE output still collides per instance and
        // must keep warning.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[values]]
            name = "assembler"
            values = ["spades", "megahit"]

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2"]

            [[rules]]
            name = "collect"
            input = ["done.marker"]
            expand_inputs = [{ pattern = "depths/{assembler}-{sample}.txt" }]
            output = ["abund/merged.txt"]
            shell = "cat {input} > abund/merged.txt"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(warnings[0].code, "SCI-AGG-RACE");
    }
}
