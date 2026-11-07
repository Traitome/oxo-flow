//! Nextflow DSL2 transpiler for converting Nextflow process blocks into oxo-flow rules.

use super::{infer_bio_types, normalize_memory, push_types_contract};
use crate::error::{OxoFlowError, Result};
use regex::Regex;
use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

/// Matches `<name> = Channel.fromPath("<pattern>")` channel bindings.
static CHANNEL_VAR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?m)^\s*(?:def\s+)?([A-Za-z_]\w*)\s*=\s*Channel\.fromPath\s*\(\s*["']([^"']+)["']"#,
    )
    .expect("valid channel-variable regex")
});

/// Matches a direct `Channel.fromPath("<pattern>")` argument.
static FROM_PATH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"Channel\.fromPath\s*\(\s*["']([^"']+)["']"#).expect("valid fromPath regex")
});

/// Matches `${name}` / `$name` interpolations, including dotted members.
static NF_VAR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_.]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)")
        .expect("valid nextflow variable regex")
});

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
    /// Fallback input patterns derived from declared `path <name>` qualifiers.
    pub inputs: Vec<String>,
    /// Declared path variable names, in declaration order. Index `i` pairs with
    /// `inputs[i]` on the fallback list.
    pub input_vars: Vec<String>,
    /// Raw output expressions (`"${x}.count"`, `"*.bam"`, `"versions.yml"`, …).
    pub output_exprs: Vec<String>,
    pub script: String,
    /// True when the script/shell body was a single-quoted Nextflow string,
    /// which does not interpolate — recorded so the caller can warn.
    pub script_single_quoted: bool,
    /// Non-fatal translation problems (stripped `def` lines, unmappable
    /// interpolations, unsupported channel globs, …).
    pub script_warnings: Vec<String>,
}

/// A process rendered into concrete oxo-flow rule fields.
#[derive(Debug, Clone, Default)]
struct RenderedProcess {
    inputs: Vec<String>,
    outputs: Vec<String>,
    shell: String,
    warnings: Vec<String>,
}

/// Transpile Nextflow DSL2 source code into an oxo-flow TOML pipeline.
pub fn transpile_nextflow(source: &str) -> Result<String> {
    transpile_nextflow_with_warnings(source).map(|(toml, _)| toml)
}

