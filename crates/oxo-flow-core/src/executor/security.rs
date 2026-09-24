use crate::error::{OxoFlowError, Result};
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// Logs the `OXO_FLOW_UNSAFE_WILDCARDS` relaxation exactly once per process
/// instead of once per rule execution.
static UNSAFE_WILDCARDS_WARNED: AtomicBool = AtomicBool::new(false);

/// Validate that an interpreter path is safe to use.
///
/// Prevents use of interpreters from untrusted locations. Only allows:
/// - Simple names (no path component): e.g., "python", "Rscript"
/// - Absolute paths in standard system directories: /usr/bin, /usr/local/bin, /opt
/// - Absolute paths in user directories: /home, /Users
///
/// Returns Ok(()) if safe, Err if potentially dangerous.
#[must_use = "interpreter path validation returns a Result that must be checked"]
pub fn validate_interpreter_path(interpreter: &str) -> Result<()> {
    // Simple names without path separators are always allowed
    if !interpreter.contains('/') && !interpreter.contains('\\') {
        return Ok(());
    }

    // Check for path traversal
    if interpreter.contains("..") {
        return Err(OxoFlowError::Validation {
            message: format!("Interpreter path '{}' contains path traversal", interpreter),
            rule: None,
            suggestion: Some("Avoid '..' in interpreter paths".to_string()),
        });
    }

    // For absolute paths, verify they're in safe directories
    if interpreter.starts_with('/') {
        let safe_prefixes = ["/usr/bin", "/usr/local/bin", "/opt", "/home", "/Users"];
        if !safe_prefixes.iter().any(|p| interpreter.starts_with(p)) {
            return Err(OxoFlowError::Validation {
                message: format!("Interpreter path '{}' not in safe directories", interpreter),
                rule: None,
                suggestion: Some(
                    "Use interpreters from standard paths (/usr/bin, /usr/local/bin, /opt, /home, /Users)".to_string(),
                ),
            });
        }
    }

    Ok(())
}

/// A category of dangerous shell patterns with associated regex patterns.
struct DangerCategory {
    /// Short identifier for the category (e.g., "RECURSIVE_DELETION").
    name: &'static str,
    /// Regex patterns that match commands in this category.
    patterns: &'static [&'static str],
    /// Human-readable description of the danger.
    description: &'static str,
}

/// All defined danger categories and their regex patterns.
static DANGER_CATEGORIES: &[DangerCategory] = &[
    DangerCategory {
        name: "RECURSIVE_DELETION",
        // Flag letters in ANY order (-rf/-fr/-r/-R/-rfv) plus the long
        // --recursive form: `rm -fr /` deletes recursively just the same,
        // so the category must fire for every spelling (issue #428 review).
        patterns: &[
            r"rm\s+-[a-zA-Z]*[rR][a-zA-Z]*\s+(?:--\S+\s+)*/",
            r"rm\s+-[a-zA-Z]*[rR][a-zA-Z]*\s+(?:--\S+\s+)*~",
            r"rm\s+--recursive(?:=\S*)?\s+(?:--\S+\s+)*/",
            r"rm\s+--recursive(?:=\S*)?\s+(?:--\S+\s+)*~",
        ],
        description: "dangerous recursive deletion",
    },
    DangerCategory {
        name: "FILESYSTEM_DESTRUCTION",
        patterns: &[r"mkfs\.?\w*", r"mkswap", r"dd\s+if=.*of=/dev/sd"],
        description: "filesystem destruction",
    },
    DangerCategory {
        name: "PERMISSION_ESCALATION",
        patterns: &[r"chmod\s+.*777\s+/", r"chmod\s+-R\s+777"],
        description: "overly permissive permission change",
    },
    DangerCategory {
        name: "BLOCK_DEVICE_WRITE",
        patterns: &[r">\s*/dev/sd[a-z]", r">>\s*/dev/sd[a-z]"],
        description: "direct block device write",
    },
    DangerCategory {
        name: "REMOTE_EXECUTION",
        patterns: &[
            r"(?:wget|curl).*\|\s*(?:sh|bash|dash)",
            r"(?:wget|curl).*\|\s*sudo",
        ],
        description: "remote code execution",
    },
    DangerCategory {
        name: "FORK_BOMB",
        patterns: &[r"\(\)\s*\{.*:.*\|.*&.*\}", r":\(\)\s*\{"],
        description: "fork bomb",
    },
    DangerCategory {
        name: "DATA_DESTRUCTION",
        patterns: &[r"dd\s+if=/dev/(?:zero|random|urandom)"],
        description: "data destruction via dd",
    },
];

