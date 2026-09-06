//! `info` command — derive catalog metadata from a workflow file.
//!
//! Feeds the oxo-community catalog drift gate: derives the machine-checkable
//! subset of metadata.json (rule_count, tools, resources, environments,
//! config keys, sample groups, pairs, references) directly from the workflow
//! file, so hand-maintained catalog entries can be diffed against it.

use crate::commands::config_comments::extract_config_descriptions;
use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::{IncludeDirective, WorkflowConfig};
use oxo_flow_core::config_impact::is_engine_injected_key;
use oxo_flow_core::rule::{EnvironmentSpec, Rule};
use oxo_flow_core::scheduler::parse_memory_mb;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Derive and print catalog metadata for a workflow file.
///
/// JSON is the default output (stdout, machine-readable); `--format text`
/// prints a human-readable summary instead.
pub fn info_command(workflow: PathBuf, format: Option<String>) -> Result<()> {
    // Re-read the raw text: the TOML parser discards comments, and `info`
    // surfaces the `[config]` section comments as parameter descriptions.
    let text = std::fs::read_to_string(&workflow)
        .with_context(|| format!("failed to read workflow {}", workflow.display()))?;
    let cfg = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse workflow {}", workflow.display()))?;
    let mut descriptions = extract_config_descriptions(&text);
    // Included files merge their own `[config]` keys into the workflow —
    // their comments describe those keys too, and nested includes merge
    // just the same (parse.rs resolves them recursively), so walk the
    // local include tree. The main file's text wins (`or_insert` follows
    // parse.rs's merge order: earlier inserts keep priority); git-pinned
    // (issue #112) and URL includes are skipped (best-effort prose, never
    // a correctness matter).
    let mut visited: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    collect_include_descriptions(&workflow, &cfg.includes, &mut visited, &mut descriptions);

    let meta = derive_meta(&workflow, &cfg, &descriptions);

    match format.as_deref() {
        Some("text") => print_text(&meta),
        Some("json") | None => {
            println!("{}", serde_json::to_string_pretty(&meta)?);
        }
        Some(other) => {
            anyhow::bail!("unknown format '{other}' (expected 'json' or 'text')");
        }
    }
    Ok(())
}

/// Recursively merge `[config]` comment descriptions from local include
/// files (URL and git-pinned includes are skipped). Unreadable files are
/// silently skipped — descriptions are prose, never a correctness matter.
fn collect_include_descriptions(
    workflow: &Path,
    includes: &[IncludeDirective],
    visited: &mut std::collections::HashSet<PathBuf>,
    descriptions: &mut BTreeMap<String, String>,
) {
    for inc in includes {
        if inc.repo.is_some() || inc.path.starts_with("http://") || inc.path.starts_with("https://")
        {
            continue;
        }
        let inc_path = workflow
            .parent()
            .map(|dir| dir.join(&inc.path))
            .unwrap_or_else(|| PathBuf::from(&inc.path));
        if !visited.insert(inc_path.clone()) {
            continue;
        }
        let Ok(inc_text) = std::fs::read_to_string(&inc_path) else {
            continue;
        };
        for (key, description) in extract_config_descriptions(&inc_text) {
            descriptions.entry(key).or_insert(description);
        }
        // Nested includes: parse the included file to find its own
        // `[[include]]` directives (their paths resolve against the
        // included file's directory).
        if let Ok(inc_cfg) = WorkflowConfig::from_file(&inc_path) {
            collect_include_descriptions(&inc_path, &inc_cfg.includes, visited, descriptions);
        }
    }
}

