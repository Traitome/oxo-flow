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

fn featurecounts_has_multi_overlap(shell: &str) -> bool {
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
            segment.split_whitespace().any(|t| t == "-O")
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

/// Whether a declared output path still carries a fan-out wildcard.
///
/// `{config.x}` references are resolved to literal constants before any
/// fan-out happens — they are not wildcards. Counting them as such
/// exempted aggregation rules whose outputs live under
/// `{config.results_dir}` while every other path component is
/// wildcard-free (the exact tutorial race from issue #443): each
/// expanded instance resolved the same `results/multiqc/report.html`
/// path and the detector stayed silent. Only non-`config` placeholders
/// (`{sample}`, `{values.t}`, `{meta.col}`, fresh wildcards, …) multiply
/// the output path per fan-out element.
fn output_has_fanout_wildcard(output: &str) -> bool {
    PLACEHOLDER_RE
        .captures_iter(output)
        .any(|cap| !cap[1].starts_with("config."))
}

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
///   `when`-gated-off rules never run (issue #263);
/// - every active fan-out dimension must be *represented* in the declared
///   outputs (issue #829): outputs carrying one dimension's wildcard (say
///   `{pair_id}`) while another active dimension (say a `[[values]]` table
///   referenced from `expand_inputs`) is unkeyed still race — the unkeyed
///   dimension's instances all bake the same concrete path.
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
    // #530: register EVERY unbound wildcard of a pattern — expand.rs
    // fans out on all of them, but this registry took only the first, so a
    // consumer keyed on the second wildcard of `idx/{build}/{part}.bt2`
    // escaped the deferred-consumer gate and instantiated once per
    // discovered value writing to the same path (the exact SCI-AGG-RACE).
    let fresh_wildcards: Vec<String> = config
        .rules
        .iter()
        .filter_map(|r| r.output_pattern.as_deref())
        .flat_map(crate::wildcard::extract_wildcards)
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

        // Every ACTIVE fan-out dimension must be represented in the
        // declared outputs — a dimension's wildcard is what makes instance
        // outputs differ. Issue #829: the old check was all-or-nothing (any
        // non-config wildcard exempted the rule), so a rule whose outputs
        // carry the pair wildcard but not the [[values]] wildcard escaped —
        // venus's merge rules fan per {chr} via expand_inputs while all 24
        // instances per pair baked the same `vcf.raw/{pair_id}.vcf.gz` and
        // raced. `{config.x}` refs resolve to the same constant on every
        // instance and never key outputs; `{meta.col}` substitutes per
        // instance from pair/group metadata, so it counts as represented
        // for those domains (values entries carry no metadata — only the
        // table's own wildcard keys a values instance's outputs).
        let output_tokens: Vec<String> = rule
            .output
            .iter()
            .flat_map(|o| PLACEHOLDER_RE.captures_iter(o).map(|c| c[1].to_string()))
            .collect();
        let mut unrepresented: Vec<String> = Vec::new();
        if triggers_pair
            && !output_tokens
                .iter()
                .any(|t| t.starts_with("meta.") || pair_wildcards.contains(&t.as_str()))
        {
            unrepresented.push(
                "the pair domain — no pair wildcard ({pair_id}, …) keys the \
                               outputs"
                    .into(),
            );
        }
        if triggers_group
            && !output_tokens
                .iter()
                .any(|t| t.starts_with("meta.") || GROUP_WILDCARDS.contains(&t.as_str()))
        {
            unrepresented.push(
                "the sample domain — no {sample}/{group} wildcard keys the \
                               outputs"
                    .into(),
            );
        }
        for v in config.values.iter().filter(|v| {
            expand_texts.iter().any(|t| {
                t.contains(&format!("{{{}}}", v.name))
                    || t.contains(&format!("{{values.{}}}", v.name))
            })
        }) {
            let keyed = output_tokens
                .iter()
                .any(|t| t == &v.name || *t == format!("values.{}", v.name));
            if !keyed {
                unrepresented.push(format!(
                    "[[values]] table '{}' — none of its entries keys the outputs",
                    v.name
                ));
            }
        }
        for w in fresh_wildcards
            .iter()
            .filter(|w| expand_texts.iter().any(|t| t.contains(&format!("{{{w}}}"))))
        {
            if !output_tokens.iter().any(|t| t == w.as_str()) {
                unrepresented.push(format!(
                    "output_pattern producer wildcard '{w}' — it does not key the outputs"
                ));
            }
        }
        if unrepresented.is_empty() {
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
            .any(|o| crate::dag::looks_like_directory(o) && !output_has_fanout_wildcard(o));
        if has_dir_output {
            continue;
        }

        let dims = unrepresented.join("; ");
        warnings.push(ScientificWarning {
            code: "SCI-AGG-RACE".into(),
            rule: rule.name.clone(),
            message: format!(
                "rule fans out per {dims}, but its declared outputs are not keyed by that \
                 dimension: expansion creates one instance per element and every instance bakes \
                 the SAME concrete output paths — run concurrently they race \
                 (FileExistsError/missing-file crashes), sequentially they duplicate work N−1 \
                 times and the surviving result depends on scheduling order."
            ),
            suggestion: "if the fan-out is unintentional — an expand_inputs variable that merely \
                         reuses a [[values]] table name is the classic case (e.g. {chr} meaning \
                         'all chromosomes') — rename the variable (e.g. {chrom}) so the rule \
                         becomes a single aggregation instance; for a genuine per-element rule, \
                         key the outputs by that dimension's wildcard (e.g. \
                         out/{chr}/{pair_id}.vcf.gz) so every instance writes a distinct path, \
                         or drop the wildcard from a sample/pair aggregation and reference the \
                         per-instance files via expand_inputs (e.g. expand_inputs = [{pattern = \
                         \"qc/{sample}/fastqc.html\"}]) instead"
                .into(),
        });
    }
    warnings
}