/// Compiled regex patterns for blocking dangerous commands, paired with their
/// category name and human-readable description. Compiled once via [`LazyLock`]
/// for efficiency.
static COMPILED_BLOCK_PATTERNS: LazyLock<Vec<(Regex, &'static str, &'static str)>> =
    LazyLock::new(|| {
        let mut patterns = Vec::new();
        for category in DANGER_CATEGORIES {
            for pattern_str in category.patterns {
                if let Ok(re) = Regex::new(pattern_str) {
                    patterns.push((re, category.name, category.description));
                }
            }
        }
        patterns
    });

/// Compiled regex patterns for warning-level checks (non-blocking).
/// These detect suspicious behavior that may be legitimate in some contexts
/// (e.g., `$(command)` substitution in shell templates).
static WARNING_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let mut patterns = Vec::new();
    let warning_patterns: &[(&str, &str)] = &[
        (r"\$\([^)]*\)", "Command substitution detected"),
        (r"`[^`]*`", "Backtick command substitution detected"),
        (r">/dev/", "Redirect to /dev/ detected"),
        (r"rm\s+-rf\s+/", "Dangerous recursive deletion detected"),
        (r"chmod\s+777\b", "Overly permissive chmod detected"),
        (r"\beval\s+", "eval usage detected"),
        (
            r"(?:wget|curl).*?(?:\|\s*(?:sh|bash|dash|sudo)|&&\s*(?:bash|sh))",
            "Remote pipe to shell detected",
        ),
    ];
    for (pattern_str, desc) in warning_patterns {
        if let Ok(re) = Regex::new(pattern_str) {
            patterns.push((re, *desc));
        }
    }
    patterns
});

/// Check a shell command for potentially dangerous patterns.
///
/// Returns a list of warnings for suspicious patterns that could indicate
/// shell injection or destructive operations.  Common bioinformatics idioms
/// such as pipes (`|`), command chaining (`&&`), and semicolons (`;`) are
/// intentionally **not** flagged because they appear in virtually every
/// genomics shell template.
///
/// This function checks the *literal* command string after wildcard expansion.
/// Call it on the expanded shell command (post `render_shell_command`) to catch
/// any dangerous content injected via wildcard values.
///
/// This is a best-effort heuristic, not a security guarantee.
#[must_use]
pub fn sanitize_shell_command(cmd: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    for (re, description) in WARNING_PATTERNS.iter() {
        if re.is_match(cmd) {
            // Self-explanatory phrasing (issue #433): state what the pattern
            // means, that the run proceeds, and where the explanation lives —
            // a bare "Shell command warning" left operators guessing whether
            // the rule would even run.
            warnings.push(format!(
                "{} in '{}': this shell idiom is allowed and the rule will run; \
                 it is flagged because such content can also ride in on rendered \
                 wildcard values — verify it is intentional",
                description, cmd
            ));
        }
    }
    warnings
}