/// Derive the machine-checkable catalog metadata from a parsed workflow.
/// `descriptions` carries the per-key `[config]` comments (see
/// `config_comments::extract_config_descriptions`).
fn derive_meta(
    workflow: &Path,
    cfg: &WorkflowConfig,
    descriptions: &BTreeMap<String, String>,
) -> Value {
    // Tools: conda/mamba env YAML stems + container image names, deduped
    // and sorted. Versions are NOT part of the catalog tool list — they live
    // in the TOML pins (oxo-community playbook §12).
    let mut tools: Vec<String> = cfg
        .rules
        .iter()
        .flat_map(|rule| rule_tools(&rule.environment))
        .collect();
    tools.sort();
    tools.dedup();

    // Max resources across rules, computed on a defaults-applied view —
    // the same value the engine uses at run time (run/dry-run both call
    // apply_defaults before scheduling).
    let mut effective = cfg.clone();
    effective.apply_defaults();
    let max_threads = effective
        .rules
        .iter()
        .map(|rule| rule.effective_threads())
        .max()
        .unwrap_or(1);
    // Compare parsed sizes, report the winning rule's original string so the
    // format ("16G" vs "16384M") stays truthful.
    let max_memory = effective
        .rules
        .iter()
        .filter_map(|rule| rule.effective_memory().map(|m| (parse_memory_mb(m), m)))
        .filter_map(|(mb, m)| mb.map(|value| (value, m)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, m)| m.to_string());

    // Environment backend distribution (system/conda/docker/…).
    let mut environments: BTreeMap<String, usize> = BTreeMap::new();
    for rule in &cfg.rules {
        *environments
            .entry(rule.environment.kind().to_string())
            .or_insert(0) += 1;
    }

    // [config] keys, excluding engine-injected keys — the run-time churn
    // keys (samples_list, pairs_list, samples_*) and the parse-time
    // injections (reference keyed-config values, reference_dir-derived
    // paths) are never catalog-relevant.
    let mut config_keys: Vec<String> = cfg
        .config
        .keys()
        .filter(|key| !is_engine_injected_key(key) && !cfg.is_injected_config_key(key))
        .cloned()
        .collect();
    config_keys.sort();

    // Ordering contract: deterministic under file edits, so the catalog
    // drift gate diffs stable output (same sorting as config_keys).
    let mut sample_groups: Vec<Value> = cfg
        .sample_groups
        .iter()
        .map(|group| json!({ "name": group.name, "samples": group.samples }))
        .collect();
    sample_groups.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let mut pairs: Vec<Value> = cfg
        .pairs
        .iter()
        .map(|pair| {
            json!({
                "pair_id": pair.pair_id,
                "experiment": pair.experiment,
                "control": pair.control,
            })
        })
        .collect();
    pairs.sort_by(|a, b| a["pair_id"].as_str().cmp(&b["pair_id"].as_str()));
    let mut references: Vec<Value> = cfg
        .references
        .iter()
        .map(|reference| json!({ "name": reference.name, "output": reference.output }))
        .collect();
    references.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));

    let mut meta = json!({
        "command": "info",
        "workflow": workflow.display().to_string(),
        "name": cfg.workflow.name,
        "version": cfg.workflow.version,
        "description": cfg.workflow.description,
        // Pre-expansion rule definitions — wildcard instances are a run-time
        // concept (depends on sample discovery), not catalog metadata.
        "rule_count": cfg.rules.len(),
        "tools": tools,
        "resources": {
            "max_threads": max_threads,
            "max_memory": max_memory,
        },
        "environments": environments,
        "config_keys": config_keys,
        "config": config_params(cfg, descriptions),
        "sample_groups": sample_groups,
        "pairs": pairs,
        "references": references,
        "input_dirs": top_level_dirs(cfg.rules.iter().flat_map(|rule| rule.input.to_vec())),
        "output_dirs": top_level_dirs(cfg.rules.iter().flat_map(|rule| rule.output.to_vec())),
    });
    // Git provenance (issue #124 pillar 3): a workflow inside a git
    // repository is uniquely addressable as repo + git ref. Keys are
    // omitted entirely when unavailable — catalog consumers test presence.
    if let Some((git_sha, git_remote, git_describe)) = git_provenance(workflow) {
        meta["git_sha"] = json!(git_sha);
        if let Some(remote) = git_remote {
            meta["git_remote"] = json!(remote);
        }
        if let Some(describe) = git_describe {
            meta["git_describe"] = json!(describe);
        }
    }
    meta
}

