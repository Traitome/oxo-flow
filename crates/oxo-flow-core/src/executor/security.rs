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
        patterns: &[
            r"rm\s+-rf\s+(?:--\S+\s+)*/",
            r"rm\s+-rf\s+(?:--\S+\s+)*~",
            r"rm\s+-r\s+(?:--\S+\s+)*/",
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
            warnings.push(format!(
                "Shell command warning: {} in '{}'",
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
///    [`DEFAULT_WILDCARD_PATTERN`]. Pipelines that legitimately need other
///    characters declare a constraint for that wildcard (the pre-existing
///    mechanism), or set `OXO_FLOW_UNSAFE_WILDCARDS=1` to relax the charset
///    layer for the process — a one-time warning is logged.
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