/// Block dangerous shell patterns that could lead to command injection.
/// Returns Ok(()) if safe, Err if dangerous patterns are detected.
///
/// Uses category-based regex matching against compiled patterns defined in
/// `DANGER_CATEGORIES` to detect destructive commands such as:
/// - Recursive deletion of root or home (`rm -rf /`, `rm -rf ~`)
/// - Filesystem destruction (`mkfs`, `mkswap`, `dd` to block devices)
/// - Permission escalation (`chmod 777 /`, `chmod -R 777`)
/// - Block device writes (`> /dev/sd*`, `>> /dev/sd*`)
/// - Remote code execution (pipe wget/curl to shell)
/// - Fork bombs
/// - Data destruction via `dd` from `/dev/zero`, `/dev/random`, `/dev/urandom`
///
/// Common bioinformatics shell idioms such as `$(command)`, backtick
/// substitution, pipes (`|`), and `&&` are intentionally **not** blocked
/// here because they appear in virtually every genomics shell template.
///
/// Shell templates in `.oxoflow` files are written by the pipeline author
/// and are trusted. To catch injection through wildcard values coming from
/// external sources (e.g., sample sheets), use
/// [`validate_wildcard_injection`] instead.
///
/// Note: &&, ||, and | are NOT blocked as they are common in
/// bioinformatics pipelines for error handling and streaming.
#[must_use = "shell safety validation returns a Result that must be checked"]
pub fn validate_shell_safety(cmd: &str) -> Result<()> {
    for (re, _name, description) in COMPILED_BLOCK_PATTERNS.iter() {
        if re.is_match(cmd) {
            return Err(OxoFlowError::Validation {
                message: format!(
                    "Shell command blocked: {} pattern detected in '{}'",
                    description, cmd
                ),
                rule: None,
                suggestion: Some(
                    "Remove dangerous shell constructs or use a script file instead".to_string(),
                ),
            });
        }
    }
    Ok(())
}

/// A single `rm -r[f]` deletion target parsed out of a rendered command.
struct DeletionTarget {
    /// The path operand as written (post-rendering).
    target: String,
}

/// Extract the path operands of `rm -r`/`rm -rf` invocations in `cmd`.
///
/// Mirrors the RECURSIVE_DELETION regexes' flexibility (extra spaces,
/// interleaved `--flags`, flag letters in any order) so a target the
/// pattern let through is analyzed here rather than silently skipped.
///
/// EVERY operand of an invocation is captured and validated — checking
/// only the first would let `rm -rf <workdir>/x /etc` sail through while
/// the real `rm` deletes both (issue #428 review finding).
///
/// Returns `Err` when a recursive-deletion invocation cannot be parsed
/// reliably (quoted operands or command substitution inside the operand
/// segment): the caller must treat that as BLOCKED, never as "nothing to
/// check". An empty `Ok` means no recursive `rm` invocation is present.
fn recursive_deletion_targets(cmd: &str) -> Result<Vec<DeletionTarget>> {
    // Flags group + operand segment. The segment stops at shell control
    // operators (&&, ||, ;, |, redirects, newline) so each `rm` invocation
    // in a pipeline is analyzed on its own.
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"rm\s+(-\S+(?:\s+-\S+)*)\s+([^;&|<>\n]+)").expect("static regex")
    });
    let mut targets = Vec::new();
    for caps in RE.captures_iter(cmd) {
        let flags: &str = caps.get(1).map_or("", |m| m.as_str());
        let is_recursive = flags.split_whitespace().any(|f| {
            let body = f.strip_prefix('-').unwrap_or(f);
            // `--recursive` (after one strip) or short flags whose letter
            // set contains r/R in ANY position: -rf, -fr, -r, -R, -rfv.
            body == "-recursive"
                || (body.chars().all(|c| c.is_ascii_alphabetic()) && body.contains(['r', 'R']))
        });
        if !is_recursive {
            continue;
        }
        let segment = caps.get(2).map_or("", |m| m.as_str());
        // Quoting or substitution inside the operand segment defeats
        // whitespace tokenization — fail closed instead of guessing.
        if segment.contains(['"', '\'', '`']) || segment.contains("$(") {
            return Err(OxoFlowError::Validation {
                message: format!(
                    "Shell command blocked: unparseable recursive deletion in '{}'",
                    cmd
                ),
                rule: None,
                suggestion: Some(
                    "Recursive deletions must be plainly spelled out; remove quotes or \
                     substitution from rm operands, or use a script file instead"
                        .to_string(),
                ),
            });
        }
        for operand in segment.split_whitespace() {
            if operand.starts_with('-') {
                continue; // trailing flags after the first operand
            }
            targets.push(DeletionTarget {
                target: operand.to_string(),
            });
        }
    }
    Ok(targets)
}