/// Transpile Nextflow DSL2 source code, additionally returning every
/// non-fatal translation warning so the CLI can surface them.
pub fn transpile_nextflow_with_warnings(source: &str) -> Result<(String, Vec<String>)> {
    let processes = parse_nextflow_processes(source)?;
    if processes.is_empty() {
        return Err(OxoFlowError::Config {
            message: "no Nextflow process definitions found in source".into(),
        });
    }

    let channels = parse_workflow_channels(source, &processes);
    let mut warnings = Vec::new();
    let mut rendered = Vec::with_capacity(processes.len());
    for process in &processes {
        let rule = render_process(process, &channels)?;
        warnings.extend(rule.warnings.iter().cloned());
        rendered.push(rule);
    }

    // Infer the biological type of each rule first: only the catalog tables
    // actually referenced by an inferred contract are emitted, so an
    // unrecognized process never gets a fabricated `[types.*]` block.
    let inferred: Vec<(Option<&'static str>, Option<&'static str>)> = processes
        .iter()
        .zip(&rendered)
        .map(|(process, rule)| {
            infer_bio_types(&process.name.to_lowercase(), &rule.inputs, &rule.outputs)
        })
        .collect();
    let mut used_types = BTreeSet::new();
    for (in_type, out_type) in &inferred {
        used_types.extend(in_type.iter().chain(out_type.iter()).copied());
    }

    let mut toml = String::new();
    toml.push_str("[workflow]\n");
    let wf_name = processes
        .first()
        .map(|p| p.name.to_lowercase())
        .unwrap_or_else(|| "imported_pipeline".into());
    toml.push_str(&format!("name = \"{wf_name}_pipeline\"\n"));
    toml.push_str("version = \"0.1.0\"\n");
    // A wildcard input is only expandable when the workflow declares where the
    // wildcard values come from; reuse the first concrete source pattern so
    // `data/{sample}.txt` is discoverable instead of staying literal.
    if let Some(sample_pattern) = rendered
        .iter()
        .flat_map(|rule| rule.inputs.iter())
        .find(|pattern| pattern.contains("{sample}"))
    {
        toml.push_str(&format!(
            "sample_pattern = \"{}\"\n",
            escape_toml_basic(sample_pattern)
        ));
    }
    toml.push('\n');

    toml.push_str("[defaults]\nthreads = 1\n\n");

    // Emit only the catalog entries the inferred contracts actually use.
    if used_types.contains("fastq") {
        toml.push_str("[types.fastq]\nformat = \"fastq\"\ncompression = \"gzip\"\n\n");
    }
    if used_types.contains("bam_coord") {
        toml.push_str(
            "[types.bam_coord]\nformat = \"bam\"\nsort_order = \"coordinate\"\nindex = \"bai\"\n\n",
        );
    }
    if used_types.contains("vcf_indexed") {
        toml.push_str(
            "[types.vcf_indexed]\nformat = \"vcf\"\ncompression = \"bgzip\"\nindex = \"tbi\"\n\n",
        );
    }

    for (index, (process, rule)) in processes.iter().zip(&rendered).enumerate() {
        toml.push_str("[[rules]]\n");
        let rule_name = process.name.to_lowercase();
        toml.push_str(&format!("name = \"{rule_name}\"\n"));

        let threads = process.cpus.unwrap_or(1);
        toml.push_str(&format!("threads = {threads}\n"));

        let mut res_fields = Vec::new();
        if let Some(mem) = &process.memory {
            let norm_mem = normalize_memory(mem);
            res_fields.push(format!("memory = \"{norm_mem}\""));
        }
        if let Some(time) = &process.time {
            res_fields.push(format!("time_limit = \"{time}\""));
        }
        if !res_fields.is_empty() {
            toml.push_str(&format!("resources = {{ {} }}\n", res_fields.join(", ")));
        }

        if let Some(container) = &process.container {
            toml.push_str(&format!("environment = {{ docker = \"{container}\" }}\n"));
        } else if let Some(conda) = &process.conda {
            toml.push_str(&format!("environment = {{ conda = \"{conda}\" }}\n"));
        }

        // Never fabricate input/output placeholders: an empty list is the
        // honest representation of an unmapped channel (source/sink rules are
        // valid oxo-flow shapes).
        if !rule.inputs.is_empty() {
            let in_str = rule
                .inputs
                .iter()
                .map(|s| format!("\"{}\"", escape_toml_basic(s)))
                .collect::<Vec<_>>()
                .join(", ");
            toml.push_str(&format!("input = [{in_str}]\n"));
        }
        if !rule.outputs.is_empty() {
            let out_str = rule
                .outputs
                .iter()
                .map(|s| format!("\"{}\"", escape_toml_basic(s)))
                .collect::<Vec<_>>()
                .join(", ");
            toml.push_str(&format!("output = [{out_str}]\n"));
        }

        toml.push_str(&render_shell(&rule.shell));

        // Inferred biological types (no inference → no contract, no catalog).
        let (in_type, out_type) = inferred[index];
        push_types_contract(
            &mut toml,
            in_type.and_then(|t| rule.inputs.first().map(|channel| (channel.as_str(), t))),
            out_type.and_then(|t| rule.outputs.first().map(|channel| (channel.as_str(), t))),
        );
        toml.push('\n');
    }

    Ok((toml, warnings))
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

        let mut peek = chars.peekable();
        while let Some((rel_idx, c)) = peek.next() {
            if in_string {
                if c == '\\' {
                    // A backslash escapes the next character inside a
                    // string (`echo "a\"b"`): without skipping it, a body
                    // with an odd number of quote glyphs never closes and
                    // the whole process was silently DROPPED.
                    peek.next();
                } else if c == quote_char {
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
            // An unclosed body means malformed DSL or a scanner gap —
            // silently dropping every remaining process made `import`
            // report success for a fragment of the pipeline.
            None => {
                return Err(crate::error::OxoFlowError::Parse {
                    path: std::path::PathBuf::from("<nextflow source>"),
                    message: format!(
                        "process '{process_name}' has no closing brace — cannot find the end of its body"
                    ),
                });
            }
        };

        let body = &source[body_start..body_end];
        let proc = parse_single_process(&process_name, body)?;
        processes.push(proc);

        search_idx = body_end + 1;
    }

    Ok(processes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    None,
    Input,
    Output,
    Script,
}

fn parse_single_process(name: &str, body: &str) -> Result<NextflowProcess> {
    let mut proc = NextflowProcess {
        name: name.to_string(),
        ..Default::default()
    };

    let mut section = Section::None;
    let mut script_lines: Vec<String> = Vec::new();
    let mut triple_delim: Option<&'static str> = None;
    let mut line_quote: Option<char> = None;

    for raw_line in body.lines() {
        let trimmed = raw_line.trim();

        // Inside a triple-quoted script body: collect raw lines until the
        // closing delimiter (which may close mid-line).
        if let Some(delim) = triple_delim {
            if let Some(pos) = trimmed.find(delim) {
                let content = &trimmed[..pos];
                if !content.trim().is_empty() {
                    script_lines.push(content.to_string());
                }
                triple_delim = None;
            } else {
                script_lines.push(raw_line.to_string());
            }
            continue;
        }

        // Continuing a quoted (non-triple) script string that opened on an
        // earlier line.
        if let Some(quote) = line_quote {
            match split_quoted_tail(trimmed, quote) {
                Some((content, closed)) => {
                    script_lines.push(content);
                    if closed {
                        line_quote = None;
                    }
                }
                None => script_lines.push(trimmed.to_string()),
            }
            continue;
        }

        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('#') {
            continue;
        }

        // Section labels may share their line with the declaration/body.
        if let Some(rest) = trimmed.strip_prefix("input:") {
            section = Section::Input;
            collect_input_decl(rest, &mut proc);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("output:") {
            section = Section::Output;
            collect_output_decl(rest, &mut proc);
            continue;
        }
        if let Some(rest) = strip_script_label(trimmed) {
            section = Section::Script;
            if let Some(delim) = triple_opener(rest) {
                let after = &rest[delim.len()..];
                if let Some(end) = after.find(delim) {
                    if !after[..end].trim().is_empty() {
                        script_lines.push(after[..end].to_string());
                    }
                } else {
                    if !after.trim().is_empty() {
                        script_lines.push(after.to_string());
                    }
                    triple_delim = Some(delim);
                }
                if delim == "'''" {
                    proc.script_single_quoted = true;
                }
                continue;
            }
            let rest = rest.trim_start();
            if let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') {
                if quote == '\'' {
                    proc.script_single_quoted = true;
                }
                let inner = &rest[quote.len_utf8()..];
                match split_quoted_tail(inner, quote) {
                    Some((content, true)) => script_lines.push(content),
                    Some((content, false)) => {
                        script_lines.push(content);
                        line_quote = Some(quote);
                    }
                    None => {
                        script_lines.push(inner.to_string());
                        line_quote = Some(quote);
                    }
                }
                continue;
            }
            if !rest.is_empty() {
                // Unquoted remainder: keep it as a best-effort script line.
                script_lines.push(rest.to_string());
            }
            continue;
        }

        if section == Section::Script {
            if let Some(delim) = triple_opener(trimmed) {
                if delim == "'''" {
                    proc.script_single_quoted = true;
                }
                let after = &trimmed[delim.len()..];
                if let Some(end) = after.find(delim) {
                    if !after[..end].trim().is_empty() {
                        script_lines.push(after[..end].to_string());
                    }
                } else {
                    if !after.trim().is_empty() {
                        script_lines.push(after.to_string());
                    }
                    triple_delim = Some(delim);
                }
                continue;
            }
            script_lines.push(raw_line.to_string());
            continue;
        }

        // Directives (and ignored metadata) close the current section.
        if parse_directive(trimmed, &mut proc) {
            section = Section::None;
            continue;
        }

        if section == Section::Input {
            collect_input_decl(trimmed, &mut proc);
        } else if section == Section::Output {
            collect_output_decl(trimmed, &mut proc);
        }
    }

    proc.script = translate_nextflow_script(
        &script_lines.join("\n"),
        name,
        &proc.input_vars,
        proc.script_single_quoted,
        &mut proc.script_warnings,
    );

    Ok(proc)
}

/// Recognize a `script:` / `shell:` label and return the remainder of the line.
fn strip_script_label(line: &str) -> Option<&str> {
    line.strip_prefix("script:")
        .or_else(|| line.strip_prefix("shell:"))
}

/// Return the triple-quote delimiter at the start of `text`, if any.
fn triple_opener(text: &str) -> Option<&'static str> {
    if text.starts_with("\"\"\"") {
        Some("\"\"\"")
    } else if text.starts_with("'''") {
        Some("'''")
    } else {
        None
    }
}

/// Split `text` at the first unescaped `quote`; the flag says whether the
/// closing quote was found. `None` means the quote character never appears.
fn split_quoted_tail(text: &str, quote: char) -> Option<(String, bool)> {
    let mut escaped = false;
    for (idx, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote {
            let content = text[..idx].replace(&format!("\\{quote}"), &quote.to_string());
            return Some((content, true));
        }
    }
    None
}

/// Parse one directive line; returns true when the line was consumed.
fn parse_directive(trimmed: &str, proc: &mut NextflowProcess) -> bool {
    if trimmed.starts_with("cpus ") || trimmed.starts_with("cpus=") {
        let val = trimmed
            .trim_start_matches("cpus")
            .trim_start_matches('=')
            .trim();
        if let Ok(c) = val.parse::<u32>() {
            proc.cpus = Some(c);
        }
        return true;
    }
    if trimmed.starts_with("memory ") || trimmed.starts_with("memory=") {
        let val = trimmed
            .trim_start_matches("memory")
            .trim_start_matches('=')
            .trim();
        let clean = val.trim_matches('\'').trim_matches('"').trim();
        proc.memory = Some(clean.replace(" ", ""));
        return true;
    }
    if trimmed.starts_with("time ") || trimmed.starts_with("time=") {
        let val = trimmed
            .trim_start_matches("time")
            .trim_start_matches('=')
            .trim();
        let clean = val.trim_matches('\'').trim_matches('"').trim();
        proc.time = Some(clean.to_string());
        return true;
    }
    if trimmed.starts_with("conda ") {
        let val = trimmed
            .trim_start_matches("conda")
            .trim()
            .trim_matches('\'')
            .trim_matches('"');
        proc.conda = Some(val.to_string());
        return true;
    }
    if trimmed.starts_with("container ") {
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
        return true;
    }
    if trimmed.starts_with("tag ")
        || trimmed.starts_with("label ")
        || trimmed.starts_with("publishDir ")
        || trimmed.starts_with("when:")
        || trimmed.starts_with("stub:")
        || trimmed.starts_with("exec:")
    {
        return true;
    }
    false
}

/// Collect path variable names / literal patterns from an `input:` declaration.
fn collect_input_decl(line: &str, proc: &mut NextflowProcess) {
    let line = line.trim();
    if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
        return;
    }

    let mut found = false;
    for (idx, _) in line.match_indices("path(") {
        found = true;
        let after = &line[idx + "path(".len()..];
        let Some(end) = after.find(')') else {
            continue;
        };
        let inside = after[..end]
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .trim();
        push_input_qualifier(inside, proc);
    }
    if found {
        return;
    }

    for prefix in ["path ", "file "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            let rest = rest.split(',').next().unwrap_or(rest).trim();
            let value = rest.trim_matches('"').trim_matches('\'').trim();
            push_input_qualifier(value, proc);
            return;
        }
    }
}

