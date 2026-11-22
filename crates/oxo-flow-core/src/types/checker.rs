//! DAG biological subtyping checker and automated coercion rule synthesizer.

use serde::{Deserialize, Serialize};
use std::fmt;

use super::type_codes;
use super::{
    BioFormat, BioTypeSpec, CompressionKind, SortOrder, TypeViolation,
    coercion_rule_for_compression, coercion_rule_for_index, normalize_reference_build,
};
use crate::config::WorkflowConfig;
use crate::dag::WorkflowDag;

/// Advisory (non-blocking) BioType diagnostic codes.
///
/// These fire on dataflow edges where a contract *cannot be checked* (rather
/// than is known to be violated): a typed consumer whose upstream producer
/// declares no `[types]` (TYPE-008), a consumer that names a reference build
/// against a producer that omits one (TYPE-009), or a typed producer/consumer
/// edge whose channels cannot be paired at all (TYPE-010). Every bridge that
/// maps a [`TypeViolation`] into a user-facing actionable diagnostic must keep
/// advisory codes non-fatal — see `crate::format` and
/// `crate::scientific_preflight`. This is the single source of truth so the
/// two bridges cannot drift.
pub const ADVISORY_TYPE_CODES: [&str; 3] = [
    type_codes::TYPE_008,
    type_codes::TYPE_009,
    type_codes::TYPE_010,
];

/// Returns true when `code` is an advisory (non-fatal) BioType diagnostic.
pub fn is_advisory_type_code(code: &str) -> bool {
    ADVISORY_TYPE_CODES.contains(&code)
}

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
                code: type_codes::TYPE_001.to_string(),
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
                    code: type_codes::TYPE_002.to_string(),
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
                code: type_codes::TYPE_003.to_string(),
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
                    code: type_codes::TYPE_004.to_string(),
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
                    code: type_codes::TYPE_005.to_string(),
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
            // A format may imply a compression stream even when the producer
            // declares none: BAM/CRAM are BGZF by specification, so a plain
            // `BAM` already satisfies a `compression=bgzip` requirement
            // (issue #1255). This is the format invariant, NOT a blanket
            // tolerance — BAM against `compression=zstd`, and FASTQ/VCF
            // (which imply no stream wrapper) against `compression=bgzip`,
            // still reject below.
            let current_comp = self
                .compression
                .unwrap_or_else(|| BioTypeSpec::implied_compression(self.format));
            if !current_comp.satisfies(expected_comp) {
                let suggestion = if expected_comp == CompressionKind::Bgzip {
                    "compress with bgzip instead of standard gzip for random access compatibility"
                        .to_string()
                } else {
                    format!("compress output stream with {}", expected_comp)
                };

                violations.push(TypeViolation {
                    code: type_codes::TYPE_006.to_string(),
                    message: format!(
                        "block compression mismatch: consumer requires {}, but producer provides {}",
                        expected_comp, current_comp
                    ),
                    suggestion: Some(suggestion),
                    coercion_rule: coercion_rule_for_compression(expected_comp)
                        .map(str::to_string),
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
                    code: type_codes::TYPE_007.to_string(),
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
            // A typed consumer reading from a producer that declares no
            // `[types]` cannot be verified: the producer's biological output is
            // unknown, so the edge silently passes today. Report it as an
            // advisory (TYPE-008) finding instead of accepting (issue #1107
            // item 1). Advisory codes never fail the admission gate.
            let Some(producer_types) = &producer_rule.types else {
                diagnostics.push(TypeDiagnostic {
                    producer_rule: producer_name.clone(),
                    consumer_rule: consumer_name.clone(),
                    producer_channel: if producer_rule.output.is_empty() {
                        "<untyped>".to_string()
                    } else {
                        producer_rule.output.join(", ")
                    },
                    consumer_channel: if consumer_types.inputs.is_empty() {
                        "<untyped>".to_string()
                    } else {
                        consumer_types
                            .inputs
                            .keys()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    },
                    violation: TypeViolation {
                        code: type_codes::TYPE_008.to_string(),
                        message: "unverifiable biological contract: consumer declares types but the upstream producer declares none, so this dataflow edge cannot be checked".to_string(),
                        suggestion: Some(
                            "declare the producer's output type under [rules.types].outputs (or [types.<name>]) so its biological output can be verified against the consumer contract"
                                .to_string(),
                        ),
                        coercion_rule: None,
                    },
                });
                continue;
            };

            // Match output channels from producer with input channels of consumer
            let mut any_channel_paired = false;
            for (out_ch, out_type_name) in &producer_types.outputs {
                for (in_ch, in_type_name) in &consumer_types.inputs {
                    // Channels match if identical, or either is "default" against a
                    // singly-typed counterparty, or both sides are singly-typed with
                    // shared raw paths, or both channel keys cross-reference the
                    // opposite rule's raw path lists (path-keyed contracts). A bare
                    // "default" never pairs with a named channel in multi-typed
                    // contexts — name both sides there.
                    let channel_match = out_ch == in_ch
                        || (out_ch == "default" && consumer_types.inputs.len() == 1)
                        || (in_ch == "default" && producer_types.outputs.len() == 1)
                        || (producer_types.outputs.len() == 1
                            && consumer_types.inputs.len() == 1
                            && producer_rule
                                .output
                                .iter()
                                .any(|o| rule.input.iter().any(|i| i == o)))
                        || (rule.input.iter().any(|i| i == out_ch)
                            && producer_rule.output.iter().any(|o| o == in_ch));

                    if channel_match {
                        any_channel_paired = true;
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
                                        code: type_codes::TYPE_UNKNOWN.to_string(),
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

                        // The consumer names a reference build but the producer
                        // omits it, so the build cannot be checked: TYPE-004 only
                        // fires when BOTH sides name a build, which lets this
                        // incomplete pair pass silently (issue #1107 item 2).
                        // Report it as an advisory (TYPE-009) finding.
                        if let (Ok(p_spec), Ok(c_spec)) = (
                            resolve_type_spec(out_type_name, config),
                            resolve_type_spec(in_type_name, config),
                        ) && let Some(expected_build) = &c_spec.reference_build
                            && p_spec.reference_build.is_none()
                        {
                            diagnostics.push(TypeDiagnostic {
                                producer_rule: producer_name.clone(),
                                consumer_rule: consumer_name.clone(),
                                producer_channel: out_ch.clone(),
                                consumer_channel: in_ch.clone(),
                                violation: TypeViolation {
                                    code: type_codes::TYPE_009.to_string(),
                                    message: format!(
                                        "reference build unverifiable: consumer requires build '{}', but the upstream producer declares no reference build, so alignment compatibility cannot be checked",
                                        expected_build
                                    ),
                                    suggestion: Some(
                                        "declare the producer's reference_build so its alignment against the consumer's required build can be verified"
                                            .to_string(),
                                    ),
                                    coercion_rule: None,
                                },
                            });
                        }
                    }
                }
            }

            // A typed producer feeding a typed consumer where NOTHING paired:
            // every channel key on both sides failed to line up, so the loop
            // above checked no contract and this edge would otherwise pass in
            // total silence — a false-accept (issue #1254). Report one advisory
            // (TYPE-010) naming both sides' channels so the operator can key
            // them consistently. Fires only when zero pairs formed, so a
            // resolvable edge never gains noise.
            if !any_channel_paired {
                diagnostics.push(TypeDiagnostic {
                    producer_rule: producer_name.clone(),
                    consumer_rule: consumer_name.clone(),
                    producer_channel: producer_types
                        .outputs
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", "),
                    consumer_channel: consumer_types
                        .inputs
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", "),
                    violation: TypeViolation {
                        code: type_codes::TYPE_010.to_string(),
                        message: "unpairable biological contract: both rules declare types, but no output channel name matches an input channel name, so this dataflow edge cannot be checked".to_string(),
                        suggestion: Some(
                            "key the producer's [rules.types].outputs and the consumer's [rules.types].inputs with the same channel names (or align them with the rules' input/output paths) so the contracts can be paired"
                                .to_string(),
                        ),
                        coercion_rule: None,
                    },
                });
            }
        }
    }

    diagnostics
}