/// Classify an absolute deletion target against the run workdir.
///
/// Returns `true` when the target resolves INSIDE `workdir` (deletion of
/// the rule's own outputs is a legitimate cleanup idiom), `false` when it
/// points anywhere else. Unresolvable targets fail CLOSED (blocked).
fn deletion_target_in_workdir(target: &str, workdir: &Path) -> bool {
    // `~`/`~/...` expand to home; `~user` needs a passwd lookup we never
    // do here — fail closed rather than treating it as a relative path.
    if target.starts_with('~') && !(target == "~" || target.starts_with("~/")) {
        return false;
    }
    // Tilde paths expand to home — never inside the run workdir unless the
    // workdir itself lives under home AND the target descends into it; the
    // path-form resolution below decides that. `~` alone is home itself.
    let expanded = if target == "~" || target.starts_with("~/") {
        match std::env::var("HOME") {
            Ok(home) => target.replacen('~', &home, 1),
            Err(_) => return false,
        }
    } else {
        target.to_string()
    };

    let path = Path::new(&expanded);
    if !path.is_absolute() {
        // Relative targets resolve against the shell cwd (the workdir or a
        // scratch dir inside it), so they are inside by construction — and
        // the category regexes never matched them anyway.
        return true;
    }

    // Lexical containment first (no filesystem round-trip): covers paths
    // that don't exist yet, which is the common case for rule outputs.
    let canonical_workdir = workdir
        .canonicalize()
        .unwrap_or_else(|_| workdir.to_path_buf());
    if path.starts_with(&canonical_workdir) || path.starts_with(workdir) {
        // Reject a trailing `..` escape like `<workdir>/../elsewhere`.
        if expanded.contains("/../") || expanded.ends_with("/..") {
            return false;
        }
        return true;
    }

    // Fall back to filesystem resolution (symlinks, `.` components).
    match path.canonicalize() {
        Ok(canonical) => canonical.starts_with(&canonical_workdir),
        // Doesn't exist yet (typical for outputs of this very rule) and
        // lexically outside workdir — fail closed.
        Err(_) => false,
    }
}

/// Workdir-aware variant of [`validate_shell_safety`] (issue #428).
///
/// Identical to the base check EXCEPT for recursive deletions
/// (`rm -rf` / `rm -r`): a target that resolves INSIDE the run `workdir`
/// is allowed, because pipelines legitimately clean up their own outputs —
/// e.g. `rm -rf {config.out_dir}/{config.aligner}/stringtie/{sample}.ballgown`
/// with an absolute `config.out_dir` pointing under the workdir previously
/// rendered to `rm -rf /abs/...` and hard-failed mid-pipeline. Deletions
/// targeting root, home, or any path outside the workdir remain hard
/// errors, as does every other danger category.
///
/// The command must already be RENDERED (post wildcard/config expansion) —
/// callers pass the output of `render_shell_command`, so `{config.*}`
/// placeholders have become concrete paths.
#[must_use = "shell safety validation returns a Result that must be checked"]
pub fn validate_shell_safety_in_workdir(cmd: &str, workdir: &Path) -> Result<()> {
    // The extractor is authoritative: it finds EVERY rm invocation with
    // recursive flags (any spelling) and either validates all of its
    // operands against the workdir or fails closed on unparseable forms
    // (quotes, substitution). Commands with no recursive rm invocation
    // fall through to the base category checks unchanged.
    match recursive_deletion_targets(cmd) {
        Ok(targets) if targets.is_empty() => validate_shell_safety(cmd),
        Ok(targets) => {
            for target in targets {
                if !deletion_target_in_workdir(&target.target, workdir) {
                    return Err(OxoFlowError::Validation {
                        message: format!(
                            "Shell command blocked: dangerous recursive deletion pattern detected in '{}'",
                            cmd
                        ),
                        rule: None,
                        suggestion: Some(
                            "Recursive deletions must target paths inside the run workdir; \
                             remove dangerous shell constructs or use a script file instead"
                                .to_string(),
                        ),
                    });
                }
            }
            Ok(())
        }
        // Extraction doubles as a fail-closed gate: an rm invocation was
        // found but its operands cannot be parsed reliably — block.
        Err(e) => Err(e),
    }
}