fn push_input_qualifier(value: &str, proc: &mut NextflowProcess) {
    if value.is_empty() {
        return;
    }
    if value.contains('*') || value.contains('?') || value.contains('/') || value.contains('.') {
        let conversion = channel_glob_to_pattern(value);
        if let Some(warning) = conversion.warning {
            proc.script_warnings.push(format!(
                "input pattern '{}' could not be mapped faithfully: {warning}",
                value
            ));
        }
        proc.inputs.push(conversion.pattern);
        return;
    }
    let var = value
        .split_whitespace()
        .next()
        .unwrap_or(value)
        .trim_matches(',');
    if var.is_empty() {
        return;
    }
    proc.inputs.push(legacy_input_pattern(var));
    proc.input_vars.push(var.to_string());
}

/// Collect a raw output expression from an `output:` declaration.
fn collect_output_decl(line: &str, proc: &mut NextflowProcess) {
    let line = line.trim();
    if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
        return;
    }
    if line.starts_with("val(") || line.starts_with("stdout") || line.starts_with("env(") {
        return;
    }

    let expr = if let Some((_, after)) = line.split_once("path(") {
        let Some(end) = after.find(')') else {
            return;
        };
        after[..end].trim()
    } else if let Some(rest) = line.strip_prefix("path ") {
        rest.trim()
    } else if line.starts_with('"') || line.starts_with('\'') {
        line
    } else {
        return;
    };

    let expr = expr
        .trim()
        .trim_matches(',')
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim();
    if !expr.is_empty() {
        proc.output_exprs.push(expr.to_string());
    }
}