/// Validate declared biological contracts across a workflow DAG.
///
/// Returns an empty success when the workflow declares no BioTypes. This is
/// the shared admission gate for CLI preflight and web run creation, so only
/// *fatal* findings fail it: advisory unverifiable-edge codes
/// (TYPE-008/TYPE-009/TYPE-010) are dropped here because an unverifiable edge
/// is not a proven violation. Callers that surface every finding (the format
/// bridge, scientific preflight) still see the advisory diagnostics via
/// [`check_workflow_types`].
pub fn validate_workflow_types(
    config: &WorkflowConfig,
    dag: &WorkflowDag,
) -> Result<(), Vec<TypeDiagnostic>> {
    let diagnostics: Vec<TypeDiagnostic> = check_workflow_types(config, dag)
        .into_iter()
        .filter(|d| !is_advisory_type_code(&d.violation.code))
        .collect();
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

/// Marker phrase identifying the BioType type-admission bail (issue #1265).
///
/// The CLI repair loop recognizes the preflight bail by this marker and
/// routes it through the verified-repair machinery; the web admission errors
/// carry the same phrase; the eval harness greps it too, so the wording is a
/// contract: do not reword it. ONE constant, owned here next to
/// [`validate_workflow_types`] — the gate that produces the findings — so the
/// CLI and the web surfaces import it instead of hand-retyping the phrase
/// (reuse, never retype; issue #1352 F1).
pub const TYPE_ADMISSION_MARKER: &str = "declared biological type contracts are violated";

/// The one-line admission error text shared by the web admission surfaces
/// (issue #1352 F1): [`TYPE_ADMISSION_MARKER`] + the findings joined with
/// `"; "`. The CLI preflight bail deliberately uses a multi-line layout
/// (`{marker} — fix before running:\n  …`) for human stderr; this one-line
/// form is the HTTP error shape both web call sites emit.
pub fn type_admission_error_message(type_errors: &[TypeDiagnostic]) -> String {
    let findings = type_errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    format!("{TYPE_ADMISSION_MARKER}: {findings}")
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
