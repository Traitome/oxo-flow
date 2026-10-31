//! Nextflow DSL2 transpiler for converting Nextflow process blocks into oxo-flow rules.

use crate::error::{OxoFlowError, Result};

/// Extracted intermediate representation of a Nextflow process.
#[derive(Debug, Clone, Default)]
pub struct NextflowProcess {
    pub name: String,
    pub tag: Option<String>,
    pub conda: Option<String>,
    pub container: Option<String>,
    pub cpus: Option<u32>,
    pub memory: Option<String>,
    pub time: Option<String>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub script: String,
}

/// Transpile Nextflow DSL2 source code into an oxo-flow TOML pipeline.
pub fn transpile_nextflow(source: &str) -> Result<String> {
    let processes = parse_nextflow_processes(source)?;
    if processes.is_empty() {
        return Err(OxoFlowError::Config {
            message: "no Nextflow process definitions found in source".into(),
        });
    }

    let mut toml = String::new();
    toml.push_str("[workflow]\n");
    let wf_name = processes
        .first()
        .map(|p| p.name.to_lowercase())
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

    for p in &processes {
        toml.push_str("[[rules]]\n");
        let rule_name = p.name.to_lowercase();
        toml.push_str(&format!("name = \"{rule_name}\"\n"));

        let threads = p.cpus.unwrap_or(1);
        toml.push_str(&format!("threads = {threads}\n"));

        let mut res_fields = Vec::new();
        if let Some(mem) = &p.memory {
            let norm_mem = normalize_memory(mem);
            res_fields.push(format!("memory = \"{norm_mem}\""));
        }
        if let Some(time) = &p.time {
            res_fields.push(format!("time_limit = \"{time}\""));
        }
        if !res_fields.is_empty() {
            toml.push_str(&format!("resources = {{ {} }}\n", res_fields.join(", ")));
        }

        if let Some(container) = &p.container {
            toml.push_str(&format!("environment = {{ docker = \"{container}\" }}\n"));
        } else if let Some(conda) = &p.conda {
            toml.push_str(&format!("environment = {{ conda = \"{conda}\" }}\n"));
        }

        // Inputs
        let rendered_inputs = if p.inputs.is_empty() {
            vec![format!("data/{{sample}}_{rule_name}_input")]
        } else {
            p.inputs.clone()
        };
        let in_str = rendered_inputs
            .iter()
            .map(|s| format!("\"{}\"", s))
            .collect::<Vec<_>>()
            .join(", ");
        toml.push_str(&format!("input = [{in_str}]\n"));

        // Outputs
        let rendered_outputs = if p.outputs.is_empty() {
            vec![format!("results/{{sample}}/{rule_name}.out")]
        } else {
            p.outputs.clone()
        };
        let out_str = rendered_outputs
            .iter()
            .map(|s| format!("\"{}\"", s))
            .collect::<Vec<_>>()
            .join(", ");
        toml.push_str(&format!("output = [{out_str}]\n"));

        // Script / Shell
        let cleaned_script = p.script.trim();
        if cleaned_script.contains('\n') {
            toml.push_str("shell = '''\n");
            toml.push_str(cleaned_script);
            toml.push_str("\n'''\n");
        } else {
            let escaped = cleaned_script.replace("\"", "\\\"");
            toml.push_str(&format!("shell = \"{escaped}\"\n"));
        }

        // Inferred biological types
        let (in_type, out_type) =
            infer_process_types(&rule_name, &rendered_inputs, &rendered_outputs);
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

/// Infer BioType annotations based on tool name and file extensions.
fn infer_process_types(
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
        || lower.contains("minimap")
    {
        (Some("fastq"), Some("bam_coord"))
    } else if lower.contains("fastqc") {
        (Some("fastq"), None)
    } else if lower.contains("sort") {
        (Some("bam_coord"), Some("bam_coord"))
    } else if lower.contains("call")
        || lower.contains("mutect")
        || lower.contains("haplotype")
        || lower.contains("deepvariant")
        || lower.contains("strelka")
    {
        (Some("bam_coord"), Some("vcf_indexed"))
    } else if has_fastq_in && has_bam_out {
        (Some("fastq"), Some("bam_coord"))
    } else if has_bam_out && has_vcf_out {
        (Some("bam_coord"), Some("vcf_indexed"))
    } else {
        (None, None)
    }
}

/// Parse all `process Name { ... }` blocks from Nextflow source.
pub fn parse_nextflow_processes(source: &str) -> Result<Vec<NextflowProcess>> {
    let mut processes = Vec::new();
    let mut search_idx = 0;

    while let Some(proc_idx) = source[search_idx..].find("process ") {
        let abs_proc_idx = search_idx + proc_idx;
        let after_proc = &source[abs_proc_idx + 8..];

        let brace_rel_idx = match after_proc.find('{') {
            Some(idx) => idx,
            None => break,
        };
        let name_part = after_proc[..brace_rel_idx].trim();
        let process_name = name_part
            .split_whitespace()
            .next()
            .unwrap_or("unnamed_process")
            .to_string();

        let body_start = abs_proc_idx + 8 + brace_rel_idx + 1;

        // Find matching closing brace
        let mut depth = 1;
        let mut body_end = None;
        let chars = source[body_start..].char_indices();
        let mut in_string = false;
        let mut quote_char = ' ';

        for (rel_idx, c) in chars {
            if in_string {
                if c == quote_char {
                    in_string = false;
                }
            } else if c == '"' || c == '\'' {
                in_string = true;
                quote_char = c;
            } else if c == '{' {
                depth += 1;
            } else if c == '}' {
                depth -= 1;
                if depth == 0 {
                    body_end = Some(body_start + rel_idx);
                    break;
                }
            }
        }

        let body_end = match body_end {
            Some(idx) => idx,
            None => break,
        };

        let body = &source[body_start..body_end];
        let proc = parse_single_process(&process_name, body)?;
        processes.push(proc);

        search_idx = body_end + 1;
    }

    Ok(processes)
}

fn parse_single_process(name: &str, body: &str) -> Result<NextflowProcess> {
    let mut proc = NextflowProcess {
        name: name.to_string(),
        ..Default::default()
    };

    let mut in_input = false;
    let mut in_output = false;
    let mut script_lines: Vec<&str> = Vec::new();
    let mut in_script_triple = false;

    for line in body.lines() {
        let trimmed = line.trim();

        if in_script_triple {
            if trimmed.contains("\"\"\"") || trimmed.contains("'''") {
                let end_script = trimmed.split("\"\"\"").next().unwrap_or(trimmed);
                let end_script = end_script.split("'''").next().unwrap_or(end_script);
                if !end_script.trim().is_empty() {
                    script_lines.push(end_script);
                }
                in_script_triple = false;
            } else {
                script_lines.push(line);
            }
            continue;
        }

        if trimmed.starts_with("script:") || trimmed.starts_with("shell:") {
            in_input = false;
            in_output = false;
            if trimmed.contains("\"\"\"") || trimmed.contains("'''") {
                in_script_triple = true;
                let after = if trimmed.contains("\"\"\"") {
                    trimmed.split_once("\"\"\"").map(|x| x.1).unwrap_or("")
                } else {
                    trimmed.split_once("'''").map(|x| x.1).unwrap_or("")
                };
                if !after.trim().is_empty() {
                    script_lines.push(after);
                }
            }
            continue;
        }

        if trimmed == "\"\"\"" || trimmed == "'''" {
            in_script_triple = true;
            continue;
        }

        if trimmed.starts_with("input:") {
            in_input = true;
            in_output = false;
            continue;
        }

        if trimmed.starts_with("output:") {
            in_input = false;
            in_output = true;
            continue;
        }

        if trimmed.starts_with("when:") {
            in_input = false;
            in_output = false;
            continue;
        }

        // Directives
        if trimmed.starts_with("cpus ") || trimmed.starts_with("cpus=") {
            let val = trimmed
                .trim_start_matches("cpus")
                .trim_start_matches('=')
                .trim();
            if let Ok(c) = val.parse::<u32>() {
                proc.cpus = Some(c);
            }
        } else if trimmed.starts_with("memory ") || trimmed.starts_with("memory=") {
            let val = trimmed
                .trim_start_matches("memory")
                .trim_start_matches('=')
                .trim();
            let clean = val.trim_matches('\'').trim_matches('"').trim();
            proc.memory = Some(clean.replace(" ", ""));
        } else if trimmed.starts_with("time ") || trimmed.starts_with("time=") {
            let val = trimmed
                .trim_start_matches("time")
                .trim_start_matches('=')
                .trim();
            let clean = val.trim_matches('\'').trim_matches('"').trim();
            proc.time = Some(clean.to_string());
        } else if trimmed.starts_with("conda ") {
            let val = trimmed
                .trim_start_matches("conda")
                .trim()
                .trim_matches('\'')
                .trim_matches('"');
            proc.conda = Some(val.to_string());
        } else if trimmed.starts_with("container ") {
            let val = trimmed
                .trim_start_matches("container")
                .trim()
                .trim_matches('\'')
                .trim_matches('"');
            if !val.contains("${") {
                proc.container = Some(val.to_string());
            } else if val.contains("biocontainers/")
                && let Some(pos) = val.find("biocontainers/")
            {
                let sub = &val[pos..];
                let end = sub
                    .find('\'')
                    .or_else(|| sub.find('"'))
                    .unwrap_or(sub.len());
                proc.container = Some(sub[..end].to_string());
            }
        } else if in_input {
            if let Some(pattern) = extract_nextflow_channel_file(trimmed) {
                proc.inputs.push(pattern);
            }
        } else if in_output && let Some(pattern) = extract_nextflow_channel_file(trimmed) {
            proc.outputs.push(pattern);
        }
    }

    // Translate script variables
    let joined_script = script_lines.join("\n");
    proc.script = translate_nextflow_script(&joined_script, name);

    Ok(proc)
}

/// Extract path or file pattern from Nextflow input/output qualifiers.
fn extract_nextflow_channel_file(line: &str) -> Option<String> {
    if line.is_empty() || line.starts_with("//") {
        return None;
    }
    if line.contains("path(") {
        let after = line.split("path(").nth(1)?;
        let inside = after
            .split(')')
            .next()?
            .trim()
            .trim_matches('"')
            .trim_matches('\'');
        if inside == "reads" || inside == "fastq" || inside == "fastqs" {
            Some("data/{sample}.fastq.gz".into())
        } else if inside == "bam" {
            Some("aligned/{sample}.bam".into())
        } else if inside == "cram" {
            Some("aligned/{sample}.cram".into())
        } else if inside == "vcf" {
            Some("variants/{sample}.vcf.gz".into())
        } else if inside.starts_with("*.") {
            let ext = inside.trim_start_matches("*.");
            Some(format!("results/{{sample}}/{ext}.out"))
        } else if inside.contains("versions.yml") {
            None
        } else {
            Some(format!("data/{inside}"))
        }
    } else {
        None
    }
}

/// Translate Nextflow bash script variables into oxo-flow `{var}` syntax.
fn translate_nextflow_script(script: &str, _process_name: &str) -> String {
    let mut out = Vec::new();
    let mut skip_versions_block = false;

    for line in script.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("cat <<-END_VERSIONS") || trimmed.starts_with("cat <<END_VERSIONS") {
            skip_versions_block = true;
            continue;
        }
        if skip_versions_block {
            if trimmed == "END_VERSIONS" {
                skip_versions_block = false;
            }
            continue;
        }

        // Variable transformations
        let mut l = line.to_string();
        l = l.replace("$task.cpus", "{threads}");
        l = l.replace("${task.cpus}", "{threads}");
        l = l.replace("${meta.id}", "{sample}");
        l = l.replace("$meta.id", "{sample}");
        l = l.replace("${prefix}", "{sample}");
        l = l.replace("$prefix", "{sample}");
        l = l.replace("${reads[0]}", "{input[0]}");
        l = l.replace("${reads[1]}", "{input[1]}");
        l = l.replace("${reads}", "{input}");
        l = l.replace("$reads", "{input}");
        l = l.replace("${bam}", "{input[0]}");
        l = l.replace("$bam", "{input[0]}");

        out.push(l);
    }

    out.join("\n").trim().to_string()
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