/// Fallback input pattern when no workflow channel source is available.
fn legacy_input_pattern(var: &str) -> String {
    match var {
        "reads" | "fastq" | "fastqs" => "data/{sample}.fastq.gz".into(),
        "bam" => "aligned/{sample}.bam".into(),
        "cram" => "aligned/{sample}.cram".into(),
        "vcf" => "variants/{sample}.vcf.gz".into(),
        _ => format!("data/{var}"),
    }
}

/// A concrete source pattern plus any honesty warning about its conversion.
struct PatternConversion {
    pattern: String,
    warning: Option<String>,
}

/// Convert a Nextflow file glob into an oxo-flow wildcard pattern.
///
/// `data/*.txt` → `data/{sample}.txt`. A glob with more than one `*` (paired
/// reads and similar) cannot be represented faithfully by one wildcard; the
/// first star maps and the rest are reported.
fn channel_glob_to_pattern(glob: &str) -> PatternConversion {
    let mut problems = Vec::new();
    if glob.contains('?') || glob.contains('[') {
        problems.push("character-class globs are not representable".to_string());
    }
    if glob.contains('{') {
        problems.push("brace-set globs (e.g. `{1,2}`) are not representable".to_string());
    }
    let stars = glob.matches('*').count();
    if stars > 1 {
        problems.push(format!(
            "{stars} wildcards: only the first maps to {{sample}}"
        ));
    }
    let pattern = if stars == 0 {
        glob.to_string()
    } else {
        glob.replacen('*', "{sample}", 1)
    };
    let warning = (!problems.is_empty()).then(|| problems.join("; "));
    PatternConversion { pattern, warning }
}