/// Character class accepted for wildcard values when the workflow declares
/// no explicit `wildcard_constraints` entry (issue #203): letters, digits,
/// dot, underscore, dash, and path separator — the superset observed across
/// every shipped example. Anything else needs an explicit constraint.
pub const DEFAULT_WILDCARD_PATTERN: &str = r"^[A-Za-z0-9._/-]+$";

static DEFAULT_WILDCARD_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(DEFAULT_WILDCARD_PATTERN).expect("static regex"));

/// Validate wildcard VALUES before they are substituted into shell commands.
///
/// Two independent layers, per key:
///
/// 1. **Default charset (issue #203)** — values for wildcards WITHOUT an
///    explicit `wildcard_constraints` entry must match
///    [`DEFAULT_WILDCARD_PATTERN`], except that the empty string is always
///    accepted (issue #374: an empty value is a legitimate feature-off
///    sentinel such as `umi2_pattern = ""`). Pipelines that legitimately
///    need other characters declare a constraint for that wildcard (the
///    pre-existing mechanism), or set `OXO_FLOW_UNSAFE_WILDCARDS=1` to
///    relax the charset layer for the process — a one-time warning is
///    logged.
/// 2. **Substitution floor (always enforced, after the charset skip)** — no
///    `$(`, no backticks, for EVERY non-`config.*` value, including
///    constrained wildcards and unsafe-mode runs. The floor sits after the
///    constraint/unsafe-mode `continue` so a per-wildcard constraint can
///    widen the character set without re-enabling command substitution
///    (issue #276: the floor previously sat before the skip, so a
///    `sample = "^.+$"` constraint silently disabled it).
///
/// `config.*` keys come from the trusted .oxoflow file (and the operator's
/// own `--arg` overrides) and skip both layers, as before. Per-instance
/// wildcard values (sample names, `[[values]]` fan-out, group metadata —
/// anything a collaborator-supplied samplesheet or auto-discovered filename
/// can carry) MUST reach this function; the caller merges them alongside
/// the `config.*` map.
#[must_use = "wildcard injection validation returns a Result that must be checked"]
pub fn validate_wildcard_injection(
    wildcard_values: &HashMap<String, String>,
    declared_constraints: &HashMap<String, String>,
) -> Result<()> {
    let unsafe_mode = std::env::var("OXO_FLOW_UNSAFE_WILDCARDS").as_deref() == Ok("1");
    if unsafe_mode && !UNSAFE_WILDCARDS_WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            "OXO_FLOW_UNSAFE_WILDCARDS=1: wildcard value charset checks are relaxed \
             for this process; command-substitution values are still rejected"
        );
    }
    validate_wildcard_injection_inner(wildcard_values, declared_constraints, unsafe_mode)
}

