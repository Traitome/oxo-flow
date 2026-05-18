use crate::error::{OxoFlowError, Result};
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

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
/// [`DANGER_CATEGORIES`] to detect destructive commands such as:
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

/// Validate wildcard VALUES for shell injection patterns.
///
/// Unlike [`validate_shell_safety`] which operates on the full rendered
/// command (and therefore trusts the shell template), this function checks
/// only the values that will be *substituted* into the template via
/// wildcard expansion (e.g., sample names from a CSV file, file paths).
///
/// Returns `Ok(())` if all values are safe, or an error if any value
/// contains a pattern that would execute arbitrary shell code.
#[must_use = "wildcard injection validation returns a Result that must be checked"]
pub fn validate_wildcard_injection(wildcard_values: &HashMap<String, String>) -> Result<()> {
    // Patterns that indicate injection attempts in externally supplied values.
    // Config values from the .oxoflow file itself (prefixed with "config.")
    // are trusted and skipped.
    let injection_patterns = [
        ("$(", "command substitution"),
        ("`", "backtick substitution"),
    ];
    for (key, value) in wildcard_values {
        // Skip config.* keys — these come from the trusted .oxoflow file.
        if key.starts_with("config.") {
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