/// Check variant calling rules (GATK, DeepVariant, Strelka, Manta, VEP).
fn check_variant_calling_rules(
    rule: &Rule,
    shell: &str,
    shell_lower: &str,
    sample_count: usize,
    warnings: &mut Vec<ScientificWarning>,
) {
    // SCI-VQSR-COHORT
    if shell_lower.contains("variantrecalibrator") && sample_count < MIN_VQSR_SAMPLES {
        warnings.push(ScientificWarning {
            code: "SCI-VQSR-COHORT".into(),
            rule: rule.name.clone(),
            message: format!(
                "VariantRecalibrator trains on the cohort, but only {} \
                 are in this run — GATK recommends a minimum of ~{MIN_VQSR_SAMPLES} \
                 (and considerably more, on the order of 100, for targeted/exome data). \
                 The pilot will fail at this step for scientific reasons, not technical ones.",
                crate::plural(sample_count, "sample", "samples")
            ),
            suggestion: "stop the pilot before VQSR (-t <rule>) or use hard filtering \
                         (VariantFiltration with QD/FS/MQ) for small cohorts"
                .into(),
        });
    }

    // SCI-BQSR-NO-KNOWN-SITES
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

    // Mutect2 checks
    if shell_lower.contains("mutect2") {
        let has_normal = has_normal_flag(shell_lower);
        if !has_normal {
            // SCI-MUTECT2-TUMOR-ONLY
            warnings.push(ScientificWarning {
                code: "SCI-MUTECT2-TUMOR-ONLY".into(),
                rule: rule.name.clone(),
                message: "Mutect2 runs without a matched normal. Tumor-only mode is supported, \
                          but GATK's documentation stresses that a matched normal helps not \
                          only with germline filtering — it also exposes technical artifacts \
                          (e.g. mapping errors at rare SNPs) that even a large panel of \
                          normals can miss. Without a matched normal, germline filtering \
                          leans entirely on population allele-frequency priors."
                    .into(),
                suggestion: "pair each tumor with a matched normal; in tumor-only mode also \
                             supply --germline-resource and --panel-of-normals"
                    .into(),
            });

            // SCI-MUTECT2-NO-PON: fires when tumor-only is explicitly configured
            let explicit_tumor_only = shell_lower.contains("tumor_only")
                || shell_lower.contains("--tumor-only")
                || shell_lower.contains("-tumor-only")
                || shell_lower.contains("--germline-resource");
            if explicit_tumor_only
                && !shell_lower.contains("--panel-of-normals")
                && !shell_lower.contains("-pon")
            {
                warnings.push(ScientificWarning {
                    code: "SCI-MUTECT2-NO-PON".into(),
                    rule: rule.name.clone(),
                    message: "Mutect2 in tumor-only mode is missing a Panel of Normals (PoN). \
                              Without a PoN, recurrent sequencing artifacts will be falsely called as somatic."
                        .into(),
                    suggestion: "supply a sequencing-matched Panel of Normals via --panel-of-normals pon.vcf.gz"
                        .into(),
                });
            }

            // SCI-MUTECT2-NO-GERMLINE: fires when tumor-only is explicitly configured
            let explicit_pon =
                shell_lower.contains("--panel-of-normals") || shell_lower.contains("-pon");
            if (explicit_tumor_only || explicit_pon) && !shell_lower.contains("--germline-resource")
            {
                warnings.push(ScientificWarning {
                    code: "SCI-MUTECT2-NO-GERMLINE".into(),
                    rule: rule.name.clone(),
                    message: "Mutect2 in tumor-only mode is missing --germline-resource. \
                              Without population allele frequencies, common germline variants cannot be filtered."
                        .into(),
                    suggestion: "supply a population allele-frequency resource (e.g. gnomAD via --germline-resource)"
                        .into(),
                });
            }
        }
    }

    // SCI-HAPLOTYPECALLER-GVCF-ERC
    if shell_lower.contains("haplotypecaller") {
        let produces_gvcf = rule.output.iter().any(|o| {
            o.ends_with(".g.vcf")
                || o.ends_with(".g.vcf.gz")
                || o.ends_with(".gvcf")
                || o.ends_with(".gvcf.gz")
        });
        let has_erc = shell_lower.contains("-erc") || shell_lower.contains("--emit-ref-confidence");
        if produces_gvcf && !has_erc {
            warnings.push(ScientificWarning {
                code: "SCI-HAPLOTYPECALLER-GVCF-ERC".into(),
                rule: rule.name.clone(),
                message: "HaplotypeCaller targets a gVCF output path but lacks -ERC GVCF / --emit-ref-confidence GVCF. \
                          Standard VCF output will be emitted without reference confidence blocks needed for joint genotyping."
                    .into(),
                suggestion: "add '-ERC GVCF' to HaplotypeCaller arguments to generate valid genomic VCFs for GenomicsDBImport / GenotypeGVCFs"
                    .into(),
            });
        }
    }

    // SCI-GENOTYPEGVCFS-NO-NEW-QUAL
    if shell_lower.contains("genotypegvcfs")
        && !shell_lower.contains("--new-qual")
        && !shell_lower.contains("-new-qual")
        && shell_lower.contains("gatk3")
    {
        warnings.push(ScientificWarning {
            code: "SCI-GENOTYPEGVCFS-NO-NEW-QUAL".into(),
            rule: rule.name.clone(),
            message: "GenotypeGVCFs is executed without explicit quality scoring model flag (--new-qual). \
                      Legacy Phred score calculation inflates QUAL values on large cohorts."
                .into(),
            suggestion: "add '--new-qual' to GenotypeGVCFs invocation to use the calibrated quality model"
                .into(),
        });
    }

    // SCI-DEEPVARIANT-MODEL-TYPE
    if (shell_lower.contains("run_deepvariant")
        || (shell_lower.contains("deepvariant") && shell_lower.contains("--reads")))
        && !shell_lower.contains("--model_type")
    {
        warnings.push(ScientificWarning {
            code: "SCI-DEEPVARIANT-MODEL-TYPE".into(),
            rule: rule.name.clone(),
            message: "DeepVariant invocation lacks the required --model_type argument. \
                      DeepVariant must know the sequencing modality to load appropriate convolutional weights."
                .into(),
            suggestion: "specify '--model_type=WGS', 'WES', 'PACBIO', or 'ONT' matching library preparation"
                .into(),
        });
    }

    // SCI-MANTA-NO-CONFIG
    if (shell_lower.contains("configmanta.py") || shell_lower.contains("configmanta"))
        && !shell_lower.contains("--referencefasta")
        && !shell_lower.contains("--reference")
    {
        warnings.push(ScientificWarning {
            code: "SCI-MANTA-NO-CONFIG".into(),
            rule: rule.name.clone(),
            message:
                "Manta structural variant configuration lacks reference genome FASTA specification."
                    .into(),
            suggestion: "provide reference genome using '--referenceFasta <ref.fa>'".into(),
        });
    }

    // SCI-STRELKA-NO-MANTA
    if (shell_lower.contains("configurestrelkasomaticworkflow.py")
        || shell_lower.contains("strelka_somatic"))
        && !shell_lower.contains("--indelcandidates")
    {
        warnings.push(ScientificWarning {
            code: "SCI-STRELKA-NO-MANTA".into(),
            rule: rule.name.clone(),
            message: "Strelka somatic workflow is executed without Manta candidate indels (--indelCandidates). \
                      Small somatic indel calling sensitivity will be significantly degraded."
                .into(),
            suggestion: "run Manta first and provide '--indelCandidates <manta_candidateSmallIndels.vcf.gz>'"
                .into(),
        });
    }

    // SCI-STRELKA-TARGETS-UNCOMPRESSED
    if shell_lower.contains("strelka")
        && (shell_lower.contains("--targetbed") || shell_lower.contains("--callregions"))
    {
        let has_uncompressed = shell.split_whitespace().any(|tok| {
            (tok.starts_with("--targetBed=") || tok.starts_with("--callRegions="))
                && tok.ends_with(".bed")
                && !tok.ends_with(".bed.gz")
        });
        if has_uncompressed {
            warnings.push(ScientificWarning {
                code: "SCI-STRELKA-TARGETS-UNCOMPRESSED".into(),
                rule: rule.name.clone(),
                message: "Strelka target region BED file is uncompressed; Strelka requires bgzip-compressed and tabix-indexed target regions."
                    .into(),
                suggestion: "compress target regions with 'bgzip' and index with 'tabix -p bed' before passing as .bed.gz"
                    .into(),
            });
        }
    }

    // SCI-VEP-OFFLINE-CACHE
    if (shell_lower.contains("vep ")
        || shell_lower.starts_with("vep")
        || shell_lower.contains("variant_effect_predictor"))
        && !shell_lower.contains("--offline")
        && !shell_lower.contains("--cache")
    {
        warnings.push(ScientificWarning {
            code: "SCI-VEP-OFFLINE-CACHE".into(),
            rule: rule.name.clone(),
            message: "Variant Effect Predictor (VEP) runs without local --cache or --offline flags. \
                      Querying online Ensembl REST endpoints causes extreme latency and breaks offline reproducibility."
                .into(),
            suggestion: "download local VEP cache files and pass '--offline --cache --dir_cache <path>'"
                .into(),
        });
    }
}

