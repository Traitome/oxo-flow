//! Snakemake transpiler for converting Snakemake rules into oxo-flow workflows.

use crate::error::{OxoFlowError, Result};

/// Extracted intermediate representation of a Snakemake rule.
#[derive(Debug, Clone, Default)]
pub struct SnakemakeRule {
    pub name: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub threads: Option<u32>,
    pub memory: Option<String>,
    pub time: Option<String>,
    pub conda: Option<String>,
    pub container: Option<String>,
    pub shell: String,
    pub temporary: bool,
    pub protected: bool,
    pub temp_outputs: Vec<String>,
    pub protected_outputs: Vec<String>,
}

/// Transpile Snakemake source code into an oxo-flow TOML pipeline.
pub fn transpile_snakemake(source: &str) -> Result<String> {
    let rules = parse_snakemake_rules(source)?;
    if rules.is_empty() {
        return Err(OxoFlowError::Config {
            message: "no Snakemake rule definitions found in source".into(),
        });
    }

    let mut toml = String::new();
    toml.push_str("[workflow]\n");
    let wf_name = rules
        .first()
        .map(|r| r.name.to_lowercase())
        .unwrap_or_else(|| "imported_pipeline".into());
    toml.push_str(&format!("name = \"{wf_name}_pipeline\"\n"));
    toml.push_str("version = \"0.1.0\"\n\n");

    toml.push_str("[defaults]\nthreads = 1\n\n");

    // Standard biological types catalog for inferred contracts
    toml.push_str("[types.fastq]\nformat = \"fastq\"\ncompression = \"gzip\"\n\n");
    toml.push_str(
        "[types.bam_coord]\nformat = \"bam\"\nsort_order = \"coordinate\"\nindex = \"bai\"\n\n",
    );
    toml.push_str(
        "[types.vcf_indexed]\nformat = \"vcf\"\ncompression = \"bgzip\"\nindex = \"tbi\"\n\n",
    );

    for r in &rules {
        toml.push_str("[[rules]]\n");
        toml.push_str(&format!("name = \"{}\"\n", r.name));

        let threads = r.threads.unwrap_or(1);
        toml.push_str(&format!("threads = {threads}\n"));

        let mut res_fields = Vec::new();
        if let Some(mem) = &r.memory {
            let norm_mem = normalize_memory(mem);
            res_fields.push(format!("memory = \"{norm_mem}\""));
        }
        if let Some(time) = &r.time {
            res_fields.push(format!("time_limit = \"{time}\""));
        }
        if !res_fields.is_empty() {
            toml.push_str(&format!("resources = {{ {} }}\n", res_fields.join(", ")));
        }

        if let Some(container) = &r.container {
            toml.push_str(&format!("environment = {{ docker = \"{container}\" }}\n"));
        } else if let Some(conda) = &r.conda {
            toml.push_str(&format!("environment = {{ conda = \"{conda}\" }}\n"));
        }

        if r.temporary {
            toml.push_str("temporary = true\n");
        }
        if !r.temp_outputs.is_empty() {
            let t_str = r
                .temp_outputs
                .iter()
                .map(|s| format!("\"{}\"", s))
                .collect::<Vec<_>>()
                .join(", ");
            toml.push_str(&format!("temp_output = [{t_str}]\n"));
        }
        if !r.protected_outputs.is_empty() {
            let p_str = r
                .protected_outputs
                .iter()
                .map(|s| format!("\"{}\"", s))
                .collect::<Vec<_>>()
                .join(", ");
            toml.push_str(&format!("protected_output = [{p_str}]\n"));
        }

        // Inputs
        let rendered_inputs = if r.inputs.is_empty() {
            vec![format!("data/{{sample}}_{}_input", r.name)]
        } else {
            r.inputs.clone()
        };
        let in_str = rendered_inputs
            .iter()
            .map(|s| format!("\"{}\"", s))
            .collect::<Vec<_>>()
            .join(", ");
        toml.push_str(&format!("input = [{in_str}]\n"));

        // Outputs
        let rendered_outputs = if r.outputs.is_empty() {
            vec![format!("results/{{sample}}/{}.out", r.name)]
        } else {
            r.outputs.clone()
        };
        let out_str = rendered_outputs
            .iter()
            .map(|s| format!("\"{}\"", s))
            .collect::<Vec<_>>()
            .join(", ");
        toml.push_str(&format!("output = [{out_str}]\n"));

        // Shell script
        let cleaned_shell = r.shell.trim();
        if cleaned_shell.contains('\n') {
            toml.push_str("shell = '''\n");
            toml.push_str(cleaned_shell);
            toml.push_str("\n'''\n");
        } else {
            let escaped = cleaned_shell.replace("\"", "\\\"");
            toml.push_str(&format!("shell = \"{escaped}\"\n"));
        }

        // Inferred biological types
        let (in_type, out_type) =
            infer_snakemake_types(&r.name, &rendered_inputs, &rendered_outputs);
        if let Some(it) = in_type
            && let Some(ot) = out_type
        {
            toml.push_str(&format!(
                "types = {{ inputs = {{ \"{}\" = \"{}\" }}, outputs = {{ \"{}\" = \"{}\" }} }}\n",
                rendered_inputs[0], it, rendered_outputs[0], ot
            ));
        }
        toml.push('\n');
    }
    Ok(toml)
}

