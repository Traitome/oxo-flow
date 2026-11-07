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

/// Matches a whole `Channel.fromPath("<pattern>")` argument. Anchored so a
/// fromPath call combined with a channel operator (`… .map { … }`) is
/// rejected instead of being silently truncated to its prefix.
static FROM_PATH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^Channel\.fromPath\s*\(\s*["']([^"']+)["']\s*\)$"#).expect("valid fromPath regex")
});

/// Matches `${name}` / `$name` interpolations, including dotted members.
static NF_VAR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_.]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)")
        .expect("valid nextflow variable regex")
});

/// One `input:` path channel declaration, in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextflowInput {
    /// `path reads` / `path(reads)` — the value is supplied by the workflow.
    Var(String),
    /// `path "data/*.txt"` — a literal pattern declared in the process body.
    Literal(String),
}

/// One `output:` path expression plus its optional `emit:` name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextflowOutput {
    /// Raw expression (`"${x}.count"`, `"*.bam"`, `"versions.yml"`, …).
    pub expr: String,
    /// `emit: <name>` label, used by `PROCESS.out.<emit>` wiring.
    pub emit: Option<String>,
}

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
    /// Declared input channels, in declaration order.
    pub input_decls: Vec<NextflowInput>,
    /// Declared output channels, in declaration order.
    pub outputs: Vec<NextflowOutput>,
    pub script: String,
    /// True when the script/shell body was a single-quoted Nextflow string,
    /// which does not interpolate — recorded so the caller can warn.
    pub script_single_quoted: bool,
    /// Non-fatal translation problems (stripped `def` lines, unmappable
    /// interpolations, unsupported channel globs, …).
    pub script_warnings: Vec<String>,
}

impl NextflowProcess {
    /// Declared path variable names paired with their channel position.
    fn input_vars(&self) -> Vec<(usize, String)> {
        self.input_decls
            .iter()
            .enumerate()
            .filter_map(|(index, decl)| match decl {
                NextflowInput::Var(var) => Some((index, var.clone())),
                NextflowInput::Literal(_) => None,
            })
            .collect()
    }
}

/// A rendered output channel (pattern plus its `emit:` label, if any).
#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderedOutput {
    pattern: String,
    emit: Option<String>,
}

/// Which upstream output fed one rendered input channel.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WiredFrom {
    /// Lowercased rule name of the upstream process.
    upstream_rule: String,
    /// Position of the selected output in the upstream's rendered outputs.
    upstream_output_index: usize,
}

/// A process rendered into concrete oxo-flow rule fields.
#[derive(Debug, Clone, Default)]
struct RenderedProcess {
    inputs: Vec<String>,
    outputs: Vec<RenderedOutput>,
    depends_on: Vec<String>,
    /// Per rendered input: the upstream output it was wired from, if any.
    wired_inputs: Vec<Option<WiredFrom>>,
    shell: String,
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

    let wiring = parse_workflow_wiring(source, &processes)?;
    let mut warnings = Vec::new();
    let mut cache: Vec<Option<RenderedProcess>> = vec![None; processes.len()];
    let mut stack = Vec::new();
    let mut rendered = Vec::with_capacity(processes.len());
    for index in 0..processes.len() {
        rendered.push(render_process_at(
            index,
            &processes,
            &wiring,
            &mut cache,
            &mut stack,
            &mut warnings,
        )?);
    }