/// Check sequence alignment and coordinate sorting rules (BWA, Bowtie2, Minimap2, HISAT2, Samtools, Picard, Fastp).
fn check_alignment_rules(
    rule: &Rule,
    shell: &str,
    shell_lower: &str,
    warnings: &mut Vec<ScientificWarning>,
) {
    // SCI-BWA-MEM-NO-RG
    if (shell_lower.contains("bwa mem") || shell_lower.contains("bwa-mem2 mem"))
        && !shell
            .split_whitespace()
            .any(|tok| tok == "-R" || tok.starts_with("-R="))
    {
        warnings.push(ScientificWarning {
            code: "SCI-BWA-MEM-NO-RG".into(),
            rule: rule.name.clone(),
            message: "bwa mem is invoked without read group declaration (-R). \
                      Downstream GATK and Picard tools will fail when @RG headers and SM tags are absent."
                .into(),
            suggestion: "add read group specification, e.g. -R '@RG\\tID:{sample}\\tSM:{sample}\\tPL:ILLUMINA'"
                .into(),
        });
    }

    // SCI-BOWTIE2-NO-RG
    if shell_lower.contains("bowtie2 ") || shell_lower.starts_with("bowtie2") {
        let has_rg = shell_lower.contains("--rg-id") || shell_lower.contains("--rg ");
        if !has_rg && !shell_lower.contains("--version") && !shell_lower.contains("--help") {
            warnings.push(ScientificWarning {
                code: "SCI-BOWTIE2-NO-RG".into(),
                rule: rule.name.clone(),
                message: "bowtie2 alignment is missing read group specification (--rg-id / --rg). \
                          Alignments without @RG metadata cannot be processed by variant calling pipelines."
                    .into(),
                suggestion: "provide '--rg-id {sample} --rg SM:{sample} --rg PL:ILLUMINA'"
                    .into(),
            });
        }
    }

    // SCI-MINIMAP2-PRESET
    if (shell_lower.contains("minimap2 ") || shell_lower.starts_with("minimap2"))
        && !shell_lower.contains("-x")
        && !shell_lower.contains("--preset")
        && !shell_lower.contains("--version")
        && !shell_lower.contains("--help")
    {
        warnings.push(ScientificWarning {
            code: "SCI-MINIMAP2-PRESET".into(),
            rule: rule.name.clone(),
            message: "minimap2 is invoked without an explicit preset flag (-x). \
                      Default heuristic parameters may severely degrade mapping quality on PacBio, ONT, or short reads."
                .into(),
            suggestion: "specify an explicit technology preset: -x map-ont, -x map-pb, -x map-hifi, or -x sr"
                .into(),
        });
    }

    // SCI-SAMTOOLS-SORT-MEM-OVERFLOW
    if shell_lower.contains("samtools sort") {
        let tokens: Vec<&str> = shell.split_whitespace().collect();
        let mut threads: u64 = rule.resources.threads as u64;
        let mut mem_per_thread_mb: Option<u64> = None;

        for (i, tok) in tokens.iter().enumerate() {
            if *tok == "-@" || *tok == "--threads" {
                if let Some(next) = tokens.get(i + 1)
                    && let Ok(t) = next.parse::<u64>()
                {
                    threads = t;
                }
            } else if tok.starts_with("-@") && tok.len() > 2 {
                if let Ok(t) = tok[2..].parse::<u64>() {
                    threads = t;
                }
            } else if *tok == "-m" {
                if let Some(next) = tokens.get(i + 1) {
                    mem_per_thread_mb = crate::scheduler::parse_memory_mb(next);
                }
            } else if tok.starts_with("-m") && tok.len() > 2 {
                mem_per_thread_mb = crate::scheduler::parse_memory_mb(&tok[2..]);
            }
        }

        if let Some(m_mb) = mem_per_thread_mb
            && let Some(rule_mem) = rule.resources.memory.as_deref()
            && let Some(rule_mem_mb) = crate::scheduler::parse_memory_mb(rule_mem)
        {
            let total_sort_mem = threads.saturating_mul(m_mb);
            if total_sort_mem > rule_mem_mb {
                warnings.push(ScientificWarning {
                    code: "SCI-SORT-THREADS-MEM-PRODUCT".into(),
                    rule: rule.name.clone(),
                    message: format!(
                        "samtools sort memory specification (-m {m_mb} MB * {threads} threads = {total_sort_mem} MB) \
                         exceeds rule allocated memory ({rule_mem_mb} MB). This causes kernel OOM kills."
                    ),
                    suggestion: format!(
                        "reduce per-thread memory: set -m {}M or increase rule allocated memory",
                        rule_mem_mb.saturating_div(threads.max(1))
                    ),
                });
            }
        }
    }

    // SCI-SAMTOOLS-INDEX-UNSORTED
    if shell_lower.contains("samtools index") {
        let has_sam_input = rule.input.iter().any(|i| i.ends_with(".sam"));
        if has_sam_input {
            warnings.push(ScientificWarning {
                code: "SCI-SAMTOOLS-INDEX-UNSORTED".into(),
                rule: rule.name.clone(),
                message: "samtools index invoked on a SAM file; samtools index strictly requires coordinate-sorted BAM or CRAM."
                    .into(),
                suggestion: "convert and coordinate-sort with 'samtools sort' before indexing"
                    .into(),
            });
        }
    }

    // SCI-MARKDUPLICATES-REMOVE-DUPS
    if shell_lower.contains("markduplicates")
        && (shell_lower.contains("remove_duplicates=true")
            || shell_lower.contains("remove_duplicates true"))
    {
        warnings.push(ScientificWarning {
            code: "SCI-MARKDUPLICATES-REMOVE-DUPS".into(),
            rule: rule.name.clone(),
            message: "MarkDuplicates is configured with REMOVE_DUPLICATES=true. \
                      Discarding duplicated alignments permanently removes read evidence needed for structural variant and CNV calling."
                .into(),
            suggestion: "keep REMOVE_DUPLICATES=false to flag duplicates in the SAM bitwise flag rather than dropping them"
                .into(),
        });
    }

    // SCI-HISAT2-NO-SPLICE-SITES
    if (shell_lower.contains("hisat2 ") || shell_lower.starts_with("hisat2"))
        && !shell_lower.contains("--known-splicesite-infile")
        && !shell_lower.contains("--version")
        && !shell_lower.contains("--help")
    {
        warnings.push(ScientificWarning {
            code: "SCI-HISAT2-NO-SPLICE-SITES".into(),
            rule: rule.name.clone(),
            message: "hisat2 RNA-seq alignment executed without --known-splicesite-infile. \
                      Splice junction discovery and mapping accuracy across annotated exons will be reduced."
                .into(),
            suggestion: "extract splice sites from annotation (hisat2_extract_splice_sites.py annotation.gtf) and pass '--known-splicesite-infile'"
                .into(),
        });
    }

    // SCI-FASTP-DEDUPLICATION
    if (shell_lower.contains("fastp ") || shell_lower.starts_with("fastp"))
        && (shell_lower.contains("--dedup") || shell.split_whitespace().any(|t| t == "-D"))
        && !shell_lower.contains("--umi")
    {
        warnings.push(ScientificWarning {
            code: "SCI-FASTP-DEDUPLICATION".into(),
            rule: rule.name.clone(),
            message: "fastp deduplication (--dedup) enabled without UMI tagging on potential RNA-seq data. \
                      Sequence-based deduplication without molecular barcodes eliminates biological dynamic range for highly expressed genes."
                .into(),
            suggestion: "disable fastp --dedup for bulk RNA-seq, or specify '--umi' if unique molecular identifiers were used"
                .into(),
        });
    }
}