fn render_process(
    process: &NextflowProcess,
    channels: &HashMap<String, Vec<String>>,
) -> Result<RenderedProcess> {
    let mut warnings = process.script_warnings.clone();
    let mut inputs = process.inputs.clone();

    let source_channels = channels.get(&process.name);
    if let Some(channels) = source_channels
        && channels.len() > 1
    {
        warnings.push(format!(
            "process '{}' is invoked {} times; only the first invocation's channel is mapped",
            process.name,
            channels.len()
        ));
    }

    if let Some(glob) = source_channels.and_then(|channels| channels.first()) {
        let conversion = channel_glob_to_pattern(glob);
        if let Some(warning) = &conversion.warning {
            warnings.push(format!("process '{}': {warning}", process.name));
        }
        if inputs.is_empty() {
            inputs.push(conversion.pattern);
        } else {
            inputs[0] = conversion.pattern;
        }
    }

    let mut outputs = Vec::new();
    for expr in &process.output_exprs {
        let expr = expr.trim();
        if expr.is_empty() || expr == "versions.yml" {
            continue;
        }
        match map_output_expr(expr, &process.input_vars, &inputs) {
            Ok(mapped) => outputs.push(mapped),
            Err(reason) => warnings.push(format!(
                "process '{}': output '{}' cannot be mapped faithfully ({reason}); output omitted \
                 — declare it by hand",
                process.name, expr
            )),
        }
    }

    if process.script.trim().is_empty() {
        return Err(OxoFlowError::Parse {
            path: std::path::PathBuf::from("<nextflow source>"),
            message: format!(
                "process '{}' has no script/shell body the importer can extract — refusing to emit \
                 an empty shell. Add a `script:` block to the source or write this rule by hand.",
                process.name
            ),
        });
    }

    Ok(RenderedProcess {
        inputs,
        outputs,
        shell: process.script.clone(),
        warnings,
    })
}