    // Infer the biological type of each rule first: only the catalog tables
    // actually referenced by an inferred contract are emitted, so an
    // unrecognized process never gets a fabricated `[types.*]` block.
    let mut inferred: Vec<(Option<&'static str>, Option<&'static str>)> = processes
        .iter()
        .zip(&rendered)
        .map(|(process, rule)| {
            infer_bio_types(
                &process.name.to_lowercase(),
                &rule.inputs,
                &rule
                    .outputs
                    .iter()
                    .map(|o| o.pattern.clone())
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    // A wired input inherits the type of the upstream output it consumes:
    // re-inferring from the pattern would re-attach the SOURCE file's type
    // (e.g. `fastq`) to a derived intermediate. Only the first input feeds
    // the emitted contract, and only a first-output wiring is unambiguous;
    // anything else stays untyped rather than guessed.
    for (index, rule) in rendered.iter().enumerate() {
        let Some(first_wired) = rule.wired_inputs.first().and_then(Option::as_ref) else {
            continue;
        };
        let upstream_index = processes
            .iter()
            .position(|process| process.name.to_lowercase() == first_wired.upstream_rule);
        inferred[index].0 = match upstream_index {
            Some(upstream) if first_wired.upstream_output_index == 0 => inferred[upstream].1,
            _ => None,
        };
    }
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
        if !rule.depends_on.is_empty() {
            let deps = rule
                .depends_on
                .iter()
                .map(|dep| format!("\"{dep}\""))
                .collect::<Vec<_>>()
                .join(", ");
            toml.push_str(&format!("depends_on = [{deps}]\n"));
        }

        // Canonical resource form (W025): `threads`/`memory` belong under
        // `resources`, never at rule level. A missing `cpus` directive
        // inherits `[defaults] threads = 1` instead of restating it.
        let mut res_fields = Vec::new();
        if let Some(cpus) = process.cpus {
            res_fields.push(format!("threads = {cpus}"));
        }
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
                .map(|out| format!("\"{}\"", escape_toml_basic(&out.pattern)))
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
            out_type.and_then(|t| rule.outputs.first().map(|out| (out.pattern.as_str(), t))),
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

    let input_vars = proc.input_vars();
    proc.script = translate_nextflow_script(
        &script_lines.join("\n"),
        name,
        &input_vars,
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
        push_input_decl(inside, proc);
    }
    if found {
        return;
    }

    for prefix in ["path ", "file "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            let rest = rest.split(',').next().unwrap_or(rest).trim();
            let value = rest.trim_matches('"').trim_matches('\'').trim();
            push_input_decl(value, proc);
            return;
        }
    }
}

fn push_input_decl(value: &str, proc: &mut NextflowProcess) {
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
        proc.input_decls
            .push(NextflowInput::Literal(conversion.pattern));
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
    proc.input_decls.push(NextflowInput::Var(var.to_string()));
}

/// Extract the `emit: <name>` label from an `output:` declaration.
fn parse_output_emit(line: &str) -> Option<String> {
    static EMIT_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\bemit:\s*([A-Za-z_]\w*)").expect("valid emit regex"));
    EMIT_RE.captures(line).map(|caps| caps[1].to_string())
}

/// Remove the `, emit: <name>` clause so it never leaks into the expression.
fn strip_output_emit(line: &str) -> String {
    static EMIT_CLAUSE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\s*,\s*emit:\s*[A-Za-z_]\w*").expect("valid emit-clause regex")
    });
    EMIT_CLAUSE_RE.replace(line, "").into_owned()
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