/// Git identity of the workflow's repository: `(HEAD sha, origin remote
/// URL, nearest tag)`. `None` when the workflow is not inside a git
/// repository — `info --json` then omits the git keys entirely (issue
/// #124 pillar 3, contract with the community catalog; field names agreed
/// with the community lane: git_sha / git_remote / git_describe).
fn git_provenance(workflow: &Path) -> Option<(String, Option<String>, Option<String>)> {
    let root = oxo_flow_core::git::find_repo_root(workflow)?;
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() {
        return None;
    }
    let git_output = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if value.is_empty() { None } else { Some(value) }
    };
    Some((
        sha,
        git_output(&["remote", "get-url", "origin"]),
        git_output(&["describe", "--tags", "--always"]),
    ))
}

/// `[config]` parameter records for the catalog parameter table: the default
/// value, its derived type, the rules that reference `{config.<key>}` in
/// their shell/script or input/output patterns, and — when the workflow
/// carries one — the `#` comment describing the key.
fn config_params(cfg: &WorkflowConfig, descriptions: &BTreeMap<String, String>) -> Vec<Value> {
    let mut records: Vec<Value> = cfg
        .config
        .iter()
        .filter(|(key, _)| !is_engine_injected_key(key) && !cfg.is_injected_config_key(key))
        .map(|(key, value)| {
            // Declared parameters (`key = { default, type, … }`) render
            // from their metadata: the raw toml table would surface the
            // whole declaration object as an opaque "table" default.
            let (default, value_type) = match cfg.config_meta.get(key) {
                Some(def) => (
                    declared_default(def),
                    def.type_.as_deref().unwrap_or("string").to_string(),
                ),
                None => {
                    let default = to_json_value(value);
                    let value_type = value_type_name(&default);
                    (default, value_type.to_string())
                }
            };
            let mut used_by: Vec<&str> = cfg
                .rules
                .iter()
                .filter(|rule| rule_uses_config(rule, key))
                .map(|rule| rule.name.as_str())
                .collect();
            used_by.sort_unstable();
            let mut record = json!({
                "key": key,
                "default": default,
                "value_type": value_type,
                "used_by": used_by,
            });
            // Only present when the workflow comments the key — the field is
            // optional so uncommented keys keep a minimal record.
            if let Some(description) = descriptions.get(key) {
                record["description"] = Value::String(description.clone());
            }
            record
        })
        .collect();
    records.sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
    records
}

/// JSON default for a declarative `[config]` entry, rendered per its
/// declared `type` (int/float/bool are real JSON numbers/booleans, not
/// quoted strings — the same coercion the engine applies at run time).
fn declared_default(def: &oxo_flow_core::config::ConfigDef) -> Value {
    let Some(default) = def.default.as_deref() else {
        return Value::Null;
    };
    match def.type_.as_deref() {
        Some("int") => default
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or(Value::Null),
        Some("float") => default
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::from)
            .unwrap_or(Value::Null),
        Some("bool") => default
            .parse::<bool>()
            .map(Value::from)
            .unwrap_or(Value::Null),
        _ => Value::String(default.to_string()),
    }
}

/// Does the rule reference the config key in its shell, script, I/O or
/// expansion patterns (`{config.<key>}`), or `when` condition (brace-less
/// `config.<key>`)?
fn rule_uses_config(rule: &Rule, key: &str) -> bool {
    let braced = format!("{{config.{key}}}");
    let paths_use = |paths: &[String]| paths.iter().any(|path| path.contains(&braced));
    if rule.shell.as_deref().is_some_and(|s| s.contains(&braced))
        || rule.script.as_deref().is_some_and(|s| s.contains(&braced))
        || paths_use(&rule.input.to_vec())
        || paths_use(&rule.output.to_vec())
        || rule
            .expand_inputs
            .iter()
            .any(|e| e.pattern.contains(&braced))
        || rule
            .input_groups
            .iter()
            .any(|g| g.pattern.contains(&braced))
        || rule
            .expand_inputs
            .iter()
            .any(|e| e.variables.values().any(|v| v.contains(&braced)))
    {
        return true;
    }
    rule.when
        .as_deref()
        .is_some_and(|expr| expr.contains(&braced) || contains_config_token(expr, key))
}