/// Check RNA-seq and Single-Cell processing rules (featureCounts, STAR, Salmon, Kallisto, RSEM, StringTie, DESeq2, CellRanger, Seurat).
fn check_rnaseq_rules(
    rule: &Rule,
    shell: &str,
    shell_lower: &str,
    config: &WorkflowConfig,
    sample_count: usize,
    warnings: &mut Vec<ScientificWarning>,
) {
    // SCI-FEATURECOUNTS-STRAND
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

    // SCI-FEATURECOUNTS-MULTI-OVERLAP
    if shell_lower.contains("featurecounts") && featurecounts_has_multi_overlap(shell) {
        warnings.push(ScientificWarning {
            code: "SCI-FEATURECOUNTS-MULTI-OVERLAP".into(),
            rule: rule.name.clone(),
            message: "featureCounts runs with -O (assign reads to all overlapping meta-features). \
                      This causes double-counting for genes with overlapping genomic coordinates on opposite strands."
                .into(),
            suggestion: "use strand-specific counting (-s 1/-s 2) or omit -O for standard gene quantification"
                .into(),
        });
    }

    // SCI-STAR-SJDB-OVERHANG
    if shell_lower.contains("star ")
        && shell_lower.contains("genomegenerate")
        && shell_lower.contains("--sjdbgtffile")
        && !shell_lower.contains("--sjdboverhang")
    {
        warnings.push(ScientificWarning {
            code: "SCI-STAR-SJDB-OVERHANG".into(),
            rule: rule.name.clone(),
            message: "STAR index generation includes annotation (--sjdbGTFfile) but omits --sjdbOverhang. \
                      The default value 100 may degrade junction mapping for 150bp or 75bp sequencing reads."
                .into(),
            suggestion: "set '--sjdbOverhang <read_length - 1>' (e.g. 149 for 150bp reads) matching sequencing library length"
                .into(),
        });
    }

    // SCI-STAR-GENOME-LOAD
    if shell_lower.contains("star ") && shell_lower.contains("--genomeload loadandkeep") {
        warnings.push(ScientificWarning {
            code: "SCI-STAR-GENOME-LOAD".into(),
            rule: rule.name.clone(),
            message: "STAR uses --genomeLoad LoadAndKeep. In batch and cluster environments, \
                      retaining the shared genome segment in RAM can lead to memory leakage and node eviction."
                .into(),
            suggestion: "use '--genomeLoad NoSharedMemory' for deterministic cluster and container execution"
                .into(),
        });
    }

    // SCI-SALMON-LIBTYPE
    if shell_lower.contains("salmon quant")
        && (shell_lower.contains("-l a") || shell_lower.contains("-l auto"))
    {
        warnings.push(ScientificWarning {
            code: "SCI-SALMON-LIBTYPE".into(),
            rule: rule.name.clone(),
            message: "salmon quant uses automatic library type detection (-l A). \
                      Automatic inference on shallow-coverage samples can select inconsistent strandedness models."
                .into(),
            suggestion: "specify explicit library type: -l ISR (dUTP reverse-stranded), -l ISF, or -l IU"
                .into(),
        });
    }

    // SCI-SALMON-NO-GC-BIAS
    if shell_lower.contains("salmon quant") && !shell_lower.contains("--gcbias") {
        warnings.push(ScientificWarning {
            code: "SCI-SALMON-NO-GC-BIAS".into(),
            rule: rule.name.clone(),
            message: "salmon quant is executed without --gcBias correction. \
                      Illumina RNA-seq exhibits systemic sequence-specific GC biases that skew transcript quantification."
                .into(),
            suggestion: "add '--gcBias' to correct for non-uniform sequence coverage"
                .into(),
        });
    }

    // SCI-KALLISTO-SINGLE-END-FRAG
    if shell_lower.contains("kallisto quant")
        && shell_lower.contains("--single")
        && (!shell_lower.contains("-l ") && !shell_lower.contains("--fragment-length"))
    {
        warnings.push(ScientificWarning {
            code: "SCI-KALLISTO-SINGLE-END-FRAG".into(),
            rule: rule.name.clone(),
            message: "kallisto quant in single-end mode requires mean fragment length (-l / --fragment-length). \
                      kallisto will abort execution without this parameter."
                .into(),
            suggestion: "supply '-l <mean_length> -s <std_dev>' based on Bioanalyzer electropherogram"
                .into(),
        });
    }

    // SCI-RSEM-NO-STRAND
    if shell_lower.contains("rsem-calculate-expression")
        && !shell_lower.contains("--strand-specific")
    {
        warnings.push(ScientificWarning {
            code: "SCI-RSEM-NO-STRAND".into(),
            rule: rule.name.clone(),
            message: "rsem-calculate-expression lacks --strand-specific configuration. \
                      Strand-specific libraries will be estimated using unstranded expectations."
                .into(),
            suggestion: "add '--strand-specific' or '--forward-prob 0.0' (reverse stranded dUTP)"
                .into(),
        });
    }

    // SCI-STRINGTIE-NO-GUIDE
    if (shell_lower.contains("stringtie ") || shell_lower.starts_with("stringtie"))
        && !shell_lower.contains("-g")
        && !shell_lower.contains("--version")
        && !shell_lower.contains("--help")
    {
        warnings.push(ScientificWarning {
            code: "SCI-STRINGTIE-NO-GUIDE".into(),
            rule: rule.name.clone(),
            message: "stringtie transcript assembly is running without a reference annotation guide (-G). \
                      De novo assembly without reference guidance results in fragmented gene models."
                .into(),
            suggestion: "supply reference gene annotation via '-G <reference.gtf>'"
                .into(),
        });
    }

    // SCI-DESEQ2-MIN-REPLICATES
    if (shell_lower.contains("deseq2") || shell_lower.contains("edger"))
        && sample_count > 0
        && sample_count < 3
    {
        warnings.push(ScientificWarning {
            code: "SCI-DESEQ2-MIN-REPLICATES".into(),
            rule: rule.name.clone(),
            message: format!(
                "Differential expression analysis configured with only {sample_count} sample(s). \
                 DESeq2 and edgeR require at least 3 biological replicates per group to estimate dispersion."
            ),
            suggestion: "ensure at least 3 biological replicates per condition before differential expression testing"
                .into(),
        });
    }

    // SCI-CELLRANGER-INTRONS
    if shell_lower.contains("cellranger count")
        && (shell_lower.contains("snrna")
            || shell_lower.contains("single-nucleus")
            || shell_lower.contains("nuclei"))
        && !shell_lower.contains("--include-introns")
    {
        warnings.push(ScientificWarning {
            code: "SCI-CELLRANGER-INTRONS".into(),
            rule: rule.name.clone(),
            message: "cellranger count on single-nucleus RNA-seq (snRNA-seq) lacks --include-introns. \
                      Nuclear pre-mRNA accounts for up to 50% of reads; omitting introns causes high dropout."
                .into(),
            suggestion: "add '--include-introns=true' to quantify intronic nuclear reads"
                .into(),
        });
    }

    // SCI-SEURAT-NO-NORMALIZATION
    if (shell_lower.contains("findclusters")
        || shell_lower.contains("runpca")
        || shell_lower.contains("runumap"))
        && !shell_lower.contains("normalizedata")
        && !shell_lower.contains("sctransform")
        && !shell_lower.contains("normalize")
    {
        warnings.push(ScientificWarning {
            code: "SCI-SEURAT-NO-NORMALIZATION".into(),
            rule: rule.name.clone(),
            message: "Single-cell clustering/dimensionality reduction executed without apparent prior normalization. \
                      Cell-to-cell differences in sequencing depth will dominate clustering over biological variation."
                .into(),
            suggestion: "run NormalizeData() or SCTransform() before RunPCA() and FindClusters()"
                .into(),
        });
    }
}