fn infer_snakemake_types(
    name: &str,
    inputs: &[String],
    outputs: &[String],
) -> (Option<&'static str>, Option<&'static str>) {
    let lower = name.to_lowercase();
    let has_fastq_in = inputs
        .iter()
        .any(|i| i.contains(".fastq") || i.contains(".fq"));
    let has_bam_out = outputs.iter().any(|o| o.contains(".bam"));
    let has_vcf_out = outputs.iter().any(|o| o.contains(".vcf"));

    if lower.contains("bwa")
        || lower.contains("bowtie")
        || lower.contains("hisat")
        || lower.contains("star")
    {
        (Some("fastq"), Some("bam_coord"))
    } else if lower.contains("fastqc") {
        (Some("fastq"), None)
    } else if lower.contains("sort") {
        (Some("bam_coord"), Some("bam_coord"))
    } else if lower.contains("call") || lower.contains("mutect") || lower.contains("haplotype") {
        (Some("bam_coord"), Some("vcf_indexed"))
    } else if has_fastq_in && has_bam_out {
        (Some("fastq"), Some("bam_coord"))
    } else if has_bam_out && has_vcf_out {
        (Some("bam_coord"), Some("vcf_indexed"))
    } else {
        (None, None)
    }
}

/// Parse all `rule <name>:` blocks from Snakemake source.
pub fn parse_snakemake_rules(source: &str) -> Result<Vec<SnakemakeRule>> {
    let mut rules = Vec::new();
    let lines: Vec<&str> = source.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        if trimmed.starts_with("rule ") && trimmed.ends_with(':') {
            let name = trimmed
                .trim_start_matches("rule ")
                .trim_end_matches(':')
                .trim()
                .to_string();

            let mut rule = SnakemakeRule {
                name,
                ..Default::default()
            };

            i += 1;
            let mut current_directive: Option<String> = None;
            let mut shell_lines = Vec::new();
            let mut in_triple_shell = false;

            while i < lines.len() {
                let next_line = lines[i];
                let next_trimmed = next_line.trim();

                // Check if a new top-level rule starts
                if (next_line.starts_with("rule ") || next_line.starts_with("checkpoint "))
                    && next_trimmed.ends_with(':')
                {
                    break;
                }

                if in_triple_shell {
                    if next_trimmed.contains("\"\"\"") || next_trimmed.contains("'''") {
                        let end_part = next_trimmed.split("\"\"\"").next().unwrap_or(next_trimmed);
                        let end_part = end_part.split("'''").next().unwrap_or(end_part);
                        if !end_part.trim().is_empty() {
                            shell_lines.push(end_part);
                        }
                        in_triple_shell = false;
                    } else {
                        shell_lines.push(next_line);
                    }
                    i += 1;
                    continue;
                }

                if next_trimmed.starts_with("input:") {
                    current_directive = Some("input".into());
                    let content = next_trimmed.trim_start_matches("input:").trim();
                    extract_paths_from_line(content, &mut rule.inputs);
                } else if next_trimmed.starts_with("output:") {
                    current_directive = Some("output".into());
                    let content = next_trimmed.trim_start_matches("output:").trim();
                    if content.contains("temp(") {
                        rule.temporary = true;
                    }
                    extract_output_paths(
                        content,
                        &mut rule.outputs,
                        &mut rule.temp_outputs,
                        &mut rule.protected_outputs,
                    );
                } else if next_trimmed.starts_with("threads:") {
                    current_directive = None;
                    let val = next_trimmed.trim_start_matches("threads:").trim();
                    if let Ok(t) = val.parse::<u32>() {
                        rule.threads = Some(t);
                    }
                } else if next_trimmed.starts_with("conda:") {
                    let val = next_trimmed
                        .trim_start_matches("conda:")
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'');
                    if val.is_empty() {
                        current_directive = Some("conda".into());
                    } else {
                        rule.conda = Some(val.to_string());
                        current_directive = None;
                    }
                } else if next_trimmed.starts_with("container:")
                    || next_trimmed.starts_with("singularity:")
                {
                    let val = next_trimmed
                        .trim_start_matches("container:")
                        .trim_start_matches("singularity:")
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'');
                    if val.is_empty() {
                        current_directive = Some("container".into());
                    } else {
                        let clean = val
                            .trim_start_matches("docker://")
                            .trim_start_matches("shub://");
                        rule.container = Some(clean.to_string());
                        current_directive = None;
                    }
                } else if next_trimmed.starts_with("resources:") {
                    current_directive = Some("resources".into());
                    let content = next_trimmed.trim_start_matches("resources:").trim();
                    parse_resources_line(content, &mut rule);
                } else if next_trimmed.starts_with("shell:") {
                    current_directive = Some("shell".into());
                    let content = next_trimmed.trim_start_matches("shell:").trim();
                    if content.contains("\"\"\"") || content.contains("'''") {
                        in_triple_shell = true;
                        let after = if content.contains("\"\"\"") {
                            content.split_once("\"\"\"").map(|x| x.1).unwrap_or("")
                        } else {
                            content.split_once("'''").map(|x| x.1).unwrap_or("")
                        };
                        if !after.trim().is_empty() {
                            shell_lines.push(after);
                        }
                    } else if !content.is_empty() {
                        shell_lines.push(content.trim_matches('"').trim_matches('\''));
                    }
                } else if !next_trimmed.is_empty() {
                    // Continuation line of current directive
                    if let Some(ref dir) = current_directive {
                        match dir.as_str() {
                            "input" => extract_paths_from_line(next_trimmed, &mut rule.inputs),
                            "output" => {
                                if next_trimmed.contains("temp(") {
                                    rule.temporary = true;
                                }
                                extract_output_paths(
                                    next_trimmed,
                                    &mut rule.outputs,
                                    &mut rule.temp_outputs,
                                    &mut rule.protected_outputs,
                                );
                            }
                            "resources" => parse_resources_line(next_trimmed, &mut rule),
                            "conda" => {
                                let val = next_trimmed.trim_matches('"').trim_matches('\'');
                                if !val.is_empty() {
                                    rule.conda = Some(val.to_string());
                                    current_directive = None;
                                }
                            }
                            "container" => {
                                let val = next_trimmed
                                    .trim_matches('"')
                                    .trim_matches('\'')
                                    .trim_start_matches("docker://")
                                    .trim_start_matches("shub://");
                                if !val.is_empty() {
                                    rule.container = Some(val.to_string());
                                    current_directive = None;
                                }
                            }
                            "shell" => {
                                if next_trimmed == "\"\"\"" || next_trimmed == "'''" {
                                    in_triple_shell = true;
                                } else if next_trimmed.starts_with("\"\"\"")
                                    || next_trimmed.starts_with("'''")
                                {
                                    in_triple_shell = true;
                                    let sub = next_trimmed
                                        .trim_start_matches("\"\"\"")
                                        .trim_start_matches("'''");
                                    if !sub.is_empty() {
                                        shell_lines.push(sub);
                                    }
                                } else {
                                    shell_lines.push(next_line);
                                }
                            }
                            _ => {}
                        }
                    }
                }

                i += 1;
            }

            let raw_shell = shell_lines.join("\n");
            rule.shell = translate_snakemake_shell(&raw_shell);
            rules.push(rule);
        } else {
            i += 1;
        }
    }

    Ok(rules)
}