/// `config.<key>` as a full token in a `when` expression:
/// `config.run_qc` and `config.target_bed != ""` match, `config.run_qc_extra`
/// does not.
fn contains_config_token(text: &str, key: &str) -> bool {
    let token = format!("config.{key}");
    let mut start = 0;
    while let Some(pos) = text[start..].find(&token) {
        let end = start + pos + token.len();
        if !text[end..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
        {
            return true;
        }
        start = end;
    }
    false
}

/// JSON form of a `[config]` TOML value (serializes without loss).
fn to_json_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// Value type name derived from the JSON form of a `[config]` value.
fn value_type_name(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "string",
        Value::Number(n) if n.is_i64() || n.is_u64() => "int",
        Value::Number(_) => "float",
        Value::Bool(_) => "bool",
        Value::Array(_) => "array",
        Value::Object(_) => "table",
        Value::Null => "null",
    }
}

/// `"reference = /data/reference/GRCh38.fa, known_sites = …"`.
fn config_text(config: Option<&Vec<Value>>) -> String {
    match config {
        Some(config) if config.is_empty() => "none".to_string(),
        Some(config) => config
            .iter()
            .map(|record| {
                let key = record["key"].as_str().unwrap_or("?");
                format!("{key} = {}", record["default"])
            })
            .collect::<Vec<_>>()
            .join(", "),
        None => "none".to_string(),
    }
}

/// Tool names provided by an environment spec:
/// conda/mamba → the env YAML file stem ("envs/fastqc.yaml" → "fastqc");
/// docker/singularity → the image name without tag or registry path.
fn rule_tools(env: &EnvironmentSpec) -> Vec<String> {
    let mut tools = Vec::new();
    if let Some(ref conda) = env.conda {
        tools.push(conda_stem(conda));
    }
    if let Some(ref mamba) = env.mamba {
        tools.push(conda_stem(mamba));
    }
    if let Some(ref docker) = env.docker {
        tools.push(image_name(docker));
    }
    if let Some(ref singularity) = env.singularity {
        tools.push(image_name(singularity));
    }
    tools
}

/// "envs/fastqc.yaml" → "fastqc" (any path; the file stem).
fn conda_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// "biocontainers/bwa-mem2:2.2.1" → "bwa-mem2",
/// "docker://broadinstitute/gatk:4.5.0.0" → "gatk".
fn image_name(image: &str) -> String {
    let image = image.strip_prefix("docker://").unwrap_or(image);
    let without_tag = image
        .rsplit_once(':')
        .map(|(name, _)| name)
        .unwrap_or(image);
    without_tag
        .rsplit('/')
        .next()
        .unwrap_or(without_tag)
        .to_string()
}