/// Check epigenomics, ChIP-seq, and ATAC-seq rules (MACS2, Bismark, Bowtie, Bedtools).
fn check_epigenomics_rules(
    rule: &Rule,
    _shell: &str,
    shell_lower: &str,
    warnings: &mut Vec<ScientificWarning>,
) {
    if shell_lower.contains("macs2 callpeak") {
        // SCI-MACS2-GENOME-SIZE
        if !shell_lower.contains("-g ") && !shell_lower.contains("--gsize") {
            warnings.push(ScientificWarning {
                code: "SCI-MACS2-GENOME-SIZE".into(),
                rule: rule.name.clone(),
                message: "macs2 callpeak is missing explicit effective genome size (-g / --gsize). \
                          Defaulting to human ('hs') will invalidate FDR and peak significance on other model organisms."
                    .into(),
                suggestion: "supply -g hs (human), -g mm (mouse), -g dm (fly), or exact mappable size"
                    .into(),
            });
        }

        // SCI-MACS2-BROAD-PEAKS
        let mentions_broad_mark = shell_lower.contains("h3k27me3")
            || shell_lower.contains("h3k9me3")
            || shell_lower.contains("h3k36me3")
            || shell_lower.contains("broad");
        if mentions_broad_mark && !shell_lower.contains("--broad") {
            warnings.push(ScientificWarning {
                code: "SCI-MACS2-BROAD-PEAKS".into(),
                rule: rule.name.clone(),
                message: "macs2 callpeak is targeting broad histone marks without the --broad flag. \
                          Diffuse chromatin enrichment will be fragmented into spurious narrow peaks."
                    .into(),
                suggestion: "add '--broad --broad-cutoff 0.1' for broad domain histone modifications"
                    .into(),
            });
        }

        // SCI-MACS2-PAIRED-END-FORMAT
        let mentions_pe = rule
            .input
            .iter()
            .any(|i| i.contains("pe") || i.contains("paired"))
            || shell_lower.contains("paired");
        if mentions_pe && shell_lower.contains("-f bam") && !shell_lower.contains("-f bampe") {
            warnings.push(ScientificWarning {
                code: "SCI-MACS2-PAIRED-END-FORMAT".into(),
                rule: rule.name.clone(),
                message:
                    "macs2 callpeak specifies single-end format '-f BAM' for paired-end library. \
                          MACS2 will discard paired mates and fragment insert length estimates."
                        .into(),
                suggestion: "use '-f BAMPE' to model exact paired fragment insertions".into(),
            });
        }

        // SCI-CHIP-NO-CONTROL
        if !shell_lower.contains("-c ")
            && !shell_lower.contains("--control")
            && !shell_lower.contains("atac")
        {
            warnings.push(ScientificWarning {
                code: "SCI-CHIP-NO-CONTROL".into(),
                rule: rule.name.clone(),
                message: "macs2 callpeak is running without a matched control / input BAM (-c / --control). \
                          ChIP-seq peak calling without control suffers high false-positive rates due to local chromatin copy number and GC bias."
                    .into(),
                suggestion: "provide a matched input control via '-c control.bam'"
                    .into(),
            });
        }
    }

    // SCI-ATAC-SHIFT-COORDINATES
    if (shell_lower.contains("atac")
        && (shell_lower.contains("callpeak") || shell_lower.contains("genrich")))
        && !shell_lower.contains("shift")
        && !shell_lower.contains("alignmentsieve")
    {
        warnings.push(ScientificWarning {
            code: "SCI-ATAC-SHIFT-COORDINATES".into(),
            rule: rule.name.clone(),
            message: "ATAC-seq peak calling performed without Tn5 transposase coordinate shifting. \
                      Cut sites must be shifted (+4 bp on +, -5 bp on - strand) to center on open chromatin events."
                .into(),
            suggestion: "use 'alignmentSieve --ATACshift' or deeptools to adjust read coordinates before peak calling"
                .into(),
        });
    }

    // SCI-BISMARK-NON-DIRECTIONAL
    if (shell_lower.contains("bismark ") || shell_lower.starts_with("bismark"))
        && (shell_lower.contains("pbat") || shell_lower.contains("swift"))
        && !shell_lower.contains("--non_directional")
        && !shell_lower.contains("--pbat")
    {
        warnings.push(ScientificWarning {
            code: "SCI-BISMARK-NON-DIRECTIONAL".into(),
            rule: rule.name.clone(),
            message: "Bismark alignment on PBAT / non-directional bisulfite library lacks --non_directional. \
                      Reads from the other strands will fail to align, losing over 50% of sequencing data."
                .into(),
            suggestion: "add '--non_directional' or '--pbat' to Bismark command line"
                .into(),
        });
    }

    // SCI-BOWTIE-BISULFITE-MISMATCH
    if shell_lower.contains("bismark")
        && shell_lower.contains("bowtie2")
        && shell_lower.contains("--score-min l,0,0")
    {
        warnings.push(ScientificWarning {
            code: "SCI-BOWTIE-BISULFITE-MISMATCH".into(),
            rule: rule.name.clone(),
            message:
                "Bisulfite alignment uses excessively strict mismatch score (--score-min L,0,0). \
                      Unconverted reads or biological SNPs will fail mapping."
                    .into(),
            suggestion:
                "use recommended Bismark default score-min (L,0,-0.2) for bisulfite sequencing"
                    .into(),
        });
    }

    // SCI-BEDTOOLS-GENOME-FILE
    if (shell_lower.contains("bedtools slop") || shell_lower.contains("bedtools flank"))
        && !shell_lower.contains("-g ")
    {
        warnings.push(ScientificWarning {
            code: "SCI-BEDTOOLS-GENOME-FILE".into(),
            rule: rule.name.clone(),
            message: "bedtools slop/flank is executed without chromosome length definitions (-g). \
                      Features extended beyond chromosome ends will create invalid coordinates."
                .into(),
            suggestion: "specify chromosome sizes file using '-g <chrom.sizes>'".into(),
        });
    }
}

