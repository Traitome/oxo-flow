//! DAG biological subtyping checker and automated coercion rule synthesizer.

use serde::{Deserialize, Serialize};
use std::fmt;

use super::{
    BioFormat, BioTypeSpec, CompressionKind, SortOrder, TypeViolation, coercion_rule_for_index,
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

impl fmt::Display for TypeDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {} -> {} ({} -> {}): {}",
            self.violation.code,
            self.producer_rule,
            self.consumer_rule,
            self.producer_channel,
            self.consumer_channel,
            self.violation.message
        )?;
        if let Some(suggestion) = &self.violation.suggestion {
            write!(f, "; hint: {suggestion}")?;
        }
        Ok(())
    }
}

impl BioTypeSpec {
    /// Verify whether `self` (the provided producer output) is a valid subtype of
    /// `expected` (the required consumer input).
    ///
    /// Accumulates all violations across format, sort order, index accompaniment,
    /// reference assembly build, read groups, compression, and extensible
    /// biological attributes.
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
            let coercion_rule = coercion_rule_for_index(expected_index).map(str::to_string);

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
            let missing_fields: Vec<&str> = self
                .read_groups
                .as_ref()
                .filter(|rg| rg.required)
                .map(|rg| {
                    expected_rg
                        .fields
                        .iter()
                        .filter(|field| !rg.fields.contains(field))
                        .map(String::as_str)
                        .collect()
                })
                .unwrap_or_else(|| expected_rg.fields.iter().map(String::as_str).collect());
            if self.read_groups.as_ref().is_none_or(|rg| !rg.required) || !missing_fields.is_empty()
            {
                let missing_detail = if missing_fields.is_empty() {
                    "no read-group contract".to_string()
                } else {
                    format!("missing fields: {}", missing_fields.join(", "))
                };
                violations.push(TypeViolation {
                    code: "TYPE-005".to_string(),
                    message: format!(
                        "read group contract mismatch: consumer requires valid @RG header tags ({missing_detail})"
                    ),
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
                    coercion_rule: Some(match expected_comp {
                        CompressionKind::Bgzip => "bgzip -c {input} > {output}".to_string(),
                        CompressionKind::Zstd => "zstd -c {input} > {output}".to_string(),
                        CompressionKind::Gzip => "gzip -c {input} > {output}".to_string(),
                        // Unreachable: the enclosing `if` excludes `None`.
                        CompressionKind::None => "gzip -dc {input} > {output}".to_string(),
                    }),
                });
            }
        }

        // 7. Extensible attribute contract (TYPE-007)
        if !expected.attributes.is_empty() {
            let unmet: Vec<&String> = expected
                .attributes
                .iter()
                .filter(|(key, value)| self.attributes.get(*key) != Some(*value))
                .map(|(key, _)| key)
                .collect();
            if !unmet.is_empty() {
                violations.push(TypeViolation {
                    code: "TYPE-007".to_string(),
                    message: format!(
                        "attribute contract mismatch: consumer requires {} that the producer does not declare",
                        unmet
                            .iter()
                            .map(|key| format!("{}={}", key, expected.attributes[*key]))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    suggestion: Some(format!(
                        "declare the missing attributes on the producer output type: {}",
                        unmet
                            .iter()
                            .map(|key| format!(
                                "{}={}",
                                key,
                                expected.attributes[*key]
                            ))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                    coercion_rule: None,
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

    // Walk all topological dependency edges in declaration order so
    // diagnostics are deterministic across runs.
    for rule in &config.rules {
        let consumer_name = &rule.name;
        let Some(consumer_types) = &rule.types else {
            continue;
        };

        // Get direct dependencies from DAG
        let mut deps = dag.dependencies(consumer_name).unwrap_or_default();
        deps.sort();
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
                    // Channels match if identical, or either is "default" in single-channel context,
                    // or channel names match explicit shared output/input paths.
                    let channel_match = out_ch == in_ch
                        || (out_ch == "default" && consumer_types.inputs.len() == 1)
                        || (in_ch == "default" && producer_types.outputs.len() == 1)
                        || (producer_types.outputs.len() == 1
                            && consumer_types.inputs.len() == 1
                            && producer_rule
                                .output
                                .iter()
                                .any(|o| rule.input.iter().any(|i| i == o)))
                        || ((rule.input.iter().any(|i| i == out_ch) || out_ch == "default")
                            && (producer_rule.output.iter().any(|o| o == in_ch)
                                || in_ch == "default"));

                    if channel_match {
                        let producer_spec = resolve_type_spec(out_type_name, config);
                        let consumer_spec = resolve_type_spec(in_type_name, config);

                        for (role, channel, resolved) in [
                            ("producer", out_ch.as_str(), &producer_spec),
                            ("consumer", in_ch.as_str(), &consumer_spec),
                        ] {
                            if let Err(message) = resolved {
                                diagnostics.push(TypeDiagnostic {
                                    producer_rule: producer_name.clone(),
                                    consumer_rule: consumer_name.clone(),
                                    producer_channel: out_ch.clone(),
                                    consumer_channel: in_ch.clone(),
                                    violation: TypeViolation {
                                        code: "TYPE-UNKNOWN".to_string(),
                                        message: format!(
                                            "{role} type for channel '{channel}' cannot be resolved: {message}"
                                        ),
                                        suggestion: Some(
                                            "declare the type under [types.<name>] or use a supported format/attribute syntax"
                                                .to_string(),
                                        ),
                                        coercion_rule: None,
                                    },
                                });
                            }
                        }

                        if let (Ok(p_spec), Ok(c_spec)) = (producer_spec, consumer_spec)
                            && let Err(violations) = p_spec.is_subtype_of(&c_spec)
                        {
                            for violation in violations {
                                diagnostics.push(TypeDiagnostic {
                                    producer_rule: producer_name.clone(),
                                    consumer_rule: consumer_name.clone(),
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

/// Validate declared biological contracts across a workflow DAG.
///
/// Returns an empty success when the workflow declares no BioTypes. This is
/// the shared admission gate for CLI preflight and web run creation.
pub fn validate_workflow_types(
    config: &WorkflowConfig,
    dag: &WorkflowDag,
) -> Result<(), Vec<TypeDiagnostic>> {
    let diagnostics = check_workflow_types(config, dag);
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

fn resolve_type_spec(
    name_or_inline: &str,
    config: &WorkflowConfig,
) -> std::result::Result<BioTypeSpec, String> {
    let lookup = name_or_inline
        .strip_prefix("types.")
        .unwrap_or(name_or_inline);
    if let Some(declared) = config.types.get(lookup) {
        return Ok(declared.clone());
    }

    parse_inline_type_spec(name_or_inline)
}

fn parse_inline_type_spec(input: &str) -> std::result::Result<BioTypeSpec, String> {
    let input = input.trim();
    let (format_name, attrs) = match input.split_once('[') {
        Some((format, rest)) => {
            let attrs = rest
                .strip_suffix(']')
                .ok_or_else(|| format!("unterminated inline type attributes in '{input}'"))?;
            (format.trim(), Some(attrs))
        }
        None => (input, None),
    };

    let format = if format_name.eq_ignore_ascii_case("gvcf") {
        BioFormat::Vcf
    } else {
        format_name
            .parse::<BioFormat>()
            .map_err(|e| format!("unknown biological type '{format_name}': {e}"))?
    };
    let mut spec = BioTypeSpec::new(format);
    if format_name.eq_ignore_ascii_case("gvcf") {
        spec.attributes
            .insert("variant_type".to_string(), "germline".to_string());
    }

    let Some(attrs) = attrs else {
        return Ok(spec);
    };

    for attr in attrs.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (key, value) = attr
            .split_once('=')
            .ok_or_else(|| format!("inline type attribute '{attr}' must be key=value"))?;
        let key = key.trim();
        let value = value.trim().trim_matches(['"', '\'']);
        match key {
            "sort_order" => {
                spec.sort_order = Some(
                    value
                        .parse::<SortOrder>()
                        .map_err(|e| format!("invalid inline type attribute '{key}': {e}"))?,
                );
            }
            "index" => {
                spec.index = Some(
                    value
                        .parse::<super::IndexKind>()
                        .map_err(|e| format!("invalid inline type attribute '{key}': {e}"))?,
                );
            }
            "compression" => {
                spec.compression = Some(
                    value
                        .parse::<CompressionKind>()
                        .map_err(|e| format!("invalid inline type attribute '{key}': {e}"))?,
                );
            }
            "reference_build" => spec.reference_build = Some(value.to_string()),
            other => {
                spec.attributes.insert(other.to_string(), value.to_string());
            }
        };
    }

    Ok(spec)
}