fn validate_wildcard_injection_inner(
    wildcard_values: &HashMap<String, String>,
    declared_constraints: &HashMap<String, String>,
    unsafe_mode: bool,
) -> Result<()> {
    let injection_patterns = [
        ("$(", "command substitution"),
        ("`", "backtick substitution"),
    ];
    for (key, value) in wildcard_values {
        if key.starts_with("config.") {
            continue;
        }
        // Charset layer only applies to unconstrained wildcards.
        if declared_constraints.contains_key(key) || unsafe_mode {
            // The substitution floor is UNCONDITIONAL for non-config values
            // (docs: "rejected unconditionally — including for constrained
            // wildcards"). It must run even when the charset layer is
            // skipped — hence after this `continue`, not before it.
            for (pattern, desc) in &injection_patterns {
                if value.contains(pattern) {
                    return Err(OxoFlowError::Validation {
                        message: format!(
                            "Wildcard injection detected: {} pattern in value '{}' for key '{}'",
                            desc, value, key
                        ),
                        rule: None,
                        suggestion: Some(
                            "Sample names and other wildcard values must not contain \
                             shell command substitution; rename the value or file."
                                .to_string(),
                        ),
                    });
                }
            }
            continue;
        }
        for (pattern, desc) in &injection_patterns {
            if value.contains(pattern) {
                return Err(OxoFlowError::Validation {
                    message: format!(
                        "Wildcard injection detected: {} pattern in value '{}' for key '{}'",
                        desc, value, key
                    ),
                    rule: None,
                    suggestion: Some(
                        "Ensure sample names and file paths do not contain shell metacharacters."
                            .to_string(),
                    ),
                });
            }
        }
        // An empty value carries no metacharacters and is a legitimate
        // "disable this feature" sentinel (issue #374: e.g.
        // `umi2_pattern = ""` meaning "no second barcode pattern"). The
        // charset regex below is `+`-quantified and would reject it, so
        // accept empty explicitly — the substitution floor is trivially
        // satisfied (an empty string contains no `$(` or backtick).
        if value.is_empty() {
            continue;
        }
        if !DEFAULT_WILDCARD_RE.is_match(value) {
            return Err(OxoFlowError::Validation {
                message: format!(
                    "Wildcard '{key}' has value '{}' outside the safe default \
                     character set ({DEFAULT_WILDCARD_PATTERN})",
                    value
                ),
                rule: None,
                suggestion: Some(format!(
                    "Declare a constraint for this wildcard in `wildcard_constraints` \
                     (e.g. {key} = '^.+$' to allow anything), or set \
                     OXO_FLOW_UNSAFE_WILDCARDS=1 to accept any characters with a logged warning."
                )),
            });
        }
    }
    Ok(())
}

/// Validate that a file path does not escape the working directory
/// (path traversal prevention).
///
/// Returns `Ok(())` if the path is safe, or an error if traversal is detected.
#[must_use = "path safety validation returns a Result that must be checked"]
pub fn validate_path_safety(workdir: &Path, path: &str) -> Result<()> {
    // Block absolute paths outside workdir
    if path.starts_with('/') {
        let abs_path = Path::new(path);
        if !abs_path.starts_with(workdir) {
            return Err(OxoFlowError::Validation {
                message: format!("Absolute path '{}' outside working directory", path),
                rule: None,
                suggestion: Some("Use relative paths within the workflow directory".to_string()),
            });
        }
    }

    // Block path traversal via ".."
    let resolved = workdir.join(path);
    if path.contains("..") {
        // Attempt canonicalization to see if it escapes
        if let Ok(canonical) = resolved.canonicalize() {
            if !canonical.starts_with(workdir) {
                return Err(OxoFlowError::Validation {
                    message: format!("Path '{}' escapes the working directory", path),
                    rule: None,
                    suggestion: Some(
                        "Use relative paths within the workflow directory".to_string(),
                    ),
                });
            }
        } else {
            // Path doesn't exist yet, but contains ".." which is suspicious
            return Err(OxoFlowError::Validation {
                message: format!(
                    "Path '{}' contains '..' which may escape the working directory",
                    path
                ),
                rule: None,
                suggestion: Some("Avoid using '..' in output paths".to_string()),
            });
        }
    }
    Ok(())
}
#[cfg(test)]
mod wildcard_default_tests {
    use super::*;

