//! Deep pipeline health checks for `oxo-flow test --deep` (issue #64).
//!
//! `compute_deep_check` walks a raw workflow config and answers the
//! "has this pipeline rotted?" questions that validate/lint/dry-run do not:
//!
//! - **D001 (error)** — script files referenced by rules exist on disk
//!   (`script =` first token, interpreter invocations in `shell`, and
//!   explicit-path first commands). A missing file fails deterministically
//!   at run time, so it is an error.
//! - **D002 (warning)** — environment definition files exist (conda/mamba
//!   YAML, venv dirs, requirements files, `pixi.toml`).
//! - **D003 (warning)** — binaries referenced by system-backend rules are in
//!   PATH. PATH is machine-specific (HPC module systems, containers), so a
//!   missing binary is a warning, not an error.
//! - **D004 (warning)** — reference data exists: path-like `[config]` values
//!   used in commands, `reference_dir`-derived tool indexes, and
//!   `[[references]]` outputs. Data can arrive later (issue #63), so a
//!   missing path is a warning.
//!
//! All relative paths resolve against `base_dir` — the directory the
//! executor runs rules from (the workflow file's directory, or `--workdir`).
//! Checks run on the **raw** (unexpanded) config, so `{sample}` placeholders
//! remain: any path still containing `{` or `*` is skipped, mirroring
//! [`crate::readiness::compute_readiness`]. Shell parsing is deliberately
//! conservative: only the first command of each line is probed for binaries,
//! and only plain interpreter invocations (`Rscript scripts/x.R`) are scanned
//! for script paths — paths embedded in R/Python expressions are not parsed.

use crate::config::WorkflowConfig;
use crate::format::Severity;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A single deep-check finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepFinding {
    /// Severity of the issue (D001 errors, the rest warnings).
    pub severity: Severity,
    /// Diagnostic code for programmatic handling ("D001".."D004").
    pub code: String,
    /// Human-readable message.
    pub message: String,
    /// Rule the finding relates to, when there is one.
    pub rule: Option<String>,
    /// Optional suggestion for how to fix the issue.
    pub suggestion: Option<String>,
    /// The path that was checked.
    pub path: String,
}

/// Deep-check report for a workflow.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeepCheckReport {
    /// Findings, in workflow order, deduplicated by `(code, path)`.
    pub findings: Vec<DeepFinding>,
    /// Number of error-severity findings (D001).
    pub error_count: usize,
    /// Number of warning-severity findings (D002/D003/D004).
    pub warning_count: usize,
    /// Whether the workflow passed (no errors).
    pub passed: bool,
    /// Script-file candidates evaluated (existing or missing).
    pub scripts_checked: usize,
    /// Environment definition candidates evaluated.
    pub envs_checked: usize,
    /// System-backend commands probed in PATH.
    pub commands_probed: usize,
    /// Reference paths evaluated.
    pub references_checked: usize,
}

/// Interpreters that announce a script path as their next plain argument.
const INTERPRETERS: &[&str] = &[
    "python",
    "python3",
    "Rscript",
    "R",
    "quarto",
    "jupyter",
    "snakemake",
    "perl",
    "ruby",
    "bash",
    "sh",
    "zsh",
    "julia",
    "nextflow",
    "miniwdl",
];

/// Interpreter flags whose following argument is inline code, not a path.
const CODE_FLAGS: &[&str] = &["-e", "-c", "--eval", "--execute", "--expression"];

/// Subcommands that sit between an interpreter and its script path.
const SUBCOMMANDS: &[&str] = &["render", "nbconvert", "run"];

/// Script file extensions recognised by the executor's interpreter map.
const SCRIPT_EXTENSIONS: &[&str] = &[
    ".py",
    ".py3",
    ".R",
    ".r",
    ".Rmd",
    ".rmd",
    ".qmd",
    ".sh",
    ".bash",
    ".pl",
    ".rb",
    ".jl",
    ".ipynb",
    ".smk",
    ".nextflow",
    ".wdl",
];

/// Characters that disqualify a token from being a plain script path
/// (quotes, parens, substitutions — the token is an expression, not a path).
const GUARD_CHARS: &[char] = &['(', ')', '\'', '"', '=', '$', '?', ';', '`'];