/// Map a Nextflow output expression onto an oxo-flow output pattern.
///
/// `${x}.count` with input `data/{sample}.txt` becomes
/// `data/{sample}.txt.count`; a plain glob keeps the legacy heuristic.
fn map_output_expr(
    expr: &str,
    input_vars: &[String],
    inputs: &[String],
) -> std::result::Result<String, String> {
    if !NF_VAR_RE.is_match(expr) {
        return Ok(legacy_output_pattern(expr));
    }

    let mut unresolved: Option<String> = None;
    let mapped = NF_VAR_RE
        .replace_all(expr, |caps: &regex::Captures| {
            let ident = caps
                .get(1)
                .or_else(|| caps.get(2))
                .map(|m| m.as_str())
                .unwrap_or_default();
            if ident == "meta.id" || ident == "prefix" {
                return "{sample}".to_string();
            }
            if let Some(pos) = input_vars.iter().position(|var| var == ident)
                && let Some(pattern) = inputs.get(pos)
            {
                return pattern.clone();
            }
            unresolved = Some(caps[0].to_string());
            caps[0].to_string()
        })
        .into_owned();

    if let Some(token) = unresolved {
        return Err(format!("unresolved variable '{token}'"));
    }
    if mapped.contains('*') || mapped.contains('?') {
        return Err("glob alongside an interpolation".to_string());
    }
    Ok(mapped)
}

/// Legacy heuristic for non-interpolated output expressions.
fn legacy_output_pattern(expr: &str) -> String {
    let inside = expr
        .trim()
        .trim_matches(',')
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim();
    if let Some(ext) = inside.strip_prefix("*.") {
        format!("results/{{sample}}/{ext}.out")
    } else {
        format!("data/{inside}")
    }
}

/// Translate Nextflow bash script variables into oxo-flow `{var}` syntax.
fn translate_nextflow_script(
    script: &str,
    process_name: &str,
    input_vars: &[String],
    single_quoted: bool,
    warnings: &mut Vec<String>,
) -> String {
    let mut out = Vec::new();
    let mut skip_versions_block = false;
    let mut local_defs: Vec<String> = Vec::new();

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

        // Nextflow `def` preludes define workflow-local variables that have no
        // oxo-flow equivalent. Feeding them to bash would fail on `def: not
        // found`, so strip the line and report the construct loudly.
        if let Some(rest) = trimmed.strip_prefix("def ")
            && let Some(def_name) = rest
                .split(|c: char| c.is_whitespace() || c == '=')
                .next()
                .filter(|name| !name.is_empty())
        {
            if !local_defs.iter().any(|name| name == def_name) {
                local_defs.push(def_name.to_string());
                push_warning(
                    warnings,
                    format!(
                        "process '{process_name}': stripped Nextflow `def {def_name} = …` prelude \
                         — oxo-flow has no per-rule script variables, so references to it were \
                         left as shell text"
                    ),
                );
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

        for (idx, var) in input_vars.iter().enumerate() {
            let placeholder = if idx == 0 {
                "{input}".to_string()
            } else {
                format!("{{input[{idx}]}}")
            };
            l = replace_nextflow_var(&l, var, &placeholder);
        }

        out.push(l);
    }

    let joined = out.join("\n").trim().to_string();

    // Residual Nextflow interpolations are never substituted by the engine —
    // the tool would receive literal `${...}`. Name the construct rather than
    // let the import look clean. Shell-style uppercase env vars are exempt.
    let mut reported: Vec<String> = Vec::new();
    for caps in NF_VAR_RE.captures_iter(&joined) {
        let ident = caps
            .get(1)
            .or_else(|| caps.get(2))
            .map(|m| m.as_str())
            .unwrap_or_default();
        let token = caps[0].to_string();
        if ident.is_empty() || reported.contains(&token) {
            continue;
        }
        let is_shell_env = ident
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if is_shell_env {
            continue;
        }
        reported.push(token.clone());
        if local_defs.iter().any(|name| ident.starts_with(name)) {
            push_warning(
                warnings,
                format!(
                    "process '{process_name}': `{token}` references a stripped Nextflow `def` \
                     local — it may expand to an empty shell variable at run time"
                ),
            );
        } else {
            push_warning(
                warnings,
                format!(
                    "process '{process_name}': unresolved Nextflow interpolation `{token}` — it is \
                     left verbatim and will not be substituted; verify this command by hand"
                ),
            );
        }
    }
    for token in ["task.ext.", "params.", "Channel."] {
        if joined.contains(token) {
            push_warning(
                warnings,
                format!(
                    "process '{process_name}': script still references Nextflow `{token}` — this \
                     cannot be translated; verify this command by hand"
                ),
            );
        }
    }
    if single_quoted
        && input_vars
            .iter()
            .any(|var| joined.contains(&format!("${var}")))
    {
        push_warning(
            warnings,
            format!(
                "process '{process_name}': the script was single-quoted in Nextflow (no string \
                 interpolation), but declared input variables were mapped to placeholders anyway \
                 — verify this command by hand"
            ),
        );
    }

    joined
}

/// Replace `${var}` and `$var` in a shell line with an oxo-flow placeholder.
fn replace_nextflow_var(line: &str, var: &str, placeholder: &str) -> String {
    let braced = format!("${{{var}}}");
    let replaced = line.replace(&braced, placeholder);
    let bare = Regex::new(&format!(r"\b\${}\b", regex::escape(var)))
        .expect("valid escaped variable regex");
    bare.replace_all(&replaced, regex::NoExpand(placeholder))
        .into_owned()
}

fn push_warning(warnings: &mut Vec<String>, warning: String) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}