    let emit = parse_output_emit(line);
    let decl = strip_output_emit(line);
    let decl = decl.trim();
    let expr = if let Some((_, after)) = decl.split_once("path(") {
        let Some(end) = after.find(')') else {
            return;
        };
        after[..end].trim()
    } else if let Some(rest) = decl.strip_prefix("path ") {
        rest.trim()
    } else if decl.starts_with('"') || decl.starts_with('\'') {
        decl
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
        proc.outputs.push(NextflowOutput {
            expr: expr.to_string(),
            emit,
        });
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

/// Render one process, recursively resolving any upstream wiring it consumes.
///
/// `cache` memoizes rendered rules, `stack` detects wiring cycles. Errors are
/// returned (never swallowed): an unresolvable construct aborts the import
/// instead of emitting an invented path or type.
fn render_process_at(
    index: usize,
    processes: &[NextflowProcess],
    wiring: &HashMap<String, Vec<Invocation>>,
    cache: &mut Vec<Option<RenderedProcess>>,
    stack: &mut Vec<usize>,
    warnings: &mut Vec<String>,
) -> Result<RenderedProcess> {
    let process = &processes[index];
    if let Some(rendered) = &cache[index] {
        return Ok(rendered.clone());
    }
    if stack.contains(&index) {
        let cycle = stack
            .iter()
            .map(|&i| processes[i].name.as_str())
            .chain(std::iter::once(process.name.as_str()))
            .collect::<Vec<_>>()
            .join(" → ");
        return Err(wiring_error(format!(
            "process wiring cycle detected: {cycle} — break the cycle or import by hand"
        )));
    }
    stack.push(index);

    let mut rule_warnings = process.script_warnings.clone();
    let mut depends_on: Vec<String> = Vec::new();
    let mut wired_inputs: Vec<Option<WiredFrom>> = Vec::new();
    let input_vars = process.input_vars();

    let invocations = wiring.get(&process.name);
    if let Some(invocations) = invocations
        && invocations.len() > 1
    {
        rule_warnings.push(format!(
            "process '{}' is invoked {} times; only the first invocation's channel is mapped",
            process.name,
            invocations.len()
        ));
    }

    let inputs: Vec<String> = match invocations.and_then(|list| list.first()) {
        Some(invocation) => {
            if process.input_decls.is_empty() {
                // A value-only process (`input: val x`) takes no path
                // channel; the argument carries a value, not a file, so
                // there is nothing to map and nothing to invent.
                if !invocation.feeds.is_empty() {
                    rule_warnings.push(format!(
                        "process '{}' is invoked with {} channel argument(s) but declares no \
                         path inputs — value-only (`val`/`env`) inputs are not mapped",
                        process.name,
                        invocation.feeds.len()
                    ));
                }
                Vec::new()
            } else if invocation.feeds.len() != process.input_decls.len() {
                return Err(wiring_error(format!(
                    "process '{}' declares {} path input(s) but is invoked with {} channel \
                     argument(s) — cannot map them faithfully. Wire every declared path input \
                     or import this rule by hand.",
                    process.name,
                    process.input_decls.len(),
                    invocation.feeds.len()
                )));
            } else {
                let mut patterns = Vec::with_capacity(invocation.feeds.len());
                for feed in &invocation.feeds {
                    match feed {
                        ChannelFeed::Source(glob) => {
                            let conversion = channel_glob_to_pattern(glob);
                            if let Some(warning) = &conversion.warning {
                                rule_warnings
                                    .push(format!("process '{}': {warning}", process.name));
                            }
                            patterns.push(conversion.pattern);
                            wired_inputs.push(None);
                        }
                        ChannelFeed::Upstream { process, emit } => {
                            let (pattern, output_index, upstream_rule) = resolve_upstream_output(
                                process,
                                emit.as_deref(),
                                processes,
                                wiring,
                                cache,
                                stack,
                                warnings,
                            )?;
                            patterns.push(pattern);
                            wired_inputs.push(Some(WiredFrom {
                                upstream_rule: upstream_rule.clone(),
                                upstream_output_index: output_index,
                            }));
                            if !depends_on.contains(&upstream_rule) {
                                depends_on.push(upstream_rule);
                            }
                        }
                        ChannelFeed::Unresolved(expression) => {
                            return Err(unresolved_wiring_error(&process.name, expression));
                        }
                    }
                }
                debug_assert_eq!(patterns.len(), invocation.feeds.len());
                patterns
            }
        }
        // A standalone process definition (a Nextflow module file has no
        // workflow block) keeps the declaration-derived heuristic: there is
        // no wiring to resolve, only the declared names.
        None => process
            .input_decls
            .iter()
            .map(|decl| match decl {
                NextflowInput::Var(var) => legacy_input_pattern(var),
                NextflowInput::Literal(pattern) => pattern.clone(),
            })
            .collect(),
    };
    wired_inputs.resize(inputs.len(), None);

    let mut outputs = Vec::new();
    for output in &process.outputs {
        let expr = output.expr.trim();
        if expr.is_empty() || expr == "versions.yml" {
            continue;
        }
        match map_output_expr(expr, &input_vars, &inputs) {
            Ok(mapped) => outputs.push(RenderedOutput {
                pattern: mapped,
                emit: output.emit.clone(),
            }),
            Err(reason) => rule_warnings.push(format!(
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

    stack.pop();
    warnings.extend(rule_warnings.iter().cloned());
    let rendered = RenderedProcess {
        inputs,
        outputs,
        depends_on,
        wired_inputs,
        shell: process.script.clone(),
    };
    cache[index] = Some(rendered.clone());
    Ok(rendered)
}

/// Resolve the output channel selected by a `PROCESS.out` / `PROCESS.out.<emit>`
/// wiring expression. Returns (pattern, output index, rule name).
#[allow(clippy::too_many_arguments)]
fn resolve_upstream_output(
    upstream_name: &str,
    emit: Option<&str>,
    processes: &[NextflowProcess],
    wiring: &HashMap<String, Vec<Invocation>>,
    cache: &mut Vec<Option<RenderedProcess>>,
    stack: &mut Vec<usize>,
    warnings: &mut Vec<String>,
) -> Result<(String, usize, String)> {
    let upstream_index = processes
        .iter()
        .position(|process| process.name == upstream_name)
        .ok_or_else(|| {
            wiring_error(format!(
                "wiring expression references upstream process '{upstream_name}', which is not \
                 defined in this source — resolve the channel explicitly or import by hand"
            ))
        })?;
    let upstream = render_process_at(upstream_index, processes, wiring, cache, stack, warnings)?;
    let rule_name = processes[upstream_index].name.to_lowercase();
    let selector = emit.map_or_else(
        || format!("{upstream_name}.out"),
        |emit| format!("{upstream_name}.out.{emit}"),
    );
    let selected: Vec<(usize, &RenderedOutput)> = match emit {
        Some(emit) => upstream
            .outputs
            .iter()
            .enumerate()
            .filter(|(_, output)| output.emit.as_deref() == Some(emit))
            .collect(),
        None => upstream.outputs.iter().enumerate().collect(),
    };
    match selected.as_slice() {
        [] => {
            let available = upstream
                .outputs
                .iter()
                .map(|output| match &output.emit {
                    Some(emit) => format!("{emit} ({})", output.pattern),
                    None => output.pattern.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            Err(wiring_error(if upstream.outputs.is_empty() {
                format!(
                    "'{selector}' selects no output: process '{upstream_name}' has no mappable \
                     output — declare one in the process's output block or import this rule by hand"
                )
            } else {
                format!(
                    "'{selector}' selects no output: process '{upstream_name}' offers {available} — \
                     select one with {upstream_name}.out.<emit> or adjust the source"
                )
            }))
        }
        [(index, output)] => Ok((output.pattern.clone(), *index, rule_name)),
        _ => Err(wiring_error(format!(
            "'{selector}' is ambiguous: process '{upstream_name}' has {} outputs; select one \
             explicitly with {upstream_name}.out.<emit>",
            selected.len()
        ))),
    }
}

fn wiring_error(message: String) -> OxoFlowError {
    OxoFlowError::Parse {
        path: std::path::PathBuf::from("<nextflow source>"),
        message,
    }
}

/// Map a Nextflow output expression onto an oxo-flow output pattern.
///
/// `${x}.count` with input `data/{sample}.txt` becomes
/// `data/{sample}.txt.count`; a plain glob keeps the legacy heuristic.
fn map_output_expr(
    expr: &str,
    input_vars: &[(usize, String)],
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
            if let Some((position, _)) = input_vars.iter().find(|(_, var)| var == ident)
                && let Some(pattern) = inputs.get(*position)
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
    input_vars: &[(usize, String)],
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

        for (position, var) in input_vars {
            let placeholder = if *position == 0 {
                "{input}".to_string()
            } else {
                format!("{{input[{position}]}}")
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
            .any(|(_, var)| joined.contains(&format!("${var}")))
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

/// What feeds one process invocation argument.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ChannelFeed {
    /// `Channel.fromPath("<glob>")`, or a variable bound to one.
    Source(String),
    /// `PROCESS.out` / `PROCESS.out.<emit>`.
    Upstream {
        process: String,
        emit: Option<String>,
    },
    /// An unrecognized expression. Rejected for path inputs at render time;
    /// ignored (with a warning) when the process declares no path inputs,
    /// where the argument is a value, not a file channel.
    Unresolved(String),
}

/// One invocation of a process, in workflow order.
#[derive(Debug, Clone, Default)]
struct Invocation {
    feeds: Vec<ChannelFeed>,
}

/// Parse workflow-block invocations — both the call form
/// (`COUNT(Channel.fromPath("data/*.txt"))`, `COUNT(reads)`) and the pipe form
/// (`Channel.fromPath("data/*.txt") | COUNT`, `reads | COUNT`) — into the
/// channel fed to each process.
///
/// Every argument is classified or rejected: an unresolvable wiring
/// expression aborts the import instead of silently falling back to a
/// fabricated path.
fn parse_workflow_wiring(
    source: &str,
    processes: &[NextflowProcess],
) -> Result<HashMap<String, Vec<Invocation>>> {
    let mut channel_vars: HashMap<String, String> = HashMap::new();
    for caps in CHANNEL_VAR_RE.captures_iter(source) {
        channel_vars
            .entry(caps[1].to_string())
            .or_insert_with(|| caps[2].to_string());
    }

    let mut wiring: HashMap<String, Vec<Invocation>> = HashMap::new();
    for block in extract_workflow_blocks(source) {
        // Call form: NAME(<channel>, …)
        for process in processes {
            let invocation = Regex::new(&format!(r"\b{}\s*\(", regex::escape(&process.name)))
                .expect("valid process-name regex");
            for hit in invocation.find_iter(block) {
                // A commented-out invocation is not wiring.
                if is_in_line_comment(block, hit.start()) {
                    continue;
                }
                let open = hit.start() + hit.as_str().rfind('(').unwrap_or(0);
                let Some(close) = find_matching_paren(block, open) else {
                    continue;
                };
                let args = &block[open + 1..close];
                let feeds = parse_invocation_args(args, &channel_vars);
                wiring
                    .entry(process.name.clone())
                    .or_default()
                    .push(Invocation { feeds });
            }
        }
        // Pipe form: <channel> | NAME
        for (name, invocation) in parse_pipe_invocations(block, processes, &channel_vars)? {
            wiring.entry(name).or_default().push(invocation);
        }
    }
    Ok(wiring)
}

/// Extract the body of every top-level `workflow [NAME] { … }` block.
///
/// Pipe scanning is restricted to these blocks so a `|` inside a process
/// script (`cmd1 | cmd2`) can never be mistaken for process wiring.
fn extract_workflow_blocks(source: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut search = 0;
    while let Some(rel) = source[search..].find("workflow") {
        let start = search + rel;
        let after_keyword = start + "workflow".len();
        // Top-level definition only: the keyword starts a line, and is not a
        // member of a longer identifier (`workflowFoo`).
        let line_prefix = &source[source[..start].rfind('\n').map_or(0, |i| i + 1)..start];
        let prev = source[..start].chars().next_back();
        if !line_prefix.trim().is_empty() || prev.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            search = after_keyword;
            continue;
        }
        let mut rest = &source[after_keyword..];
        let mut body_open = after_keyword;
        if !rest.trim_start().starts_with('{') {
            // Optional workflow name: `workflow MY_PIPELINE { … }`
            let name_len = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .count();
            if name_len == 0 {
                search = after_keyword;
                continue;
            }
            let after_name = &rest[name_len..];
            let trimmed = after_name.trim_start();
            body_open += name_len + (after_name.len() - trimmed.len());
            rest = trimmed;
            if !rest.starts_with('{') {
                search = after_keyword;
                continue;
            }
        } else {
            let trimmed = rest.trim_start();
            body_open += rest.len() - trimmed.len();
        }
        let body_start = body_open + 1;
        match find_matching_brace(source, body_start) {
            Some(end) => {
                blocks.push(&source[body_start..end]);
                search = end + 1;
            }
            None => break,
        }
    }
    blocks
}

/// Whether `idx` falls after a `//` on the same line (a commented-out
/// invocation must not become wiring).
fn is_in_line_comment(text: &str, idx: usize) -> bool {
    let line_start = text[..idx].rfind('\n').map_or(0, |position| position + 1);
    text[line_start..idx].contains("//")
}

/// Find the `}` matching the `{` just before `body_start`, honoring quoted
/// strings and escapes (same scanner as the process-body parser).
fn find_matching_brace(source: &str, body_start: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    for (idx, ch) in source[body_start..].char_indices() {
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
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(body_start + idx);
                }
            }
            _ => {}
        }
    }
    None
}

/// Split an invocation's argument text at top-level commas.
fn split_top_level_args(args: &str) -> Vec<&str> {
    if args.trim().is_empty() {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    let mut start = 0usize;
    for (idx, ch) in args.char_indices() {
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
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(args[start..idx].trim());
                start = idx + 1;
            }
            _ => {}
        }
    }
    parts.push(args[start..].trim());
    // A trailing comma is legal in Nextflow invocation lists; it must not
    // become an empty (unresolvable) argument.
    if parts.len() > 1 && parts.last().is_some_and(|part| part.is_empty()) {
        parts.pop();
    }
    parts
}

fn parse_invocation_args(args: &str, channel_vars: &HashMap<String, String>) -> Vec<ChannelFeed> {
    split_top_level_args(args)
        .into_iter()
        .map(|arg| parse_channel_feed(arg, channel_vars))
        .collect()
}

/// Classify one wiring argument. Unrecognized expressions are carried as
/// `Unresolved` and rejected only when they feed a declared path input — a
/// value-only process argument is not a file channel.
fn parse_channel_feed(arg: &str, channel_vars: &HashMap<String, String>) -> ChannelFeed {
    let arg = arg.trim();
    if let Some(caps) = FROM_PATH_RE.captures(arg) {
        let whole = caps.get(0).expect("capture 0").as_str();
        if whole == arg {
            return ChannelFeed::Source(caps[1].to_string());
        }
        return ChannelFeed::Unresolved(arg.to_string());
    }
    if is_identifier(arg) {
        if let Some(glob) = channel_vars.get(arg) {
            return ChannelFeed::Source(glob.clone());
        }
        return ChannelFeed::Unresolved(arg.to_string());
    }
    if let Some((upstream, emit)) = parse_upstream_ref(arg) {
        return ChannelFeed::Upstream {
            process: upstream,
            emit,
        };
    }
    ChannelFeed::Unresolved(arg.to_string())
}

/// The loud, actionable rejection for an unresolvable path-channel wiring.
fn unresolved_wiring_error(process: &str, arg: &str) -> OxoFlowError {
    wiring_error(format!(
        "process '{process}' is wired from the unsupported expression `{arg}` — the importer \
         maps Channel.fromPath(\"glob\"), a channel variable bound to it, and \
         PROCESS.out[.<emit>] only; resolve this channel explicitly or import by hand"
    ))
}

fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Parse `PROCESS.out` / `PROCESS.out.<emit>` into `(PROCESS, emit)`.
fn parse_upstream_ref(text: &str) -> Option<(String, Option<String>)> {
    static UPSTREAM_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^([A-Za-z_]\w*)\.out(?:\.([A-Za-z_]\w*))?$")
            .expect("valid upstream-wiring regex")
    });
    let caps = UPSTREAM_RE.captures(text)?;
    Some((
        caps[1].to_string(),
        caps.get(2).map(|m| m.as_str().to_string()),
    ))
}

/// Parse `<channel> | PROCESS` statements inside one workflow block.
fn parse_pipe_invocations(
    block: &str,
    processes: &[NextflowProcess],
    channel_vars: &HashMap<String, String>,
) -> Result<Vec<(String, Invocation)>> {
    let mut invocations = Vec::new();
    for (idx, ch) in block.char_indices() {
        if ch != '|' {
            continue;
        }
        // `||` is a logical operator, never a channel pipe; a second `|` of
        // a chain is handled by its own statement check below.
        let prev = block[..idx].chars().next_back();
        let next = block[idx + 1..].chars().next();
        if prev == Some('|') || next == Some('|') {
            continue;
        }
        if is_in_line_comment(block, idx) {
            continue;
        }
        let rhs = block[idx + 1..].trim_start();
        let name: String = rhs
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() || !processes.iter().any(|process| process.name == name) {
            continue;
        }
        // The fed channel is the expression since the start of this
        // statement (a preceding pipe means a chain we cannot map). Braces
        // are NOT statement boundaries: a brace-set glob inside the quoted
        // channel expression would otherwise truncate it.
        let statement_start = block[..idx].rfind(['\n', ';']).map_or(0, |pos| pos + 1);
        let lhs = block[statement_start..idx].trim();
        if lhs.contains('|') {
            return Err(wiring_error(format!(
                "process '{name}' is wired from the pipe chain `{lhs} | {name}` — chained channel \
                 pipes cannot be mapped faithfully; feed the source directly to '{name}' or \
                 import this rule by hand"
            )));
        }
        let feed = match parse_channel_feed(lhs, channel_vars) {
            ChannelFeed::Unresolved(arg) => return Err(unresolved_wiring_error(&name, &arg)),
            feed => feed,
        };
        invocations.push((name, Invocation { feeds: vec![feed] }));
    }
    Ok(invocations)
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

    /// Audit fixture: the canonical Nextflow DSL2 pipe form.
    const PIPE_FORM_FIXTURE: &str = r#"
process COUNT {
    input: path x
    output: "${x}.count"
    script: "wc -l < ${x} > ${x}.count"
}
workflow { Channel.fromPath("data/*.txt") | COUNT }
"#;

    const TWO_PROCESS_FIXTURE: &str = r#"
process TRIM {
    input: path reads
    output: "${reads}.trimmed"
    script: "trim ${reads} > ${reads}.trimmed"
}
process ALIGN {
    input: path reads
    output: "${reads}.bam"
    script: "cp ${reads} aligned.bam"
}
workflow {
    TRIM(Channel.fromPath("data/*.fastq.gz"))
    ALIGN(TRIM.out)
}
"#;

    #[test]
    fn pipe_form_maps_the_same_wildcard_as_the_call_form() {
        let (toml, warnings) =
            transpile_nextflow_with_warnings(PIPE_FORM_FIXTURE).expect("pipe form must transpile");

        assert!(
            warnings.is_empty(),
            "a resolvable pipe form must not warn: {warnings:?}"
        );
        assert!(
            toml.contains(r#"input = ["data/{sample}.txt"]"#),
            "pipe form must map Channel.fromPath to the wildcard input, not invent a path: {toml}"
        );
        assert!(
            toml.contains(r#"output = ["data/{sample}.txt.count"]"#),
            "pipe-form output must follow the input pattern: {toml}"
        );
        assert!(
            toml.contains(r#"sample_pattern = "data/{sample}.txt""#),
            "the wildcard needs a declared source: {toml}"
        );
        assert!(
            !toml.contains("data/x"),
            "the input variable name must never be emitted as a path: {toml}"
        );

        let config = WorkflowConfig::parse(&toml).expect("generated workflow must parse");
        assert_eq!(config.rules.len(), 1);
    }

    #[test]
    fn pipe_form_follows_a_channel_variable() {
        let nf = r#"
process COUNT {
    input: path x
    output: "${x}.count"
    script: "wc -l < ${x} > ${x}.count"
}
reads = Channel.fromPath("data/*.txt")
workflow { reads | COUNT }
"#;
        let toml = transpile_nextflow(nf).expect("variable pipe");
        assert!(toml.contains(r#"input = ["data/{sample}.txt"]"#), "{toml}");
    }

    #[test]
    fn unresolvable_pipe_source_aborts_loudly() {
        let nf = r#"
process COUNT {
    input: path x
    output: "${x}.count"
    script: "wc -l < ${x} > ${x}.count"
}
workflow { Channel.fromFilePairs("data/*_{1,2}.fastq.gz") | COUNT }
"#;
        let error = transpile_nextflow(nf).expect_err("unresolvable pipe source must abort");
        let message = error.to_string();
        assert!(
            message.contains("Channel.fromFilePairs"),
            "the error must name the unsupported construct: {message}"
        );
        assert!(
            message.contains("import by hand"),
            "the error must give an actionable next step: {message}"
        );
    }

    #[test]
    fn chained_pipe_aborts_loudly() {
        let nf = r#"
process COUNT {
    input: path x
    output: "${x}.count"
    script: "wc -l < ${x} > ${x}.count"
}
process EXTRA {
    input: path x
    output: "${x}.extra"
    script: "cp ${x} ${x}.extra"
}
workflow { Channel.fromPath("data/*.txt") | COUNT | EXTRA }
"#;
        let error = transpile_nextflow(nf).expect_err("a pipe chain must abort");
        assert!(
            error.to_string().contains("pipe chain"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn process_wiring_maps_the_upstream_output_and_adds_the_edge() {
        let (toml, warnings) =
            transpile_nextflow_with_warnings(TWO_PROCESS_FIXTURE).expect("wiring must transpile");

        assert!(
            warnings.is_empty(),
            "a fully resolvable wiring must not warn: {warnings:?}"
        );
        assert!(
            toml.contains(r#"name = "trim""#)
                && toml.contains(r#"input = ["data/{sample}.fastq.gz"]"#),
            "the source process keeps the fromPath wildcard: {toml}"
        );
        assert!(
            toml.contains(r#"input = ["data/{sample}.fastq.gz.trimmed"]"#),
            "align must consume trim's OUTPUT pattern, not reinvent the source: {toml}"
        );
        assert!(
            toml.contains(r#"depends_on = ["trim"]"#),
            "the wiring must add a depends_on edge: {toml}"
        );
        assert!(
            !toml.contains("[types.fastq]"),
            "no fabricated fastq contract may be attached to a derived intermediate: {toml}"
        );

        let config = WorkflowConfig::parse(&toml).expect("generated workflow must parse");
        let dag =
            crate::dag::WorkflowDag::from_rules(&config.rules).expect("wired DAG must construct");
        assert_eq!(
            dag.edge_count(),
            1,
            "the imported DAG must keep TRIM → ALIGN"
        );
    }

    #[test]
    fn wired_input_inherits_the_upstream_output_type() {
        let nf = r#"
process BWA_MEM {
    input: path reads
    output: path "${reads}.bam", emit: bam
    script: "bwa mem ${reads} > ${reads}.bam"
}
process MYSTERY {
    input: path bam
    output: "${bam}.done"
    script: "touch ${bam}.done"
}
workflow {
    BWA_MEM(Channel.fromPath("data/*.fastq.gz"))
    MYSTERY(BWA_MEM.out.bam)
}
"#;
        let toml = transpile_nextflow(nf).expect("wired types");
        assert!(
            toml.contains(r#"inputs = { "data/{sample}.fastq.gz.bam" = "bam_coord" }"#),
            "the wired input must carry the upstream output's type, not a guess: {toml}"
        );
    }

    #[test]
    fn process_wiring_can_select_an_emit() {
        let nf = r#"
process TRIM {
    input: path reads
    output: path "${reads}.bam", emit: bam
    script: "cp ${reads} ${reads}.bam"
}
process ALIGN {
    input: path reads
    output: "${reads}.sorted"
    script: "cp ${reads} sorted.out"
}
workflow {
    TRIM(Channel.fromPath("data/*.fastq.gz"))
    ALIGN(TRIM.out.bam)
}
"#;
        let toml = transpile_nextflow(nf).expect("emit selection must transpile");
        assert!(
            toml.contains(r#"input = ["data/{sample}.fastq.gz.bam"]"#),
            "TRIM.out.bam must select the bam output: {toml}"
        );
        assert!(toml.contains(r#"depends_on = ["trim"]"#), "{toml}");
    }

    #[test]
    fn ambiguous_out_wiring_aborts() {
        let nf = r#"
process TRIM {
    input: path reads
    output:
    path "${reads}.bam", emit: bam
    path "${reads}.log", emit: log
    script: "cp ${reads} ${reads}.bam"
}
process ALIGN {
    input: path reads
    output: "${reads}.sorted"
    script: "cp ${reads} sorted.out"
}
workflow {
    TRIM(Channel.fromPath("data/*.fastq.gz"))
    ALIGN(TRIM.out)
}
"#;
        let error = transpile_nextflow(nf).expect_err("ambiguous .out must abort");
        let message = error.to_string();
        assert!(
            message.contains("ambiguous") && message.contains("TRIM.out.<emit>"),
            "the error must name the ambiguity and the fix: {message}"
        );
    }

    #[test]
    fn unknown_upstream_process_aborts() {
        let nf = r#"
process ALIGN {
    input: path reads
    output: "${reads}.sorted"
    script: "cp ${reads} sorted.out"
}
workflow { ALIGN(NOT_DEFINED.out) }
"#;
        let error = transpile_nextflow(nf).expect_err("unknown upstream must abort");
        assert!(
            error.to_string().contains("NOT_DEFINED"),
            "the error must name the unknown upstream: {error}"
        );
    }

    #[test]
    fn unsupported_wiring_expression_aborts() {
        let nf = r#"
process ALIGN {
    input: path reads
    output: "${reads}.sorted"
    script: "cp ${reads} sorted.out"
}
workflow { ALIGN(params.reads) }
"#;
        let error = transpile_nextflow(nf).expect_err("unsupported expression must abort");
        let message = error.to_string();
        assert!(
            message.contains("params.reads") && message.contains("PROCESS.out"),
            "the error must name the expression and the supported forms: {message}"
        );
    }

    #[test]
    fn imported_rules_do_not_trip_the_deprecation_lint() {
        let nf = r#"
process TRIM {
    cpus 4
    memory '8 GB'
    input: path reads
    output: "${reads}.trimmed"
    script: "trim ${reads} > ${reads}.trimmed"
}
workflow { TRIM(Channel.fromPath("data/*.fastq.gz")) }
"#;
        let toml = transpile_nextflow(nf).expect("transpile");
        assert!(
            toml.contains("resources = { threads = 4, memory = \"8G\" }"),
            "threads/memory must be emitted under resources: {toml}"
        );
        let config = WorkflowConfig::parse(&toml).expect("parse generated workflow");
        let diagnostics = crate::format::lint_format(&config, None);
        assert!(
            !diagnostics.iter().any(|d| d.code == "W025"),
            "the importer must not emit its own deprecated keys: {diagnostics:?}"
        );
    }

    #[test]
    fn value_only_process_invocation_warns_instead_of_aborting() {
        let nf = r#"
process REPORT {
    input: val prefix
    output: "report.txt"
    script: "echo ${prefix} > report.txt"
}
workflow { REPORT("run") }
"#;
        let (toml, warnings) =
            transpile_nextflow_with_warnings(nf).expect("value-only inputs must not abort");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("no path inputs") && w.contains("value-only")),
            "the unmapped value input must be reported: {warnings:?}"
        );
        assert!(!toml.contains("input = ["), "{toml}");
    }

    #[test]
    fn invoked_process_with_unmatched_input_count_aborts() {
        let nf = r#"
process PAIR {
    input: path r1
    path r2
    output: "${r1}.merged"
    script: "cat ${r1} ${r2} > ${r1}.merged"
}
workflow { PAIR(Channel.fromPath("data/*_R1.fastq.gz")) }
"#;
        let error = transpile_nextflow(nf).expect_err("missing channel argument must abort");
        let message = error.to_string();
        assert!(
            message.contains("declares 2 path input(s)") && message.contains("1 channel argument"),
            "the error must spell out the mismatch: {message}"
        );
    }
}