/// Shell builtins and coreutils that are never worth probing in PATH.
const SHELL_BUILTINS: &[&str] = &[
    "echo", "cd", "mkdir", "ls", "cat", "rm", "mv", "cp", "grep", "sed", "awk", "sort", "wc",
    "head", "tail", "cut", "paste", "find", "xargs", "chmod", "ln", "touch", "sleep", "date",
    "true", "false", "test", "export", "unset", "source", "env", "basename", "dirname", "tr",
    "uniq", "printf", "tee", "which", "pwd", "umask", "nohup", "time", "command",
];

/// Tool token → derived reference key under `reference_dir`.
const TOOL_INDEX_KEYS: &[(&str, &str)] = &[
    ("bwa-mem2", "bwamem2_index"),
    ("bwamem2", "bwamem2_index"),
    ("bwa", "bwa_index"),
    ("bowtie2", "bowtie2_index"),
    ("star", "star_index"),
    ("STAR", "star_index"),
    ("hisat2", "hisat2_index"),
    ("minimap2", "minimap2_index"),
    ("gatk", "gatk_dict"),
];

/// Compute deep checks on a raw (unexpanded) workflow config.
pub fn compute_deep_check(config: &WorkflowConfig, base_dir: &Path) -> DeepCheckReport {
    let vars = config_vars(config);
    let mut report = DeepCheckReport::default();
    // Deduplicate by (code, path): a shared script referenced by five rules
    // is one problem, not five.
    let mut seen: HashSet<(String, String)> = HashSet::new();

    for rule in &config.rules {
        let env = config.resolve_environment(rule);
        let env_kind = env
            .as_ref()
            .map(|e| e.kind().to_string())
            .unwrap_or_else(|| "system".to_string());

        // 1. Script files (D001, error).
        let mut script_candidates: Vec<String> = Vec::new();
        if let Some(script) = rule.script.as_deref() {
            let expanded = expand_config(script, &vars);
            let first = expanded.split_whitespace().next().unwrap_or("");
            // `script = "python scripts/x.py"` is legal (executed as a
            // command); only a non-interpreter first token is a bare path.
            if is_resolvable_path(first) && !INTERPRETERS.contains(&first) {
                script_candidates.push(first.to_string());
            }
            script_candidates.extend(interpreter_script_candidates(&expanded));
        }
        if let Some(shell) = rule.shell.as_deref() {
            script_candidates.extend(interpreter_script_candidates(&expand_config(shell, &vars)));
        }
        for path in script_candidates {
            check_script_path(&mut report, &mut seen, base_dir, &path, &rule.name);
        }

        // 2. Environment definition files (D002, warning).
        if let Some(env) = &env {
            for (path, what) in env_file_candidates(env) {
                if !seen.insert(("D002".to_string(), path.clone())) {
                    continue;
                }
                report.envs_checked += 1;
                if !resolve_path(&path, base_dir).exists() {
                    report.findings.push(DeepFinding {
                        severity: Severity::Warning,
                        code: "D002".to_string(),
                        message: format!("{what} not found: {path}"),
                        rule: Some(rule.name.clone()),
                        suggestion: Some(
                            "ship the environment definition with the workflow repository"
                                .to_string(),
                        ),
                        path,
                    });
                    report.warning_count += 1;
                }
            }
        }

        // 3. System-backend binaries (D003, warning) and explicit-path first
        //    commands (D001). Only the first command of each line is probed —
        //    no `&&`/`|`/`;` pipeline parsing.
        if env_kind == "system"
            && let Some(shell) = rule.shell.as_deref()
        {
            for token in first_command_tokens(&expand_config(shell, &vars)) {
                if !is_resolvable_path(&token)
                    || token.contains('&')
                    || token.contains('|')
                    || SHELL_BUILTINS.contains(&token.as_str())
                {
                    continue;
                }
                if token.contains('/') {
                    // Explicit path: a deterministic failure, like a script.
                    check_script_path(&mut report, &mut seen, base_dir, &token, &rule.name);
                    continue;
                }
                report.commands_probed += 1;
                let path_dirs: Vec<PathBuf> =
                    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
                if find_in_path(&token, path_dirs.iter().map(PathBuf::as_path)).is_none()
                    && seen.insert(("D003".to_string(), token.clone()))
                {
                    report.findings.push(DeepFinding {
                        severity: Severity::Warning,
                        code: "D003".to_string(),
                        message: format!("binary not found in PATH: {token}"),
                        rule: Some(rule.name.clone()),
                        suggestion: Some(
                            "install the tool or load it via the environment/module system"
                                .to_string(),
                        ),
                        path: token.clone(),
                    });
                    report.warning_count += 1;
                }
            }
        }

        // 4. Reference data (D004, warning).
        // (a) Path-like config values referenced by this rule's commands.
        for (key, value) in &vars {
            let placeholder = format!("{{{key}}}");
            let referenced = rule
                .shell
                .as_deref()
                .is_some_and(|s| s.contains(&placeholder))
                || rule
                    .script
                    .as_deref()
                    .is_some_and(|s| s.contains(&placeholder));
            if referenced && looks_like_path_value(value) {
                check_reference_path(&mut report, &mut seen, base_dir, value, Some(&rule.name));
            }
        }
    }

    // (b) `reference_dir`-derived tool indexes for tools mentioned in shells.
    let derived = config.derive_reference_paths();
    if !derived.is_empty() {
        let all_tokens: HashSet<&str> = config
            .rules
            .iter()
            .filter_map(|r| r.shell.as_deref())
            .flat_map(str::split_whitespace)
            .collect();
        for (tool, key) in TOOL_INDEX_KEYS {
            if all_tokens.contains(tool)
                && let Some(path) = derived.get(*key)
            {
                check_reference_path(&mut report, &mut seen, base_dir, path, None);
            }
        }
        if all_tokens.contains("samtools")
            && all_tokens.contains("faidx")
            && let Some(path) = derived.get("samtools_faidx")
        {
            check_reference_path(&mut report, &mut seen, base_dir, path, None);
        }
    }

    // (c) `[[references]]` build outputs.
    for reference in &config.references {
        let output = expand_config(&reference.output, &vars);
        if is_resolvable_path(&output) {
            check_reference_path(&mut report, &mut seen, base_dir, &output, None);
        }
    }

    report.passed = report.error_count == 0;
    report
}