/// Parse workflow-block invocations, mapping each process name to the
/// `Channel.fromPath` patterns fed to it.
fn parse_workflow_channels(
    source: &str,
    processes: &[NextflowProcess],
) -> HashMap<String, Vec<String>> {
    let mut channel_vars: HashMap<String, String> = HashMap::new();
    for caps in CHANNEL_VAR_RE.captures_iter(source) {
        channel_vars
            .entry(caps[1].to_string())
            .or_insert_with(|| caps[2].to_string());
    }

    let mut channels: HashMap<String, Vec<String>> = HashMap::new();
    for process in processes {
        let invocation = Regex::new(&format!(r"\b{}\s*\(", regex::escape(&process.name)))
            .expect("valid process-name regex");
        for hit in invocation.find_iter(source) {
            let open = hit.start() + hit.as_str().rfind('(').unwrap_or(0);
            let Some(close) = find_matching_paren(source, open) else {
                continue;
            };
            let args = &source[open + 1..close];
            let mut patterns: Vec<String> = FROM_PATH_RE
                .captures_iter(args)
                .map(|caps| caps[1].to_string())
                .collect();
            if patterns.is_empty() {
                let ident = args.trim();
                if let Some(pattern) = channel_vars.get(ident) {
                    patterns.push(pattern.clone());
                }
            }
            if !patterns.is_empty() {
                channels
                    .entry(process.name.clone())
                    .or_default()
                    .extend(patterns);
            }
        }
    }
    channels
}

/// Find the `)` matching the `(` at `open`, honoring quoted strings.
fn find_matching_paren(source: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    for (idx, ch) in source[open..].char_indices() {
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => in_string = Some(ch),
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + idx);
                }
            }
            _ => {}
        }
    }
    None
}

/// Render a shell body as a TOML value, preferring a literal multi-line string
/// (shell backslashes must survive; basic strings would interpret them).
fn render_shell(shell: &str) -> String {
    let shell = shell.trim();
    if shell.contains('\n') && !shell.contains("'''") {
        format!("shell = '''\n{shell}\n'''\n")
    } else {
        format!("shell = \"{}\"\n", escape_toml_basic(shell))
    }
}

