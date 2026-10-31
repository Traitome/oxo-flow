//! DAG biological subtyping checker and automated coercion rule synthesizer.

use serde::{Deserialize, Serialize};

use super::{
    BioFormat, BioTypeSpec, CompressionKind, IndexKind, SortOrder, TypeViolation,
    normalize_reference_build,
};
use crate::config::WorkflowConfig;
use crate::dag::WorkflowDag;

/// Diagnostic record capturing a compile-time biological type conflict between DAG rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeDiagnostic {
    /// Upstream producing rule name.
    pub producer_rule: String,
    /// Downstream consuming rule name.
    pub consumer_rule: String,
    /// Producer output channel or file pattern.
    pub producer_channel: String,
    /// Consumer input channel or file pattern.
    pub consumer_channel: String,
    /// Specific biological type violation.
    pub violation: TypeViolation,
}

impl BioTypeSpec {
    /// Verify whether `self` (the provided producer output) is a valid subtype of
    /// `expected` (the required consumer input).
    ///
    /// Accumulates all violations across format, sort order, index accompaniment,
    /// reference assembly build, read groups, and compression.
    pub fn is_subtype_of(&self, expected: &BioTypeSpec) -> Result<(), Vec<TypeViolation>> {
        let mut violations = Vec::new();

        // 1. Format check (TYPE-001)
        if self.format != expected.format && expected.format != BioFormat::Generic {
            let (suggestion, coercion_rule) = match (self.format, expected.format) {
                (BioFormat::Sam, BioFormat::Bam) => (
                    Some("convert SAM to binary BAM using samtools view".to_string()),
                    Some("samtools view -b -o {output} {input}".to_string()),
                ),
                (BioFormat::Bcf, BioFormat::Vcf) => (
                    Some("convert binary BCF to text VCF using bcftools view".to_string()),
                    Some("bcftools view -O v -o {output} {input}".to_string()),
                ),
                (BioFormat::Vcf, BioFormat::Bcf) => (
                    Some("convert VCF to binary BCF using bcftools view".to_string()),
                    Some("bcftools view -O b -o {output} {input}".to_string()),
                ),
                (BioFormat::Bam, BioFormat::Cram) => (
                    Some(
                        "convert BAM to reference-compressed CRAM using samtools view".to_string(),
                    ),
                    Some("samtools view -C -T {reference} -o {output} {input}".to_string()),
                ),
                _ => (
                    Some(format!(
                        "connect an adapter rule to convert {} into {}",
                        self.format, expected.format
                    )),
                    None,
                ),
            };

            violations.push(TypeViolation {
                code: "TYPE-001".to_string(),
                message: format!(
                    "biological format mismatch: producer emits {}, but consumer requires {}",
                    self.format, expected.format
                ),
                suggestion,
                coercion_rule,
            });
        }

        // 2. Sort order check (TYPE-002)
        if let Some(expected_order) = expected.sort_order
            && expected_order != SortOrder::Unsorted
            && expected_order != SortOrder::Unknown
        {
            let current_order = self.sort_order.unwrap_or(SortOrder::Unsorted);
            if !current_order.satisfies(expected_order) {
                violations.push(TypeViolation {
                    code: "TYPE-002".to_string(),
                    message: format!(
                        "sort order mismatch: producer output is {}, but consumer requires {}",
                        current_order, expected_order
                    ),
                    suggestion: Some(format!(
                        "sort alignments by {} using samtools sort",
                        expected_order
                    )),
                    coercion_rule: Some("samtools sort -o {output} {input}".to_string()),
                });
            }
        }

        // 3. Index accompaniment check (TYPE-003)
        if let Some(expected_index) = expected.index
            && self.index != Some(expected_index)
        {
            let coercion_rule = match expected_index {
                IndexKind::Bai | IndexKind::Csi => Some("samtools index {input}".to_string()),
                IndexKind::Crai => Some("samtools index {input}".to_string()),
                IndexKind::Tbi => Some("tabix -p vcf {input}".to_string()),
                IndexKind::Fai => Some("samtools faidx {input}".to_string()),
            };

            violations.push(TypeViolation {
                code: "TYPE-003".to_string(),
                message: format!(
                    "missing companion index: consumer requires {} index, but producer does not declare it",
                    expected_index
                ),
                suggestion: Some(format!(
                    "generate companion index for {} using index tool",
                    self.format
                )),
                coercion_rule,
            });
        }

        // 4. Reference genome assembly build compatibility (TYPE-004)
        if let (Some(my_build), Some(expected_build)) =
            (&self.reference_build, &expected.reference_build)
        {
            let norm_my = normalize_reference_build(my_build);
            let norm_expected = normalize_reference_build(expected_build);
            if norm_my != norm_expected {
                violations.push(TypeViolation {
                    code: "TYPE-004".to_string(),
                    message: format!(
                        "reference build conflict: producer aligned against '{}' ({}), but consumer requires '{}' ({})",
                        my_build, norm_my, expected_build, norm_expected
                    ),
                    suggestion: Some(
                        "realign reads against matching reference genome build or perform coordinate liftover"
                            .to_string(),
                    ),
                    coercion_rule: None,
                });
            }
        }

        // 5. Read group contract (TYPE-005)
        if let Some(expected_rg) = &expected.read_groups
            && expected_rg.required
        {
            let missing = self.read_groups.as_ref().is_none_or(|rg| !rg.required);
            if missing {
                violations.push(TypeViolation {
                    code: "TYPE-005".to_string(),
                    message: "missing read group contract: consumer requires valid @RG header tags, but producer does not declare them"
                        .to_string(),
                    suggestion: Some(
                        "add read groups during alignment (e.g. bwa mem -R) or inject with gatk AddOrReplaceReadGroups"
                            .to_string(),
                    ),
                    coercion_rule: Some(
                        "gatk AddOrReplaceReadGroups -I {input} -O {output} -RGID 1 -RGLB lib1 -RGPL ILLUMINA -RGPU unit1 -RGSM sample"
                            .to_string(),
                    ),
                });
            }
        }

        // 6. Compression stream check (TYPE-006)
        if let Some(expected_comp) = expected.compression
            && expected_comp != CompressionKind::None
        {
            let current_comp = self.compression.unwrap_or(CompressionKind::None);
            if !current_comp.satisfies(expected_comp) {
                let suggestion = if expected_comp == CompressionKind::Bgzip {
                    "compress with bgzip instead of standard gzip for random access compatibility"
                        .to_string()
                } else {
                    format!("compress output stream with {}", expected_comp)
                };

                violations.push(TypeViolation {
                    code: "TYPE-006".to_string(),
                    message: format!(
                        "block compression mismatch: consumer requires {}, but producer provides {}",
                        expected_comp, current_comp
                    ),
                    suggestion: Some(suggestion),
                    coercion_rule: Some("bgzip -c {input} > {output}".to_string()),
                });
            }
        }

        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }
}