fn extract_paths_from_line(line: &str, target: &mut Vec<String>) {
    for token in line.split(',') {
        let part = token.trim();
        if part.is_empty() {
            continue;
        }
        let val = if let Some((_, rhs)) = part.split_once('=') {
            rhs.trim()
        } else {
            part
        };
        let clean = val
            .trim_start_matches("temp(")
            .trim_start_matches("protected(")
            .trim_end_matches(')')
            .trim_matches('"')
            .trim_matches('\'')
            .trim_matches('[');
        if !clean.is_empty() && (clean.contains('/') || clean.contains('{') || clean.contains('.'))
        {
            target.push(clean.to_string());
        }
    }
}

fn parse_resources_line(line: &str, rule: &mut SnakemakeRule) {
    for part in line.split(',') {
        let trimmed = part.trim();
        if let Some((k, v)) = trimmed.split_once('=') {
            let key = k.trim();
            let val = v.trim().trim_matches('"').trim_matches('\'');
            if key == "mem_mb" {
                if let Ok(mb) = val.parse::<u64>() {
                    let gb = mb.div_ceil(1024);
                    rule.memory = Some(format!("{gb}GB"));
                }
            } else if key == "time" || key == "runtime" {
                rule.time = Some(val.to_string());
            }
        }
    }
}