/// Check metagenomics and microbiome rules (Kraken2, Bracken, QIIME2, HUMAnN, MetaPhlAn, CheckM).
fn check_metagenomics_rules(
    rule: &Rule,
    _shell: &str,
    shell_lower: &str,
    warnings: &mut Vec<ScientificWarning>,
) {
    // SCI-KRAKEN2-CONFIDENCE
    if (shell_lower.contains("kraken2 ") || shell_lower.starts_with("kraken2"))
        && (!shell_lower.contains("--confidence")
            || shell_lower.contains("--confidence 0.0")
            || shell_lower.contains("--confidence 0 "))
    {
        warnings.push(ScientificWarning {
            code: "SCI-KRAKEN2-CONFIDENCE".into(),
            rule: rule.name.clone(),
            message: "kraken2 runs with default confidence threshold of 0.0. \
                      Shared k-mers between related species cause substantial false-positive genus and species assignments."
                .into(),
            suggestion: "set '--confidence 0.1' (or 0.2) to filter low-confidence taxonomic assignments"
                .into(),
        });
    }

    // SCI-BRACKEN-READ-LEN
    if (shell_lower.contains("bracken ") || shell_lower.starts_with("bracken"))
        && !shell_lower.contains("-r ")
    {
        warnings.push(ScientificWarning {
            code: "SCI-BRACKEN-READ-LEN".into(),
            rule: rule.name.clone(),
            message: "bracken abundance estimation lacks read length parameter (-r). \
                      Bracken relies on precomputed database distributions for specific read lengths (e.g. 100, 150)."
                .into(),
            suggestion: "supply '-r <read_length>' (e.g. -r 150) matching sequencing read length"
                .into(),
        });
    }

    // SCI-QIIME2-DENOISE-TRIM
    if shell_lower.contains("dada2 denoise")
        && (shell_lower.contains("--p-trunc-len 0") || shell_lower.contains("--p-trunc-len-f 0"))
    {
        warnings.push(ScientificWarning {
            code: "SCI-QIIME2-DENOISE-TRIM".into(),
            rule: rule.name.clone(),
            message: "QIIME2 DADA2 denoising is configured without 3' quality truncation (--p-trunc-len 0). \
                      Degraded quality tails at read ends will dramatically increase error-rate estimates and inflate spurious ASVs."
                .into(),
            suggestion: "inspect FastQC quality profiles and set --p-trunc-len where median quality drops below Q30"
                .into(),
        });
    }

    // SCI-HUMANN-NUCLEOTIDE-DB
    if (shell_lower.contains("humann ") || shell_lower.starts_with("humann"))
        && !shell_lower.contains("--nucleotide-database")
    {
        warnings.push(ScientificWarning {
            code: "SCI-HUMANN-NUCLEOTIDE-DB".into(),
            rule: rule.name.clone(),
            message: "HUMAnN functional profiling is running without explicit --nucleotide-database. \
                      HUMAnN will skip ChocoPhlAn pangenome alignment and fall back directly to translated diamond search."
                .into(),
            suggestion: "provide '--nucleotide-database <chocophlan_dir>'"
                .into(),
        });
    }

    // SCI-METAPHLAN-UNKNOWN-ESTIMATION
    if (shell_lower.contains("metaphlan ") || shell_lower.starts_with("metaphlan"))
        && shell_lower.contains("--unknown_estimation")
        && !shell_lower.contains("--bowtie2db")
    {
        warnings.push(ScientificWarning {
            code: "SCI-METAPHLAN-UNKNOWN-ESTIMATION".into(),
            rule: rule.name.clone(),
            message: "MetaPhlAn --unknown_estimation requires explicit marker database index path (--bowtie2db)."
                .into(),
            suggestion: "specify '--bowtie2db <marker_database_dir>'"
                .into(),
        });
    }

    // SCI-CHECKM-NO-LINEAGE
    if shell_lower.contains("checkm lineage_wf") && !shell_lower.contains("-x ") {
        warnings.push(ScientificWarning {
            code: "SCI-CHECKM-NO-LINEAGE".into(),
            rule: rule.name.clone(),
            message: "checkm lineage_wf is missing genome fasta extension flag (-x). \
                      CheckM will fail to detect FASTA bins in the input folder."
                .into(),
            suggestion: "specify '-x fa' or '-x fasta' to match MAG bin file extensions".into(),
        });
    }
}