/// Check one script-file candidate: missing → D001 error.
fn check_script_path(
    report: &mut DeepCheckReport,
    seen: &mut HashSet<(String, String)>,
    base_dir: &Path,
    path: &str,
    rule_name: &str,
) {
    if !seen.insert(("D001".to_string(), path.to_string())) {
        return;
    }
    report.scripts_checked += 1;
    if !resolve_path(path, base_dir).exists() {
        report.findings.push(DeepFinding {
            severity: Severity::Error,
            code: "D001".to_string(),
            message: format!("script file not found: {path}"),
            rule: Some(rule_name.to_string()),
            suggestion: Some("add the file to the repository or fix the path".to_string()),
            path: path.to_string(),
        });
        report.error_count += 1;
    }
}

/// Check one reference-data path: missing → D004 warning.
fn check_reference_path(
    report: &mut DeepCheckReport,
    seen: &mut HashSet<(String, String)>,
    base_dir: &Path,
    path: &str,
    rule_name: Option<&str>,
) {
    if !seen.insert(("D004".to_string(), path.to_string())) {
        return;
    }
    report.references_checked += 1;
    if !resolve_path(path, base_dir).exists() {
        report.findings.push(DeepFinding {
            severity: Severity::Warning,
            code: "D004".to_string(),
            message: format!("reference path not found: {path}"),
            rule: rule_name.map(str::to_string),
            suggestion: Some(
                "make the reference data available before running the workflow".to_string(),
            ),
            path: path.to_string(),
        });
        report.warning_count += 1;
    }
}