    fn v(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, val)| (k.to_string(), val.to_string()))
            .collect()
    }

    #[test]
    fn default_charset_accepts_realistic_samples() {
        let vals = v(&[
            ("sample", "SRR1039508"),
            ("chr", "chr21"),
            ("path", "data/subdir/genome.fa"),
        ]);
        assert!(validate_wildcard_injection_inner(&vals, &HashMap::new(), false).is_ok());
    }

    #[test]
    fn empty_string_is_accepted_by_default_charset() {
        // Issue #374: an empty value is a legitimate feature-off sentinel
        // (e.g. `umi2_pattern = ""` = "no second barcode pattern"). It must
        // pass the default charset layer without a declared constraint.
        let vals = v(&[("umi2_pattern", ""), ("sample", "S1")]);
        assert!(validate_wildcard_injection_inner(&vals, &HashMap::new(), false).is_ok());
        // Empty stays accepted under a constraint and in unsafe mode too.
        let mut constraints = HashMap::new();
        constraints.insert("umi2_pattern".to_string(), "^.+$".to_string());
        assert!(validate_wildcard_injection_inner(&vals, &constraints, false).is_ok());
        assert!(validate_wildcard_injection_inner(&vals, &HashMap::new(), true).is_ok());
    }

    #[test]
    fn default_charset_rejects_metacharacters_and_spaces() {
        for bad in ["a; b", "x&&y", "a|b", "$(id)", "`id`", "two words", "a>b"] {
            let vals = v(&[("sample", bad)]);
            let err = validate_wildcard_injection_inner(&vals, &HashMap::new(), false)
                .err()
                .unwrap_or_else(|| panic!("'{bad}' must be rejected"));
            let msg = err.to_string();
            if bad.contains("$(") || bad.contains('`') {
                assert!(msg.contains("injection"), "{msg}");
            } else {
                assert!(msg.contains("safe default"), "{msg}");
            }
        }
    }

    #[test]
    fn explicit_constraint_overrides_default_charset() {
        let vals = v(&[("sample", "tumor / normal")]);
        let mut constraints = HashMap::new();
        constraints.insert("sample".to_string(), "^.+$".to_string());
        assert!(
            validate_wildcard_injection_inner(&vals, &constraints, false).is_ok(),
            "declared constraint governs"
        );
    }

    #[test]
    fn unsafe_mode_relaxes_charset_but_not_substitution_floor() {
        let relaxed = v(&[("sample", "two words")]);
        assert!(validate_wildcard_injection_inner(&relaxed, &HashMap::new(), true).is_ok());
        let hostile = v(&[("sample", "$(id)")]);
        assert!(validate_wildcard_injection_inner(&hostile, &HashMap::new(), true).is_err());
    }

    #[test]
    fn constrained_wildcard_still_hits_the_substitution_floor() {
        // Issue #276 Repro A: the doc promises "rejected unconditionally —
        // including for constrained wildcards", but the floor used to sit
        // BEFORE the constraint skip, so a declared `sample = "^.+$"`
        // constraint silently disabled it.
        let mut constraints = HashMap::new();
        constraints.insert("sample".to_string(), "^.+$".to_string());
        for hostile in ["x$(touch pwned.txt)", "a`touch pwned`b"] {
            let vals = v(&[("sample", hostile)]);
            let err = validate_wildcard_injection_inner(&vals, &constraints, false)
                .err()
                .unwrap_or_else(|| panic!("'{hostile}' must be rejected under a constraint"));
            assert!(err.to_string().contains("injection"), "{err}");
        }
    }

    #[test]
    fn substitution_floor_holds_in_unsafe_mode() {
        // Issue #276: OXO_FLOW_UNSAFE_WILDCARDS=1 relaxes the CHARSET layer
        // only — the floor must still reject command substitution.
        for hostile in ["x$(touch pwned.txt)", "`touch pwned`"] {
            let vals = v(&[("sample", hostile)]);
            let err = validate_wildcard_injection_inner(&vals, &HashMap::new(), true)
                .err()
                .unwrap_or_else(|| panic!("'{hostile}' must be rejected in unsafe mode"));
            assert!(err.to_string().contains("injection"), "{err}");
        }
    }
}