/// Top-level directory names of input/output patterns (deduped, sorted).
/// Wildcard components like `{config.x}` are placeholders, not directories.
fn top_level_dirs<I>(paths: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut dirs: Vec<String> = paths
        .into_iter()
        .filter_map(|path| path.split('/').next().map(str::to_string))
        .filter(|dir| !dir.is_empty() && !dir.contains('{'))
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

/// Human-readable summary of the derived metadata.
fn print_text(meta: &Value) {
    println!(
        "{} {} v{}",
        "Workflow:".bold(),
        meta["name"].as_str().unwrap_or(""),
        meta["version"].as_str().unwrap_or("")
    );
    if let Some(description) = meta["description"].as_str()
        && !description.is_empty()
    {
        println!("  {description}");
    }
    println!("{} {}", "Rules:".bold(), meta["rule_count"]);
    println!(
        "{} {}",
        "Tools:".bold(),
        join_or_none(str_list(&meta["tools"]))
    );
    let mut resources = format!("{} threads", meta["resources"]["max_threads"]);
    if let Some(memory) = meta["resources"]["max_memory"].as_str() {
        resources.push_str(&format!(" / {memory}"));
    }
    println!("{} {resources}", "Resources (max per rule):".bold());
    let env_counts: Vec<String> = meta["environments"]
        .as_object()
        .map(|envs| {
            envs.iter()
                .map(|(backend, count)| format!("{backend}={count}"))
                .collect()
        })
        .unwrap_or_default();
    println!("{} {}", "Environments:".bold(), env_counts.join(", "));
    println!(
        "{} {}",
        "Config keys:".bold(),
        config_text(meta["config"].as_array())
    );
    println!(
        "{} {}",
        "Sample groups:".bold(),
        sample_groups_text(meta["sample_groups"].as_array())
    );
    println!(
        "{} {}",
        "Pairs:".bold(),
        pairs_text(meta["pairs"].as_array())
    );
    let reference_names: Vec<&str> = meta["references"]
        .as_array()
        .map(|refs| {
            refs.iter()
                .filter_map(|reference| reference["name"].as_str())
                .collect()
        })
        .unwrap_or_default();
    println!(
        "{} {}",
        "References:".bold(),
        join_or_none(Some(reference_names))
    );
    println!(
        "{} {}",
        "Input dirs:".bold(),
        join_or_none(str_list(&meta["input_dirs"]))
    );
    println!(
        "{} {}",
        "Output dirs:".bold(),
        join_or_none(str_list(&meta["output_dirs"]))
    );
}

/// `["a", "b"]` → `"a, b"`; empty/missing → `"none"`.
fn join_or_none(values: Option<Vec<&str>>) -> String {
    match values {
        Some(values) if values.is_empty() => "none".to_string(),
        Some(values) => values.join(", "),
        None => "none".to_string(),
    }
}

/// String items of a JSON array (or `None` when it is not an array).
fn str_list(value: &Value) -> Option<Vec<&str>> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_str).collect())
}

/// `"samples (3 samples), case (2 samples)"`.
fn sample_groups_text(groups: Option<&Vec<Value>>) -> String {
    match groups {
        Some(groups) if groups.is_empty() => "none".to_string(),
        Some(groups) => groups
            .iter()
            .map(|group| {
                let name = group["name"].as_str().unwrap_or("?");
                let count = group["samples"].as_array().map_or(0, Vec::len);
                format!("{name} ({count} samples)")
            })
            .collect::<Vec<_>>()
            .join(", "),
        None => "none".to_string(),
    }
}