fn extract_output_paths(
    line: &str,
    outputs: &mut Vec<String>,
    temp: &mut Vec<String>,
    prot: &mut Vec<String>,
) {
    for token in line.split(',') {
        let part = token.trim();
        if part.is_empty() {
            continue;
        }
        let is_temp = part.contains("temp(");
        let is_prot = part.contains("protected(");
        let val = if let Some((_, rhs)) = part.split_once('=') {
            rhs.trim()
        } else {
            part
        };
        let clean = val
            .trim_start_matches("temp(")
            .trim_start_matches("protected(")
            .trim_end_matches(')')
            .trim_matches(['"', '\''])
            .trim_matches('[');
        if !clean.is_empty() && (clean.contains('/') || clean.contains('{') || clean.contains('.'))
        {
            outputs.push(clean.to_string());
            if is_temp {
                temp.push(clean.to_string());
            }
            if is_prot {
                prot.push(clean.to_string());
            }
        }
    }
}

fn normalize_memory(raw: &str) -> String {
    let s = raw.trim().replace(" ", "");
    if s.ends_with("GB") || s.ends_with("gb") {
        format!("{}G", s[..s.len() - 2].trim())
    } else if s.ends_with("MB") || s.ends_with("mb") {
        format!("{}M", s[..s.len() - 2].trim())
    } else {
        s
    }
}

fn translate_snakemake_shell(shell: &str) -> String {
    let mut s = shell.trim().to_string();
    let is_triple_dq = s.starts_with("\"\"\"") && s.ends_with("\"\"\"");
    let is_triple_sq = s.starts_with("'''") && s.ends_with("'''");
    if (is_triple_dq || is_triple_sq) && s.len() >= 6 {
        s = s[3..s.len() - 3].trim().to_string();
    } else if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s = s[1..s.len() - 1].trim().to_string();
    }
    s = s.replace("{wildcards.sample}", "{sample}");
    s = s.replace("{input.reads}", "{input}");
    s = s.replace("{input.r1}", "{input[0]}");
    s = s.replace("{input.r2}", "{input[1]}");
    s = s.replace("{output.bam}", "{output[0]}");
    s = s.replace("{output.bai}", "{output[1]}");
    s.trim().to_string()
}