/// Escape a string for use inside a TOML basic (`"…"`) string.
fn escape_toml_basic(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkflowConfig;

    const ISSUE_1040_FIXTURE: &str = r#"
process COUNT {
    input: path x
    output: "${x}.count"
    script: "wc -l < ${x} > ${x}.count"
}
workflow { COUNT(Channel.fromPath("data/*.txt")) }
"#;

    #[test]
    fn captures_inline_script_and_maps_from_path_wildcard() {
        let (toml, warnings) =
            transpile_nextflow_with_warnings(ISSUE_1040_FIXTURE).expect("fixture must transpile");

        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        assert!(
            toml.contains(r#"shell = "wc -l < {input} > {input}.count""#),
            "inline script block must be captured: {toml}"
        );
        assert!(
            toml.contains(r#"input = ["data/{sample}.txt"]"#),
            "fromPath pattern must become a wildcard input: {toml}"
        );
        assert!(
            toml.contains(r#"output = ["data/{sample}.txt.count"]"#),
            "output must follow the input pattern: {toml}"
        );
        assert!(
            toml.contains(r#"sample_pattern = "data/{sample}.txt""#),
            "the wildcard needs a declared source: {toml}"
        );
        assert!(
            !toml.contains("_count_input"),
            "no fabricated input placeholder: {toml}"
        );
        assert!(!toml.contains("[types."), "no fabricated types: {toml}");

        let config = WorkflowConfig::parse(&toml).expect("generated workflow must parse");
        assert_eq!(config.rules.len(), 1);
        assert_eq!(
            config.rules[0].shell.as_deref(),
            Some("wc -l < {input} > {input}.count")
        );
    }

    #[test]
    fn captures_triple_quoted_multiline_script() {
        let nf = r#"
process TRIM {
    input: path reads
    output: "${reads}.trimmed"
    script: """
    cutadapt -j $task.cpus ${reads} > ${reads}.trimmed
    """
}
workflow { TRIM(Channel.fromPath("raw/*.fastq.gz")) }
"#;
        let (toml, warnings) = transpile_nextflow_with_warnings(nf).expect("triple quote");
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        assert!(
            toml.contains("cutadapt -j {threads} {input} > {input}.trimmed"),
            "multi-line script body lost: {toml}"
        );
        assert!(toml.contains(r#"input = ["raw/{sample}.fastq.gz"]"#));
        assert!(toml.contains(r#"output = ["raw/{sample}.fastq.gz.trimmed"]"#));
    }

    #[test]
    fn channel_variable_binding_is_followed() {
        let nf = r#"
process COUNT {
    input: path x
    output: "${x}.count"
    script: "wc -l < ${x} > ${x}.count"
}
reads = Channel.fromPath("data/*.txt")
workflow { COUNT(reads) }
"#;
        let toml = transpile_nextflow(nf).expect("variable channel");
        assert!(toml.contains(r#"input = ["data/{sample}.txt"]"#), "{toml}");
    }

    #[test]
    fn type_catalog_is_only_emitted_for_inferred_rules() {
        let nf = r#"
process BWA_MEM {
    input: tuple val(meta), path(reads)
    output: tuple val(meta), path("*.bam"), emit: bam
    script: "bwa mem $reads > out.bam"
}
process MYSTERY_STEP {
    input: path x
    output: "${x}.done"
    script: "touch ${x}.done"
}
"#;
        let toml = transpile_nextflow(nf).expect("mixed inference");
        assert!(toml.contains("[types.fastq]"), "fastq is inferred: {toml}");
        assert!(
            toml.contains("[types.bam_coord]"),
            "bam is inferred: {toml}"
        );
        assert!(
            !toml.contains("[types.vcf_indexed]"),
            "unused catalog entries must not be emitted: {toml}"
        );
    }

    #[test]
    fn def_prelude_is_stripped_with_a_loud_warning() {
        let nf = r#"
process MAPPED {
    input: path x
    output: "${x}.out"
    script: """
    def args = task.ext.args ?: ''
    wc -c $args ${x} > ${x}.out
    """
}
"#;
        let (toml, warnings) = transpile_nextflow_with_warnings(nf).expect("def prelude");
        assert!(
            !toml.contains("def args"),
            "Nextflow def prelude must not become bash: {toml}"
        );
        assert!(toml.contains("wc -c"), "best-effort shell retained: {toml}");
        let joined = warnings.join("\n");
        assert!(
            joined.contains("def args"),
            "warning must name the stripped construct: {joined}"
        );
        assert!(
            joined.contains("$args"),
            "warning must name the dangling reference: {joined}"
        );
    }

    #[test]
    fn empty_script_fails_loudly_instead_of_emitting_an_empty_shell() {
        let nf = r#"
process EMPTY {
    input: path x
    output: "x.out"
    script: ""
}
"#;
        let error = transpile_nextflow(nf).expect_err("empty script must fail");
        assert!(
            error
                .to_string()
                .contains("refusing to emit an empty shell"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn unmappable_output_is_warned_and_omitted_not_fabricated() {
        let nf = r#"
process OUT {
    input: path x
    output: "${params.outdir}/x.txt"
    script: "printenv > out.txt"
}
"#;
        let (toml, warnings) = transpile_nextflow_with_warnings(nf).expect("unmappable output");
        assert!(
            warnings.iter().any(|w| w.contains("params.outdir")),
            "warning must name the construct: {warnings:?}"
        );
        assert!(
            !toml.contains("output = ["),
            "unmappable output must be omitted, not fabricated: {toml}"
        );
    }
}