/// Perform compile-time biological type checking across all DAG dependency edges.
///
/// If any rule specifies typed inputs/outputs (either directly or via `[types]`),
/// contracts along dataflow edges are checked for biological compatibility.
pub fn check_workflow_types(config: &WorkflowConfig, dag: &WorkflowDag) -> Vec<TypeDiagnostic> {
    let mut diagnostics = Vec::new();

    // Map rule name -> Rule reference
    let rule_map: std::collections::HashMap<&str, &crate::rule::Rule> =
        config.rules.iter().map(|r| (r.name.as_str(), r)).collect();

    // Walk all topological dependency edges
    for (consumer_name, rule) in &rule_map {
        let Some(consumer_types) = &rule.types else {
            continue;
        };

        // Get direct dependencies from DAG
        let deps = dag.dependencies(consumer_name).unwrap_or_default();
        for producer_name in deps {
            let Some(producer_rule) = rule_map.get(producer_name.as_str()) else {
                continue;
            };
            let Some(producer_types) = &producer_rule.types else {
                continue;
            };

            // Match output channels from producer with input channels of consumer
            for (out_ch, out_type_name) in &producer_types.outputs {
                for (in_ch, in_type_name) in &consumer_types.inputs {
                    // Channels match if identical, or either is "default", or channel names match output/input paths
                    let channel_match = out_ch == in_ch
                        || out_ch == "default"
                        || in_ch == "default"
                        || producer_rule
                            .output
                            .iter()
                            .any(|o| rule.input.iter().any(|i| i == o));

                    if channel_match {
                        let producer_spec = resolve_type_spec(out_type_name, config);
                        let consumer_spec = resolve_type_spec(in_type_name, config);

                        if let (Some(p_spec), Some(c_spec)) = (producer_spec, consumer_spec)
                            && let Err(violations) = p_spec.is_subtype_of(&c_spec)
                        {
                            for violation in violations {
                                diagnostics.push(TypeDiagnostic {
                                    producer_rule: producer_name.clone(),
                                    consumer_rule: (*consumer_name).to_string(),
                                    producer_channel: out_ch.clone(),
                                    consumer_channel: in_ch.clone(),
                                    violation,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    diagnostics
}

fn resolve_type_spec<'a>(
    name_or_inline: &'a str,
    config: &'a WorkflowConfig,
) -> Option<BioTypeSpec> {
    if let Some(declared) = config.types.get(name_or_inline) {
        return Some(declared.clone());
    }

    // Try parsing format name as a basic BioTypeSpec
    if let Ok(format) = name_or_inline.parse::<BioFormat>() {
        return Some(BioTypeSpec::new(format));
    }

    None
}