/// Check workflow concurrency, hardware resources, and dataflow sanity (pipefail, tmpdir, sort product, ports).
fn check_concurrency_hardware_rules(
    rule: &Rule,
    shell: &str,
    shell_lower: &str,
    config: &WorkflowConfig,
    warnings: &mut Vec<ScientificWarning>,
) {
    // SCI-TMPDIR-NOT-SCRATCH
    let heavy_tool = shell_lower.contains("samtools")
        || shell_lower.contains("gatk")
        || shell_lower.contains("star ")
        || shell_lower.contains("bwa");
    let hardcoded_tmp = shell.contains("/tmp/")
        || shell.contains("tmpdir=/tmp")
        || shell.contains("java.io.tmpdir=/tmp");
    if heavy_tool && hardcoded_tmp {
        warnings.push(ScientificWarning {
            code: "SCI-TMPDIR-NOT-SCRATCH".into(),
            rule: rule.name.clone(),
            message: "Heavy bioinformatic tool writes temporary files to hardcoded '/tmp' instead of engine scratch space. \
                      Spilling large intermediate files into root /tmp can exhaust OS root volume."
                .into(),
            suggestion: "use '${TMPDIR}' or configure [engine] tmp_dir to direct scratch files to high-speed NVMe/tmpfs"
                .into(),
        });
    }

    // SCI-PIPE-FAIL-DISABLED
    let pipe_count = shell.matches('|').count();
    let or_count = shell.matches("||").count();
    let actual_pipes = pipe_count.saturating_sub(or_count * 2);
    let has_pipefail = shell.contains("pipefail")
        || config
            .defaults
            .shell_prelude
            .as_deref()
            .is_some_and(|p| p.contains("pipefail"));
    if actual_pipes >= 2 && !has_pipefail {
        warnings.push(ScientificWarning {
            code: "SCI-PIPE-FAIL-DISABLED".into(),
            rule: rule.name.clone(),
            message: "Shell pipeline contains pipe commands ('|') without 'set -o pipefail'. \
                      Failures in upstream commands (e.g. samtools view) will be masked if the downstream tool exits 0."
                .into(),
            suggestion: "prepend 'set -eo pipefail;' or set defaults.shell_prelude = 'set -eo pipefail;'"
                .into(),
        });
    }

    // SCI-ZERO-BYTE-COMPLETION
    let outputs_binary = rule.output.iter().any(|o| {
        o.ends_with(".bam") || o.ends_with(".cram") || o.ends_with(".vcf.gz") || o.ends_with(".bcf")
    });
    let masks_failure_with_touch = shell.contains("|| touch")
        || shell.contains("|| : >")
        || shell.contains("|| true")
        || (outputs_binary
            && (shell.trim().starts_with("touch ") || shell.trim().starts_with(": >")));
    if outputs_binary && masks_failure_with_touch {
        warnings.push(ScientificWarning {
            code: "SCI-ZERO-BYTE-COMPLETION".into(),
            rule: rule.name.clone(),
            message: "Rule masks failure or creates empty zero-byte binary biological artifacts (.bam / .vcf.gz). \
                      Downstream tools expecting indexed binary headers will fail with corrupted stream errors."
                .into(),
            suggestion: "remove '|| touch' or dummy file creation; allow the rule to fail so the scheduler can trigger retry or diagnosis"
                .into(),
        });
    }

    // SCI-MULTI-TENANT-PORT-COLLISION
    if (shell_lower.contains("jupyter")
        || shell_lower.contains("tensorboard")
        || shell_lower.contains("rstudio"))
        && (shell_lower.contains("--port 8888")
            || shell_lower.contains("--port 6006")
            || shell_lower.contains("--port 8787"))
    {
        warnings.push(ScientificWarning {
            code: "SCI-MULTI-TENANT-PORT-COLLISION".into(),
            rule: rule.name.clone(),
            message: "Interactive server rule uses a hardcoded port. In multi-tenant HPC or cloud environments, concurrent runs will fail with port conflicts."
                .into(),
            suggestion: "bind to an ephemeral or environment-assigned port, e.g. '--port ${PORT:-0}'"
                .into(),
        });
    }
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

        check_variant_calling_rules(rule, shell, &shell_lower, sample_count, &mut warnings);
        check_alignment_rules(rule, shell, &shell_lower, &mut warnings);
        check_rnaseq_rules(
            rule,
            shell,
            &shell_lower,
            config,
            sample_count,
            &mut warnings,
        );
        check_epigenomics_rules(rule, shell, &shell_lower, &mut warnings);
        check_metagenomics_rules(rule, shell, &shell_lower, &mut warnings);
        check_concurrency_hardware_rules(rule, shell, &shell_lower, config, &mut warnings);
    }

    // Biological type contract check (TYPE-001 .. TYPE-006)
    if (!config.types.is_empty() || config.rules.iter().any(|r| r.types.is_some()))
        && let Ok(dag) = crate::dag::WorkflowDag::from_rules(&config.rules)
    {
        let type_diags = crate::types::check_workflow_types(config, &dag);
        for td in type_diags {
            warnings.push(ScientificWarning {
                code: td.violation.code,
                rule: td.consumer_rule,
                message: format!(
                    "biological type mismatch from upstream rule '{}' ({} -> {}): {}",
                    td.producer_rule,
                    td.producer_channel,
                    td.consumer_channel,
                    td.violation.message
                ),
                suggestion: td.violation.suggestion.unwrap_or_else(|| {
                    "align biological data format contracts between producer and consumer".into()
                }),
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
        assert!(warnings[0].message.contains("2 samples"));
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
        assert!(warnings[0].message.contains("2 samples"));
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
    fn agg_race_config_var_in_output_does_not_mask_the_race() {
        // Live repro from the first-workflow tutorial: outputs live under
        // `{config.results_dir}` — a resolved constant, not a fan-out
        // wildcard. The old check treated any `{...}` placeholder as an
        // output wildcard and exempted the rule, so both expanded
        // instances wrote the same results/multiqc/report.html silently.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [config]
            samples_dir = "raw_data"
            results_dir = "results"

            [[sample_groups]]
            name = "auto"
            samples = ["sample1", "sample2"]

            [[rules]]
            name = "fastqc_raw"
            input = ["{config.samples_dir}/{sample}_R1.fastq.gz"]
            output = ["{config.results_dir}/fastqc/{sample}_R1_fastqc.html"]
            shell = "echo html > {output}"

            [[rules]]
            name = "multiqc_per_sample"
            input = ["{config.results_dir}/fastqc/{sample}_R1_fastqc.html"]
            output = ["{config.results_dir}/multiqc/report.html"]
            shell = "cat {input} >> {output}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(warnings[0].code, "SCI-AGG-RACE");
        assert_eq!(warnings[0].rule, "multiqc_per_sample");
    }

    #[test]
    fn agg_race_config_keyed_output_still_silent() {
        // A genuinely keyed output under a config dir — the fan-out
        // wildcard is present, so the normal per-instance shape holds and
        // the rule stays silent.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [config]
            results_dir = "results"

            [[sample_groups]]
            name = "auto"
            samples = ["sample1", "sample2"]

            [[rules]]
            name = "fastqc_raw"
            input = ["{sample}_R1.fastq.gz"]
            output = ["{config.results_dir}/fastqc/{sample}.html"]
            shell = "echo html > {output}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert!(warnings.iter().all(|w| w.code != "SCI-AGG-RACE"));
    }

    #[test]
    fn fresh_wildcard_registry_covers_every_pattern_wildcard() {
        // #530: the fresh-wildcard registry took only the FIRST wildcard of
        // each output_pattern — a deferred consumer keyed on the second
        // wildcard escaped the gate and instantiated once per discovered
        // value, all writes landing on the same done-file (SCI-AGG-RACE).
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[values]]
            name = "build"
            values = ["37", "38"]

            [[rules]]
            name = "indexer"
            output_pattern = "idx/{build}/{part}.bt2"
            shell = "touch {output_pattern}"

            [[rules]]
            name = "consumer"
            input = ["idx/37/{part}.bt2"]
            output = ["done.txt"]
            shell = "cat {input} > {output}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert!(
            warnings
                .iter()
                .any(|w| w.code == "SCI-AGG-RACE" && w.rule == "consumer"),
            "consumer keyed on the second wildcard must be gated: {warnings:?}"
        );
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
    fn agg_race_values_dimension_unkeyed_by_outputs_warns() {
        // Issue #829 (venus gather_chr_vcfs): expand_inputs fans the rule
        // per {chr}, but the outputs are keyed only by {pair_id}. The old
        // all-or-nothing output check saw the pair wildcard and exempted
        // the rule — yet all 24 values instances per pair bake the SAME
        // concrete path and race. The per-dimension check must catch it
        // and name the table.
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

            [[values]]
            name = "chr"
            values = ["chr1", "chr2", "chr3"]

            [[rules]]
            name = "merge_chr_vcfs"
            input = ["done.marker"]
            expand_inputs = [{ pattern = "vcf.call/{chr}/{pair_id}.vcf.gz" }]
            output = ["vcf.raw/{pair_id}.vcf.gz"]
            shell = "cat {input} > {output}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(warnings[0].code, "SCI-AGG-RACE");
        assert_eq!(warnings[0].rule, "merge_chr_vcfs");
        assert!(
            warnings[0].message.contains("'chr'"),
            "message must name the unrepresented values table: {warnings:?}"
        );
    }

    #[test]
    fn agg_race_silent_when_every_active_dimension_keyed() {
        // The intentional per-chromosome scatter (venus Mutect2): the
        // values wildcard keys the outputs alongside the pair wildcard, so
        // every instance writes a distinct path — must stay silent.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [[pairs]]
            pair_id = "P1"
            experiment = "T1"
            control = "N1"

            [[values]]
            name = "chr"
            values = ["chr1", "chr2"]

            [[rules]]
            name = "mutect2_chr"
            input = ["bam/{pair_id}.bam"]
            output = ["vcf.call/{chr}/{pair_id}.vcf.gz"]
            shell = "gatk Mutect2 -R ref.fa -I {input} -O {output}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        assert!(
            warnings.iter().all(|w| w.code != "SCI-AGG-RACE"),
            "fully keyed fan-out must not fire: {warnings:?}"
        );
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

    #[test]
    fn biological_type_checking_surfaces_in_scientific_preflight() {
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0"

            [types.raw_sam]
            format = "sam"
            sort_order = "unsorted"

            [types.sorted_bam]
            format = "bam"
            sort_order = "coordinate"
            index = "bai"

            [[rules]]
            name = "align"
            output = ["out.sam"]
            shell = "bwa mem ref.fa reads.fq > out.sam"
            [rules.types]
            outputs = { default = "raw_sam" }

            [[rules]]
            name = "call_variants"
            input = ["out.sam"]
            output = ["calls.vcf"]
            shell = "gatk HaplotypeCaller -I out.sam -O calls.vcf"
            [rules.types]
            inputs = { default = "sorted_bam" }
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        let codes: Vec<&str> = warnings.iter().map(|w| w.code.as_str()).collect();
        assert!(
            codes.contains(&"TYPE-001"),
            "must detect SAM -> BAM format mismatch: {warnings:?}"
        );
        assert!(
            codes.contains(&"TYPE-002"),
            "must detect sort order mismatch: {warnings:?}"
        );
        assert!(
            codes.contains(&"TYPE-003"),
            "must detect missing index: {warnings:?}"
        );
    }

    #[test]
    fn test_variant_calling_extended_checks() {
        let toml = r#"
            [workflow]
            name = "vc_checks"

            [[rules]]
            name = "m2_tumor"
            output = ["tumor.vcf.gz"]
            shell = "gatk Mutect2 -R ref.fa -I tumor.bam --tumor-only -O tumor.vcf.gz"

            [[rules]]
            name = "hc_gvcf"
            output = ["sample.g.vcf.gz"]
            shell = "gatk HaplotypeCaller -R ref.fa -I sample.bam -O sample.g.vcf.gz"

            [[rules]]
            name = "dv"
            output = ["dv.vcf.gz"]
            shell = "run_deepvariant --ref=ref.fa --reads=sample.bam --output_vcf=dv.vcf.gz"

            [[rules]]
            name = "vep_online"
            output = ["annot.vcf"]
            shell = "vep -i input.vcf -o annot.vcf"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        let codes: Vec<&str> = warnings.iter().map(|w| w.code.as_str()).collect();

        assert!(codes.contains(&"SCI-MUTECT2-TUMOR-ONLY"));
        assert!(codes.contains(&"SCI-MUTECT2-NO-PON"));
        assert!(codes.contains(&"SCI-MUTECT2-NO-GERMLINE"));
        assert!(codes.contains(&"SCI-HAPLOTYPECALLER-GVCF-ERC"));
        assert!(codes.contains(&"SCI-DEEPVARIANT-MODEL-TYPE"));
        assert!(codes.contains(&"SCI-VEP-OFFLINE-CACHE"));
    }

    #[test]
    fn test_alignment_extended_checks() {
        let toml = r#"
            [workflow]
            name = "aln_checks"

            [[rules]]
            name = "bwa_no_rg"
            output = ["aln.sam"]
            shell = "bwa mem ref.fa r1.fq r2.fq > aln.sam"

            [[rules]]
            name = "sort_mem_oom"
            output = ["sorted.bam"]
            shell = "samtools sort -@ 8 -m 4G aln.sam -o sorted.bam"
            [rules.resources]
            memory = "16G"

            [[rules]]
            name = "markdup_rm"
            output = ["dedup.bam"]
            shell = "picard MarkDuplicates I=sorted.bam O=dedup.bam M=metrics.txt REMOVE_DUPLICATES=true"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        let codes: Vec<&str> = warnings.iter().map(|w| w.code.as_str()).collect();

        assert!(codes.contains(&"SCI-BWA-MEM-NO-RG"));
        assert!(codes.contains(&"SCI-SORT-THREADS-MEM-PRODUCT"));
        assert!(codes.contains(&"SCI-MARKDUPLICATES-REMOVE-DUPS"));
    }

    #[test]
    fn test_rnaseq_extended_checks() {
        let toml = r#"
            [workflow]
            name = "rna_checks"

            [[rules]]
            name = "salmon_auto"
            output = ["quant.sf"]
            shell = "salmon quant -i index -l A -1 r1.fq -2 r2.fq -o out_dir"

            [[rules]]
            name = "star_idx"
            output = ["Genome"]
            shell = "STAR --runMode genomeGenerate --genomeDir idx --sjdbGTFfile genes.gtf"

            [[rules]]
            name = "fc_overlap"
            output = ["counts.txt"]
            shell = "featureCounts -s 2 -O -a genes.gtf -o counts.txt in.bam"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        let codes: Vec<&str> = warnings.iter().map(|w| w.code.as_str()).collect();

        assert!(codes.contains(&"SCI-SALMON-LIBTYPE"));
        assert!(codes.contains(&"SCI-SALMON-NO-GC-BIAS"));
        assert!(codes.contains(&"SCI-STAR-SJDB-OVERHANG"));
        assert!(codes.contains(&"SCI-FEATURECOUNTS-MULTI-OVERLAP"));
    }

    #[test]
    fn test_epigenomics_extended_checks() {
        let toml = r#"
            [workflow]
            name = "epi_checks"

            [[rules]]
            name = "macs2_missing_size"
            output = ["peaks.narrowPeak"]
            shell = "macs2 callpeak -t treat.bam -n out"

            [[rules]]
            name = "bedtools_no_g"
            output = ["slop.bed"]
            shell = "bedtools slop -i in.bed -b 100 > slop.bed"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        let codes: Vec<&str> = warnings.iter().map(|w| w.code.as_str()).collect();

        assert!(codes.contains(&"SCI-MACS2-GENOME-SIZE"));
        assert!(codes.contains(&"SCI-CHIP-NO-CONTROL"));
        assert!(codes.contains(&"SCI-BEDTOOLS-GENOME-FILE"));
    }

    #[test]
    fn test_metagenomics_extended_checks() {
        let toml = r#"
            [workflow]
            name = "meta_checks"

            [[rules]]
            name = "kraken_zero_conf"
            output = ["kraken.out"]
            shell = "kraken2 --db db r1.fq > kraken.out"

            [[rules]]
            name = "bracken_no_r"
            output = ["bracken.out"]
            shell = "bracken -d db -i kraken.report -o bracken.out"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        let codes: Vec<&str> = warnings.iter().map(|w| w.code.as_str()).collect();

        assert!(codes.contains(&"SCI-KRAKEN2-CONFIDENCE"));
        assert!(codes.contains(&"SCI-BRACKEN-READ-LEN"));
    }

    #[test]
    fn test_concurrency_hardware_extended_checks() {
        let toml = r#"
            [workflow]
            name = "hw_checks"

            [[rules]]
            name = "heavy_hardcoded_tmp"
            output = ["out.bam"]
            shell = "samtools sort in.bam -T /tmp/sort_scratch -o out.bam"

            [[rules]]
            name = "multi_stage_pipe"
            output = ["final.bam"]
            shell = "bwa mem ref.fa r1.fq r2.fq | samtools view -bS - | samtools sort -o final.bam"

            [[rules]]
            name = "touch_mask_failure"
            output = ["corrupt.vcf.gz"]
            shell = "caller input.bam > corrupt.vcf.gz || touch corrupt.vcf.gz"

            [[rules]]
            name = "server_port"
            output = ["srv.done"]
            shell = "jupyter notebook --port 8888 --no-browser"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let warnings = analyze_scientific_constraints(&config);
        let codes: Vec<&str> = warnings.iter().map(|w| w.code.as_str()).collect();

        assert!(codes.contains(&"SCI-TMPDIR-NOT-SCRATCH"));
        assert!(codes.contains(&"SCI-PIPE-FAIL-DISABLED"));
        assert!(codes.contains(&"SCI-ZERO-BYTE-COMPLETION"));
        assert!(codes.contains(&"SCI-MULTI-TENANT-PORT-COLLISION"));
    }
}