/// Environment definition files a rule's effective environment depends on,
/// as `(path, human-readable what)` pairs. Container images, module names,
/// and project-local prefixes are not files in the repository, so they are
/// skipped.
fn env_file_candidates(env: &crate::rule::EnvironmentSpec) -> Vec<(String, String)> {
    let mut candidates = Vec::new();
    for (spec, what) in [
        (&env.conda, "conda environment YAML"),
        (&env.mamba, "mamba environment YAML"),
    ] {
        // Bare env names are legal (`conda_env_name_from_spec` falls back to
        // the file stem); only file-like specs are repository files.
        if let Some(value) = spec
            && looks_file_like(value)
        {
            candidates.push((value.clone(), what.to_string()));
        }
    }
    if let Some(value) = &env.venv {
        candidates.push((value.clone(), "venv directory".to_string()));
    }
    if let Some(value) = &env.venv_requirements {
        candidates.push((value.clone(), "venv requirements file".to_string()));
    }
    if env.pixi.is_some() {
        candidates.push(("pixi.toml".to_string(), "pixi project file".to_string()));
    }
    candidates
}

/// True when a conda/mamba spec looks like a repository file (has a
/// directory component or a YAML extension) rather than a bare env name.
fn looks_file_like(value: &str) -> bool {
    value.contains('/') || value.ends_with(".yaml") || value.ends_with(".yml")
}

/// Map every `[config]` value to the `config.<key>` form the executor
/// resolves in commands (`{config.data_dir}` → value).
fn config_vars(config: &WorkflowConfig) -> HashMap<String, String> {
    config
        .config
        .iter()
        .map(|(key, value)| {
            let string_val = match value {
                toml::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (format!("config.{key}"), string_val)
        })
        .collect()
}

/// Expand `{config.x}` placeholders (simple substitution only, mirroring
/// the executor's config expansion in paths).
fn expand_config(cmd: &str, vars: &HashMap<String, String>) -> String {
    crate::executor::checkpoint::expand_config_in_path(cmd, vars)
}

/// True when a token is a concrete, checkable path (no unresolved
/// placeholders or globs left).
fn is_resolvable_path(token: &str) -> bool {
    !token.is_empty() && !token.contains('{') && !token.contains('*')
}

/// True when a config value looks like a filesystem path rather than a
/// scalar setting or a URL.
fn looks_like_path_value(value: &str) -> bool {
    value.contains('/')
        && !value.starts_with("http://")
        && !value.starts_with("https://")
        && !value.contains('{')
        && !value.contains('*')
}

/// Scan a command for plain interpreter invocations (`Rscript scripts/x.R`)
/// and return the script-path candidates. Inline-code arguments (`-e "...")`
/// and flags are skipped.
fn interpreter_script_candidates(cmd: &str) -> Vec<String> {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let mut candidates = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if !INTERPRETERS.contains(&tokens[i]) {
            i += 1;
            continue;
        }
        i += 1;
        while i < tokens.len()
            && (CODE_FLAGS.contains(&tokens[i])
                || SUBCOMMANDS.contains(&tokens[i])
                || tokens[i].starts_with('-'))
        {
            i += 1;
        }
        if i < tokens.len() && is_script_candidate(tokens[i]) {
            candidates.push(tokens[i].to_string());
        }
        i += 1;
    }
    candidates
}

/// True when a token can be a plain script path: a directory component or a
/// script extension, and none of the expression-marker guard characters.
fn is_script_candidate(token: &str) -> bool {
    if !is_resolvable_path(token) || GUARD_CHARS.iter().any(|c| token.contains(*c)) {
        return false;
    }
    token.contains('/') || SCRIPT_EXTENSIONS.iter().any(|ext| token.ends_with(ext))
}