/// `"CASE_001 (EXP_01 vs CTRL_01)"`; tumor-only pairs omit the control side.
fn pairs_text(pairs: Option<&Vec<Value>>) -> String {
    match pairs {
        Some(pairs) if pairs.is_empty() => "none".to_string(),
        Some(pairs) => pairs
            .iter()
            .map(|pair| {
                let id = pair["pair_id"].as_str().unwrap_or("?");
                let experiment = pair["experiment"].as_str().unwrap_or("?");
                match pair["control"].as_str() {
                    Some(control) => format!("{id} ({experiment} vs {control})"),
                    None => format!("{id} ({experiment})"),
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
        None => "none".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(path: &str) -> WorkflowConfig {
        WorkflowConfig::from_file(Path::new(path)).unwrap()
    }

    /// Load a gallery fixture and derive its metadata end-to-end, extracting
    /// comment descriptions from the raw file text like `info_command` does.
    fn meta(display: &str, fixture: &str) -> Value {
        let cfg = config(fixture);
        let text = std::fs::read_to_string(fixture).unwrap();
        derive_meta(
            Path::new(display),
            &cfg,
            &extract_config_descriptions(&text),
        )
    }

    #[test]
    fn conda_stem_keeps_only_file_stem() {
        assert_eq!(conda_stem("envs/fastqc.yaml"), "fastqc");
        assert_eq!(conda_stem("envs/samtools.yml"), "samtools");
        assert_eq!(conda_stem("qc.yaml"), "qc");
        assert_eq!(conda_stem(""), "");
    }

    #[test]
    fn image_name_strips_tag_and_registry_path() {
        assert_eq!(image_name("biocontainers/bwa-mem2:2.2.1"), "bwa-mem2");
        assert_eq!(image_name("docker://broadinstitute/gatk:4.5.0.0"), "gatk");
        assert_eq!(image_name("ubuntu:22.04"), "ubuntu");
        assert_eq!(image_name("biocontainers/fastqc"), "fastqc");
        assert_eq!(
            image_name("localhost:5000/biocontainers/star:2.7.10"),
            "star"
        );
    }

    #[test]
    fn git_provenance_keys_present_inside_repo() {
        // oxo-flow's own workspace is a git repository with an origin
        // remote: deriving metadata for a workflow inside it must carry
        // the git identity keys (issue #124 pillar 3). The real path is
        // passed as both display and fixture so the walk-up finds .git.
        let fixture = "../../examples/gallery/05_conda_environments.oxoflow";
        let meta = meta(fixture, fixture);
        assert_eq!(meta["git_sha"].as_str().map(|s| !s.is_empty()), Some(true));
        // git_remote is only derivable when the repository has an origin
        // remote — the key is omitted otherwise (the catalog contract).
        // Assert presence ONLY when this checkout actually has an origin,
        // so tarball / --no-remote / mirror checkouts don't fail the test
        // (issue #136 finding 29: the old unconditional assertion was
        // environment-coupled).
        let has_origin = std::process::Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert_eq!(
            meta["git_remote"].as_str().map(|s| !s.is_empty()),
            has_origin.then_some(true)
        );
        assert_eq!(
            meta["git_describe"].as_str().map(|s| !s.is_empty()),
            Some(true)
        );
    }

    #[test]
    fn git_provenance_keys_absent_outside_repo() {
        let dir = std::env::temp_dir().join(format!("oxo-info-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wf = dir.join("wf.oxoflow");
        std::fs::write(
            &wf,
            "[workflow]\nname = \"n\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"r\"\noutput = [\"o.txt\"]\nshell = \"echo hi > {output}\"\n",
        )
        .unwrap();
        let meta = meta(wf.to_str().unwrap(), wf.to_str().unwrap());
        assert!(meta.get("git_sha").is_none());
        assert!(meta.get("git_remote").is_none());
        assert!(meta.get("git_describe").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn derive_meta_from_gallery_05() {
        let meta = meta(
            "05_conda_environments.oxoflow",
            "../../examples/gallery/05_conda_environments.oxoflow",
        );

        assert_eq!(meta["name"], "environment-showcase");
        assert_eq!(meta["version"], "1.0.0");
        assert_eq!(meta["rule_count"], 5);
        assert_eq!(meta["tools"], json!(["analysis", "bwa-mem2", "qc"]));
        assert_eq!(meta["resources"]["max_threads"], 8);
        assert_eq!(meta["resources"]["max_memory"], "16G");
        assert_eq!(
            meta["environments"],
            json!({"conda": 2, "docker": 1, "system": 1, "venv": 1})
        );
        assert_eq!(meta["config_keys"], json!([]));
        assert_eq!(meta["sample_groups"], json!([]));
        assert_eq!(meta["pairs"], json!([]));
        assert_eq!(meta["references"], json!([]));
        assert_eq!(
            meta["input_dirs"],
            json!(["aligned", "data", "qc", "results"])
        );
        assert_eq!(
            meta["output_dirs"],
            json!(["aligned", "data", "qc", "results"])
        );
    }

    #[test]
    fn derive_meta_from_gallery_13() {
        let meta = meta(
            "13_simple_variant_calling.oxoflow",
            "../../examples/gallery/13_simple_variant_calling.oxoflow",
        );

        assert_eq!(meta["name"], "simple-variant-calling");
        // singularity images count as container tools ("docker://" stripped).
        assert_eq!(meta["tools"], json!(["alignment", "fastp", "gatk", "qc"]));
        // Engine-injected keys (samples_list, pairs_list, samples_*) excluded.
        assert_eq!(meta["config_keys"], json!(["known_sites", "reference"]));
        assert_eq!(meta["sample_groups"][0]["name"], "samples");
        assert_eq!(
            meta["sample_groups"][0]["samples"],
            json!(["NA12878", "NA12891", "NA12892"])
        );
        assert_eq!(meta["pairs"], json!([]));
        assert_eq!(meta["references"], json!([]));
    }

    #[test]
    fn derive_meta_config_details() {
        let meta = meta(
            "13_simple_variant_calling.oxoflow",
            "../../examples/gallery/13_simple_variant_calling.oxoflow",
        );

        assert_eq!(
            meta["config"],
            json!([
                {
                    "key": "known_sites",
                    "default": "/data/reference/dbsnp_146.hg38.vcf.gz",
                    "value_type": "string",
                    "used_by": ["base_recalibrator"],
                },
                {
                    "key": "reference",
                    "default": "/data/reference/GRCh38.fa",
                    "value_type": "string",
                    "used_by": [
                        "apply_bqsr",
                        "base_recalibrator",
                        "bwa_align",
                        "haplotype_caller"
                    ],
                },
            ])
        );
    }

    #[test]
    fn derive_meta_excludes_engine_injected_reference_keys() {
        // config_keys / config report only user-declared parameters: the
        // reference keyed-config value (config.mini_index) and the
        // reference_dir-derived paths are engine injections, not catalog
        // parameters the author wrote.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refs.oxoflow");
        std::fs::write(
            &path,
            r#"
            [workflow]
            name = "t"
            version = "1.0"

            [config]
            reference_dir = "refs"
            min_cov = 30

            [[references]]
            name = "mini_index"
            source = "refs/genome.fa"
            output = "refs/genome.fa.idx"
            build = "touch refs/genome.fa.idx"

            [[rules]]
            name = "step1"
            output = ["out.txt"]
            shell = "echo {config.min_cov}"
            "#,
        )
        .unwrap();
        let cfg = WorkflowConfig::from_file(&path).unwrap();
        let meta = derive_meta(&path, &cfg, &std::collections::BTreeMap::new());
        let keys = meta["config_keys"].as_array().unwrap();
        assert_eq!(keys.len(), 2, "{keys:?}");
        let names: Vec<&str> = keys.iter().filter_map(|k| k.as_str()).collect();
        assert!(names.contains(&"reference_dir"));
        assert!(names.contains(&"min_cov"));
        assert!(
            !names.contains(&"mini_index") && !names.contains(&"reference_fasta"),
            "engine-injected reference keys must not appear: {names:?}"
        );
    }

    #[test]
    fn derive_meta_resources_reflect_workflow_defaults() {
        // resources must report the same value the engine uses at run
        // time: [defaults] applies before scheduling, so max_threads /
        // max_memory reflect it (not the serde unset sentinel of 1/null).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("defaults.oxoflow");
        std::fs::write(
            &path,
            r#"
            [workflow]
            name = "t"
            version = "1.0"

            [defaults]
            threads = 4
            memory = "8G"

            [[rules]]
            name = "step1"
            output = ["out.txt"]
            shell = "echo hi"
            "#,
        )
        .unwrap();
        let cfg = WorkflowConfig::from_file(&path).unwrap();
        let meta = derive_meta(&path, &cfg, &std::collections::BTreeMap::new());
        assert_eq!(meta["resources"]["max_threads"], 4);
        assert_eq!(meta["resources"]["max_memory"], "8G");
    }

    #[test]
    fn derive_meta_config_empty_when_no_keys() {
        let meta = meta(
            "05_conda_environments.oxoflow",
            "../../examples/gallery/05_conda_environments.oxoflow",
        );

        assert_eq!(meta["config"], json!([]));
    }

    #[test]
    fn derive_meta_config_description_from_comments() {
        let meta = meta(
            "16_16s_qiime2_amplicon.oxoflow",
            "../../examples/gallery/16_16s_qiime2_amplicon.oxoflow",
        );

        // `classifier` carries a 4-line comment block (joined with spaces);
        // uncommented keys omit the field entirely.
        let records = meta["config"].as_array().unwrap();
        let classifier = records
            .iter()
            .find(|record| record["key"] == "classifier")
            .unwrap();
        assert_eq!(
            classifier["description"],
            "Optional: pre-trained classifier for the target 16S region \
             (e.g. silva-138-99-515-806-nb-classifier.qza). Set via \
             `oxo-flow run wf.oxoflow classifier=/path/to/classifier.qza`; \
             without it, skip the classify step or train a classifier first."
        );
        let trim_left_f = records
            .iter()
            .find(|record| record["key"] == "trim_left_f")
            .unwrap();
        assert!(trim_left_f.get("description").is_none());
    }

    #[test]
    fn derive_meta_config_used_by_when_expressions() {
        let meta = meta(
            "11_conditional_workflow.oxoflow",
            "../../examples/gallery/11_conditional_workflow.oxoflow",
        );

        // `when` conditions use the brace-less `config.<key>` form and must
        // count as usage alongside `{config.<key>}` in shells and I/O paths.
        assert_eq!(
            meta["config"],
            json!([
                {
                    "key": "min_coverage",
                    "default": 30,
                    "value_type": "int",
                    "used_by": ["vep_annotate"],
                },
                {
                    "key": "reference",
                    "default": "/ref/hg38.fa",
                    "value_type": "string",
                    "used_by": ["align", "haplotype_caller"],
                },
                {
                    "key": "run_annotation",
                    "default": true,
                    "value_type": "bool",
                    "used_by": ["report", "vep_annotate"],
                },
                {
                    "key": "run_qc",
                    "default": true,
                    "value_type": "bool",
                    "used_by": ["fastqc", "report"],
                },
                {
                    "key": "sequencing_mode",
                    "default": "WGS",
                    "value_type": "string",
                    "used_by": ["wes_coverage", "wgs_coverage"],
                },
                {
                    "key": "target_bed",
                    "default": "",
                    "value_type": "string",
                    "used_by": ["wes_coverage"],
                },
            ])
        );
    }

    #[test]
    fn derive_meta_config_used_by_expand_inputs_and_groups() {
        // {config.<key>} inside expand_inputs patterns / variables and
        // input_groups patterns must count as usage too — community
        // workflows (rnaseq cat_reads) feed config keys exclusively
        // through these channels: reads_dir appears ONLY in the
        // input_groups pattern, parts_dir ONLY in the expand_inputs
        // pattern.
        let dir = std::env::temp_dir().join(format!("oxo-info-expand-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wf = dir.join("wf.oxoflow");
        std::fs::write(
            &wf,
            r#"
[workflow]
name = "n"
version = "1.0.0"

[config]
reads_dir = "raw"
parts_dir = "parts"
unused_key = "x"
[[rules]]
name = "cat_reads"
input_groups = [
    { pattern = "{config.reads_dir}/{sample}_R{read}.fastq.gz", group_by = "sample" },
]
output = ["merged/{sample}.fastq.gz"]
shell = "cat {input} > {output[0]}"

[[rules]]
name = "aggregate"
input = []
expand_inputs = [
    { pattern = "{config.parts_dir}/*.txt", variables = {} },
]
output = ["all.txt"]
shell = "cat {input} > {output[0]}"

[[rules]]
name = "unused_user"
input = []
output = ["u.txt"]
shell = "echo done > {output[0]}"
"#,
        )
        .unwrap();
        let meta = meta(wf.to_str().unwrap(), wf.to_str().unwrap());
        let records = meta["config"].as_array().unwrap();
        let used_by = |key: &str| -> Vec<String> {
            records
                .iter()
                .find(|record| record["key"] == key)
                .unwrap_or_else(|| panic!("{key} must appear in config records"))["used_by"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(
            used_by("reads_dir"),
            vec!["cat_reads"],
            "input_groups pattern is a usage site"
        );
        assert_eq!(
            used_by("parts_dir"),
            vec!["aggregate"],
            "expand_inputs pattern is a usage site"
        );
        assert_eq!(used_by("unused_key"), Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
