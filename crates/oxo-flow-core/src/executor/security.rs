use crate::error::{OxoFlowError, Result};
use std::collections::HashMap;
use std::path::Path;

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
    let dangerous_patterns = [
        ("$(", "Command substitution detected"),
        ("`", "Backtick command substitution detected"),
        (">/dev/", "Redirect to /dev/ detected"),
        ("rm -rf /", "Dangerous recursive deletion detected"),
        ("chmod 777", "Overly permissive chmod detected"),
        ("eval ", "eval usage detected"),
    ];
    for (pattern, warning) in &dangerous_patterns {
        if cmd.contains(pattern) {
            warnings.push(format!("Shell command warning: {} in '{}'", warning, cmd));
        }
    }
    warnings
}

/// Block dangerous shell patterns that could lead to command injection.
/// Returns Ok(()) if safe, Err if dangerous patterns are detected.
///
/// Only blocks unconditionally destructive commands (e.g., `rm -rf /`).
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
    // R12: Improve validate_shell_safety coverage
    let block_patterns = [
        ("rm -rf /", "dangerous deletion"),
        ("rm -rf ~", "dangerous deletion of home directory"),
        ("mkfs", "filesystem creation"),
        ("dd if=/dev/zero", "data destruction"),
        (":(){ :|:& };:", "fork bomb"),
    ];
    for (pattern, desc) in &block_patterns {
        if cmd.contains(pattern) {
            return Err(OxoFlowError::Validation {
                message: format!(
                    "Shell command blocked: {} pattern detected in '{}'",
                    desc, cmd
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