/// First whitespace token of each non-empty line — the command that line
/// executes.
fn first_command_tokens(cmd: &str) -> Vec<String> {
    cmd.lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

/// Join a possibly-relative path onto `base_dir` (absolute paths untouched).
fn resolve_path(path: &str, base_dir: &Path) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

/// Search for an executable named `name` in the given directories.
///
/// Requires the executable bit (on Unix); takes an explicit directory
/// iterator so tests can probe a tempdir without touching the environment.
pub(crate) fn find_in_path<'a>(
    name: &str,
    dirs: impl Iterator<Item = &'a Path>,
) -> Option<PathBuf> {
    for dir in dirs {
        let exe = dir.join(name);
        if !exe.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let Ok(metadata) = exe.metadata() else {
                continue;
            };
            if metadata.permissions().mode() & 0o111 == 0 {
                continue;
            }
        }
        return Some(exe);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkflowConfig;
    use crate::format::Severity;
    use std::path::Path;

    /// Parse a TOML workflow and run deep checks against `dir` (raw config,
    /// mirroring the CLI's `WorkflowConfig::from_file`).
    fn deep_for(toml: &str, dir: &Path) -> DeepCheckReport {
        let config = WorkflowConfig::parse(toml).expect("parse workflow");
        compute_deep_check(&config, dir)
    }

    /// Minimal valid workflow header around a rules body.
    fn wf(rules: &str) -> String {
        format!(
            "[workflow]\nname = \"deep\"\nversion = \"1.0.0\"\n\
             description = \"deep check fixture\"\nauthor = \"tests\"\n\n{rules}"
        )
    }

    fn findings_of<'a>(report: &'a DeepCheckReport, code: &str) -> Vec<&'a DeepFinding> {
        report.findings.iter().filter(|f| f.code == code).collect()
    }

    // ── D001: script file existence ────────────────────────────────────────

    #[test]
    fn script_field_missing_file_reports_d001_error() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf(
            "[[rules]]\nname = \"analyze\"\noutput = [\"results/a.txt\"]\n\
             description = \"run script\"\nscript = \"scripts/analyze.py --out results/a.txt\"\n",
        );
        let report = deep_for(&toml, dir.path());
        assert_eq!(findings_of(&report, "D001").len(), 1);
        let f = &findings_of(&report, "D001")[0];
        assert_eq!(f.severity, Severity::Error);
        assert_eq!(f.rule.as_deref(), Some("analyze"));
        assert!(
            f.path.ends_with("scripts/analyze.py"),
            "path was {}",
            f.path
        );
        assert_eq!(report.error_count, 1);
        assert!(!report.passed);
    }

    #[test]
    fn script_field_existing_file_is_clean() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("scripts")).unwrap();
        std::fs::write(dir.path().join("scripts/analyze.py"), b"x").unwrap();
        let toml = wf(
            "[[rules]]\nname = \"analyze\"\noutput = [\"results/a.txt\"]\n\
             description = \"run script\"\nscript = \"scripts/analyze.py --out results/a.txt\"\n",
        );
        let report = deep_for(&toml, dir.path());
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert_eq!(report.error_count, 0);
        assert!(report.passed);
        assert_eq!(report.scripts_checked, 1);
    }

    #[test]
    fn script_field_templated_path_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf(
            "[[rules]]\nname = \"analyze\"\noutput = [\"results/a.txt\"]\n\
             description = \"run script\"\nscript = \"scripts/{sample}.py --out results/a.txt\"\n",
        );
        let report = deep_for(&toml, dir.path());
        assert!(report.findings.is_empty(), "{:?}", report.findings);
    }

    #[test]
    fn script_field_config_expansion_resolves_path() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf("[config]\nscripts_dir = \"scripts\"\n\n\
             [[rules]]\nname = \"analyze\"\noutput = [\"results/a.txt\"]\n\
             description = \"run script\"\nscript = \"{config.scripts_dir}/analyze.py\"\n");
        let report = deep_for(&toml, dir.path());
        let d001 = findings_of(&report, "D001");
        assert_eq!(d001.len(), 1);
        assert!(d001[0].path.ends_with("scripts/analyze.py"));
    }

    // ── D001 via shell interpreter invocations ─────────────────────────────

    #[test]
    fn shell_interpreter_plain_arg_reports_d001() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf(
            "[[rules]]\nname = \"cluster\"\noutput = [\"results/c.txt\"]\n\
             description = \"run R script\"\n\
             shell = \"Rscript scripts/seurat_analysis.R --out results/c.txt\"\n",
        );
        let report = deep_for(&toml, dir.path());
        let d001 = findings_of(&report, "D001");
        assert_eq!(d001.len(), 1);
        assert!(d001[0].path.ends_with("scripts/seurat_analysis.R"));
    }

    #[test]
    fn shell_r_expression_string_is_not_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf(
            "[[rules]]\nname = \"report\"\noutput = [\"results/r.html\"]\n\
             description = \"render report\"\n\
             shell = \"Rscript -e \\\"rmarkdown::render('templates/sc_report.Rmd')\\\"\"\n",
        );
        let report = deep_for(&toml, dir.path());
        assert!(
            findings_of(&report, "D001").is_empty(),
            "{:?}",
            findings_of(&report, "D001")
        );
    }

    #[test]
    fn shell_code_flag_import_expression_is_not_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf("[[rules]]\nname = \"py\"\noutput = [\"results/p.txt\"]\n\
             description = \"run python\"\n\
             shell = \"python -c \\\"import scripts.util; print(1)\\\"\"\n");
        let report = deep_for(&toml, dir.path());
        assert!(
            findings_of(&report, "D001").is_empty(),
            "{:?}",
            findings_of(&report, "D001")
        );
    }

    #[test]
    fn shell_module_invocation_is_not_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf("[[rules]]\nname = \"py\"\noutput = [\"results/p.txt\"]\n\
             description = \"run python module\"\n\
             shell = \"python -m pip install --upgrade pip\"\n");
        let report = deep_for(&toml, dir.path());
        assert!(
            findings_of(&report, "D001").is_empty(),
            "{:?}",
            findings_of(&report, "D001")
        );
    }

    #[test]
    fn explicit_path_first_token_missing_reports_d001() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf("[[rules]]\nname = \"run\"\noutput = [\"results/x.txt\"]\n\
             description = \"run local script\"\n\
             shell = \"./scripts/run.sh --out results/x.txt\"\n");
        let report = deep_for(&toml, dir.path());
        let d001 = findings_of(&report, "D001");
        assert_eq!(d001.len(), 1);
        assert!(d001[0].path.ends_with("./scripts/run.sh"));
    }

    // ── D003: system-backend binaries in PATH ──────────────────────────────

    #[test]
    fn missing_binary_reports_d003_warning() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf("[[rules]]\nname = \"qc\"\noutput = [\"results/q.txt\"]\n\
             description = \"run fake tool\"\n\
             shell = \"fake_tool_for_deep_check_9f3k --in data.fq\"\n");
        let report = deep_for(&toml, dir.path());
        let d003 = findings_of(&report, "D003");
        assert_eq!(d003.len(), 1);
        assert_eq!(d003[0].severity, Severity::Warning);
        assert_eq!(report.error_count, 0);
        assert!(report.passed);
        assert_eq!(report.commands_probed, 1);
    }

    #[test]
    fn present_binary_is_clean() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf("[[rules]]\nname = \"sh\"\noutput = [\"results/s.txt\"]\n\
             description = \"run shell builtin\"\n\
             shell = \"sh -c 'echo hi > results/s.txt'\"\n");
        let report = deep_for(&toml, dir.path());
        assert!(findings_of(&report, "D003").is_empty());
        assert_eq!(report.commands_probed, 1);
    }

    #[test]
    fn denylisted_commands_are_not_probed() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf("[[rules]]\nname = \"prep\"\noutput = [\"results/p.txt\"]\n\
             description = \"prepare dirs\"\n\
             shell = \"mkdir -p results && echo hi > results/p.txt\"\n");
        let report = deep_for(&toml, dir.path());
        assert!(findings_of(&report, "D003").is_empty());
        assert_eq!(report.commands_probed, 0);
    }

    #[test]
    fn conda_backend_gates_binary_probe() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf("[[rules]]\nname = \"qc\"\noutput = [\"results/q.txt\"]\n\
             description = \"run tool in conda env\"\n\
             shell = \"fake_tool_for_deep_check_9f3k --in data.fq\"\n\
             [rules.environment]\nconda = \"envs/qc.yaml\"\n");
        let report = deep_for(&toml, dir.path());
        assert!(findings_of(&report, "D003").is_empty());
        assert_eq!(report.commands_probed, 0);
        // The env yaml itself is still checked.
        assert_eq!(findings_of(&report, "D002").len(), 1);
    }

    // ── D002: environment definition files ─────────────────────────────────

    #[test]
    fn missing_conda_yaml_reports_d002_warning() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf("[[rules]]\nname = \"qc\"\noutput = [\"results/q.txt\"]\n\
             description = \"qc step\"\nshell = \"cat data.fq > results/q.txt\"\n\
             [rules.environment]\nconda = \"envs/qc.yaml\"\n");
        let report = deep_for(&toml, dir.path());
        let d002 = findings_of(&report, "D002");
        assert_eq!(d002.len(), 1);
        assert_eq!(d002[0].severity, Severity::Warning);
        assert!(d002[0].path.ends_with("envs/qc.yaml"));
        assert_eq!(report.envs_checked, 1);
    }

    #[test]
    fn bare_conda_env_name_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf("[[rules]]\nname = \"qc\"\noutput = [\"results/q.txt\"]\n\
             description = \"qc step\"\nshell = \"cat data.fq > results/q.txt\"\n\
             [rules.environment]\nconda = \"myenv\"\n");
        let report = deep_for(&toml, dir.path());
        assert!(findings_of(&report, "D002").is_empty());
    }

    #[test]
    fn missing_venv_dir_and_requirements_report_d002() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf("[[rules]]\nname = \"py\"\noutput = [\"results/p.txt\"]\n\
             description = \"python step\"\nshell = \"python -c 'print(1)'\"\n\
             [rules.environment]\nvenv = \"envs/venv\"\n\
             venv_requirements = \"envs/requirements.txt\"\n");
        let report = deep_for(&toml, dir.path());
        assert_eq!(findings_of(&report, "D002").len(), 2);
        assert_eq!(report.envs_checked, 2);
    }

    #[test]
    fn pixi_env_checks_base_dir_pixi_toml() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf("[[rules]]\nname = \"px\"\noutput = [\"results/p.txt\"]\n\
             description = \"pixi step\"\nshell = \"cat data.fq > results/p.txt\"\n\
             [rules.environment]\npixi = \"default\"\n");
        let report = deep_for(&toml, dir.path());
        let d002 = findings_of(&report, "D002");
        assert_eq!(d002.len(), 1);
        assert!(d002[0].path.ends_with("pixi.toml"));
    }

    #[test]
    fn docker_and_singularity_envs_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf("[[rules]]\nname = \"d\"\noutput = [\"results/d.txt\"]\n\
             description = \"docker step\"\nshell = \"cat data.fq > results/d.txt\"\n\
             [rules.environment]\ndocker = \"biocontainers/bwa:0.7.17\"\n\n\
             [[rules]]\nname = \"s\"\noutput = [\"results/s.txt\"]\n\
             description = \"singularity step\"\nshell = \"cat data.fq > results/s.txt\"\n\
             [rules.environment]\nsingularity = \"docker://biocontainers/bwa:0.7.17\"\n");
        let report = deep_for(&toml, dir.path());
        assert!(report.findings.is_empty(), "{:?}", report.findings);
    }

    #[test]
    fn env_group_resolution_applies_for_binary_gate() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf("[env_groups.qc_env]\nconda = \"envs/qc.yaml\"\n\n\
             [[rules]]\nname = \"qc\"\nenv_group = \"qc_env\"\n\
             output = [\"results/q.txt\"]\ndescription = \"qc step\"\n\
             shell = \"fake_tool_for_deep_check_9f3k --in data.fq\"\n");
        let report = deep_for(&toml, dir.path());
        assert!(findings_of(&report, "D003").is_empty());
        assert_eq!(findings_of(&report, "D002").len(), 1);
    }

    // ── D004: reference data readiness ─────────────────────────────────────

    #[test]
    fn missing_config_reference_path_reports_d004_warning() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf("[config]\nreference = \"/data/refs/GRCh38/genome.fa\"\n\n\
             [[rules]]\nname = \"align\"\noutput = [\"results/a.sam\"]\n\
             description = \"align reads\"\n\
             shell = \"bwa mem {config.reference} data.fq > results/a.sam\"\n");
        let report = deep_for(&toml, dir.path());
        let d004 = findings_of(&report, "D004");
        assert_eq!(d004.len(), 1);
        assert_eq!(d004[0].severity, Severity::Warning);
        assert_eq!(d004[0].path, "/data/refs/GRCh38/genome.fa");
        assert_eq!(report.references_checked, 1);
    }

    #[test]
    fn non_path_and_url_config_values_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf(
            "[config]\nthreads = \"8\"\nurl = \"https://example.com/data.fa\"\n\n\
             [[rules]]\nname = \"x\"\noutput = [\"results/x.txt\"]\n\
             description = \"use config\"\n\
             shell = \"cat data.fq > results/x.txt && echo {config.threads} {config.url}\"\n",
        );
        let report = deep_for(&toml, dir.path());
        assert!(findings_of(&report, "D004").is_empty());
    }

    #[test]
    fn derived_bwa_index_checked_when_reference_dir_set() {
        let dir = tempfile::tempdir().unwrap();
        // `reference_dir` is a top-level key: it must precede the [workflow]
        // table header, or TOML scopes it under [workflow] and it is dropped.
        let toml = format!(
            "reference_dir = \"/data/refs/GRCh38\"\n\n{}",
            wf(
                "[[rules]]\nname = \"align\"\noutput = [\"results/a.sam\"]\n\
                 description = \"align reads\"\n\
                 shell = \"bwa mem genome.fa data.fq > results/a.sam\"\n",
            )
        );
        let report = deep_for(&toml, dir.path());
        let d004 = findings_of(&report, "D004");
        assert_eq!(d004.len(), 1, "{:?}", d004);
        assert_eq!(d004[0].path, "/data/refs/GRCh38/bwa/genome.fa");
    }

    #[test]
    fn reference_block_output_checked() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf(
            "[[references]]\nname = \"faidx\"\nsource = \"/data/refs/genome.fa\"\n\
             output = \"/data/refs/genome.fa.fai\"\n\
             build = \"samtools faidx /data/refs/genome.fa\"\n\n\
             [[rules]]\nname = \"x\"\noutput = [\"results/x.txt\"]\n\
             description = \"step\"\nshell = \"cat data.fq > results/x.txt\"\n",
        );
        let report = deep_for(&toml, dir.path());
        let d004 = findings_of(&report, "D004");
        assert_eq!(d004.len(), 1);
        assert_eq!(d004[0].path, "/data/refs/genome.fa.fai");
    }

    // ── dedupe and accounting ──────────────────────────────────────────────

    #[test]
    fn same_missing_path_across_rules_reports_once() {
        let dir = tempfile::tempdir().unwrap();
        let toml = wf("[[rules]]\nname = \"a\"\noutput = [\"results/a.txt\"]\n\
             description = \"step a\"\nscript = \"scripts/shared.py\"\n\n\
             [[rules]]\nname = \"b\"\noutput = [\"results/b.txt\"]\n\
             description = \"step b\"\nscript = \"scripts/shared.py\"\n");
        let report = deep_for(&toml, dir.path());
        assert_eq!(findings_of(&report, "D001").len(), 1);
    }

    // ── find_in_path ───────────────────────────────────────────────────────

    #[cfg(unix)]
    #[test]
    fn find_in_path_finds_executable_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("mytool");
        std::fs::write(&tool, b"#!/bin/sh\n").unwrap();
        let mut perms = std::fs::metadata(&tool).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&tool, perms).unwrap();

        let found = find_in_path("mytool", std::iter::once(dir.path()));
        assert_eq!(found.as_deref(), Some(tool.as_path()));
    }

    #[cfg(unix)]
    #[test]
    fn find_in_path_skips_non_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notexec"), b"x").unwrap();

        assert!(find_in_path("notexec", std::iter::once(dir.path())).is_none());
    }

    #[test]
    fn find_in_path_returns_none_for_unknown_name() {
        let dir = tempfile::tempdir().unwrap();
        assert!(find_in_path("no_such_tool_xyz", std::iter::once(dir.path())).is_none());
    }
}
