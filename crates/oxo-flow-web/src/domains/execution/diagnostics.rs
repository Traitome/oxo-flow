//! Deterministic diagnostics engine for pipeline execution errors.
//!
//! Matches exit codes and stderr patterns against 30+ known error signatures
//! to produce structured, actionable diagnostic results — including
//! auto-fixable configuration changes when a known remedy exists.

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum ErrorCategory {
    Tool,
    Resource,
    Data,
    System,
    Config,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DiagnosticResult {
    pub rule: String,
    pub error_pattern: Option<String>,
    pub category: ErrorCategory,
    pub likely_cause: String,
    pub suggestions: Vec<String>,
    pub auto_fixable: bool,
    pub fix_action: Option<FixAction>,
    pub relevant_log_lines: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FixAction {
    pub description: String,
    pub config_change: Option<ConfigChange>,
    pub command: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConfigChange {
    pub path: String,
    pub old_value: String,
    pub new_value: String,
}

struct Pattern {
    id: &'static str,
    category: ErrorCategory,
    exit_codes: Vec<i32>,
    stderr_patterns: Vec<&'static str>,
    likely_cause: &'static str,
    auto_fixable: bool,
    fix_desc: Option<&'static str>,
    fix_config_path: Option<&'static str>,
}

pub struct DiagnosticsEngine {
    patterns: Vec<Pattern>,
}

impl DiagnosticsEngine {
    pub fn new() -> Self {
        let patterns = vec![
            // ---- Tool errors ----
            Pattern {
                id: "command_not_found",
                category: ErrorCategory::Tool,
                exit_codes: vec![127],
                stderr_patterns: vec!["command not found", "No such file"],
                likely_cause: "Required tool not installed or not in PATH.",
                auto_fixable: true,
                fix_desc: Some("Install missing tool via conda or package manager."),
                fix_config_path: None,
            },
            Pattern {
                id: "segfault",
                category: ErrorCategory::Tool,
                exit_codes: vec![139, 11],
                stderr_patterns: vec!["segmentation fault", "SIGSEGV"],
                likely_cause: "Tool crashed with segfault. Possible binary incompatibility or bug.",
                auto_fixable: false,
                fix_desc: Some("Try a different tool version or check for known issues."),
                fix_config_path: None,
            },
            Pattern {
                id: "illegal_instruction",
                category: ErrorCategory::Tool,
                exit_codes: vec![132, 4],
                stderr_patterns: vec!["illegal instruction", "SIGILL"],
                likely_cause: "Binary compiled for different CPU architecture.",
                auto_fixable: false,
                fix_desc: Some("Rebuild tool for this CPU or use a compatible binary."),
                fix_config_path: None,
            },
            Pattern {
                id: "bus_error",
                category: ErrorCategory::Tool,
                exit_codes: vec![138, 10],
                stderr_patterns: vec!["bus error", "SIGBUS"],
                likely_cause: "Memory alignment error. Input file may be on corrupt filesystem.",
                auto_fixable: false,
                fix_desc: Some("Check filesystem integrity. Move data to reliable storage."),
                fix_config_path: None,
            },
            Pattern {
                id: "tool_version_mismatch",
                category: ErrorCategory::Tool,
                exit_codes: vec![],
                stderr_patterns: vec!["version", "incompatible", "unsupported"],
                likely_cause: "Tool version incompatible with inputs or parameters.",
                auto_fixable: false,
                fix_desc: Some("Use a compatible tool version."),
                fix_config_path: None,
            },
            // ---- Resource errors ----
            Pattern {
                id: "oom_killed",
                category: ErrorCategory::Resource,
                exit_codes: vec![137, 9],
                stderr_patterns: vec![
                    "out of memory",
                    "memory error",
                    "cannot allocate memory",
                    "bad_alloc",
                    "std::bad_alloc",
                ],
                likely_cause: "Process killed due to insufficient memory (OOM).",
                auto_fixable: true,
                fix_desc: Some("Increase memory in [resources] section."),
                fix_config_path: Some("resources.memory"),
            },
            Pattern {
                id: "timeout",
                category: ErrorCategory::Resource,
                exit_codes: vec![124],
                stderr_patterns: vec!["timed out", "timeout", "time limit"],
                likely_cause: "Process exceeded time limit.",
                auto_fixable: true,
                fix_desc: Some("Increase time_limit in [resources] section."),
                fix_config_path: Some("resources.time_limit"),
            },
            Pattern {
                id: "disk_full",
                category: ErrorCategory::Resource,
                exit_codes: vec![],
                stderr_patterns: vec!["no space left on device", "disk quota exceeded", "ENOSPC"],
                likely_cause: "Disk full. Free up space or redirect output.",
                auto_fixable: false,
                fix_desc: Some("Clean temp files or increase disk allocation."),
                fix_config_path: None,
            },
            Pattern {
                id: "cpu_limit",
                category: ErrorCategory::Resource,
                exit_codes: vec![],
                stderr_patterns: vec![
                    "cpu time",
                    "cpu limit",
                    "resource temporarily unavailable",
                    "EAGAIN",
                ],
                likely_cause: "CPU time limit exceeded or resource temporarily unavailable.",
                auto_fixable: true,
                fix_desc: Some("Increase thread count or CPU allocation."),
                fix_config_path: Some("resources.threads"),
            },
            Pattern {
                id: "ulimit",
                category: ErrorCategory::Resource,
                exit_codes: vec![],
                stderr_patterns: vec!["too many open files", "ulimit", "EMFILE", "ENFILE"],
                likely_cause: "Too many open files. Increase ulimit.",
                auto_fixable: true,
                fix_desc: Some("Run 'ulimit -n 65536' before starting."),
                fix_config_path: None,
            },
            // ---- Data errors ----
            Pattern {
                id: "file_not_found",
                category: ErrorCategory::Data,
                exit_codes: vec![1],
                stderr_patterns: vec![
                    "no such file or directory",
                    "cannot open",
                    "not found",
                    "ENOENT",
                ],
                likely_cause: "Input file not found. Check paths and wildcard expansion.",
                auto_fixable: false,
                fix_desc: Some("Verify input file exists at expected path."),
                fix_config_path: None,
            },
            Pattern {
                id: "truncated_file",
                category: ErrorCategory::Data,
                exit_codes: vec![1],
                stderr_patterns: vec![
                    "truncated",
                    "unexpected end",
                    "premature",
                    "corrupt",
                    "incomplete",
                ],
                likely_cause: "Input file truncated or corrupted.",
                auto_fixable: false,
                fix_desc: Some("Verify file integrity with checksum. Re-download if needed."),
                fix_config_path: None,
            },
            Pattern {
                id: "empty_file",
                category: ErrorCategory::Data,
                exit_codes: vec![],
                stderr_patterns: vec!["empty file", "zero length", "no data"],
                likely_cause: "Input file is empty.",
                auto_fixable: false,
                fix_desc: Some("Check upstream steps produced valid output."),
                fix_config_path: None,
            },
            Pattern {
                id: "low_quality_fastq",
                category: ErrorCategory::Data,
                exit_codes: vec![],
                stderr_patterns: vec![
                    "per base sequence quality.*fail",
                    "low quality",
                    "poor quality",
                ],
                likely_cause: "FASTQ files have low quality scores.",
                auto_fixable: true,
                fix_desc: Some("Insert fastp or Trimmomatic step before this rule."),
                fix_config_path: None,
            },
            Pattern {
                id: "gzip_corrupt",
                category: ErrorCategory::Data,
                exit_codes: vec![1],
                stderr_patterns: vec!["not in gzip format", "gzip", "corrupt input"],
                likely_cause: "Gzipped file is corrupt or not actually gzipped.",
                auto_fixable: false,
                fix_desc: Some("Verify file is valid gzip: 'gzip -t file.gz'"),
                fix_config_path: None,
            },
            Pattern {
                id: "bam_truncated",
                category: ErrorCategory::Data,
                exit_codes: vec![1],
                stderr_patterns: vec!["truncated file", "bam index", "EOF marker"],
                likely_cause: "BAM file missing index or truncated.",
                auto_fixable: true,
                fix_desc: Some("Run 'samtools index' to rebuild BAM index."),
                fix_config_path: None,
            },
            // ---- System errors ----
            Pattern {
                id: "permission_denied",
                category: ErrorCategory::System,
                exit_codes: vec![126, 13],
                stderr_patterns: vec!["permission denied", "EACCES", "not writable"],
                likely_cause: "Insufficient file permissions.",
                auto_fixable: false,
                fix_desc: Some("Check file permissions. Ensure user has r/w access."),
                fix_config_path: None,
            },
            Pattern {
                id: "broken_pipe",
                category: ErrorCategory::System,
                exit_codes: vec![141, 13],
                stderr_patterns: vec!["broken pipe", "SIGPIPE"],
                likely_cause: "Downstream process exited early, breaking the pipe.",
                auto_fixable: false,
                fix_desc: Some("Check all pipeline commands produce valid output."),
                fix_config_path: None,
            },
            Pattern {
                id: "network_error",
                category: ErrorCategory::System,
                exit_codes: vec![],
                stderr_patterns: vec!["connection refused", "network", "cannot resolve", "timeout"],
                likely_cause: "Network resource unavailable.",
                auto_fixable: false,
                fix_desc: Some("Check network connectivity and remote resource availability."),
                fix_config_path: None,
            },
            Pattern {
                id: "signal_kill",
                category: ErrorCategory::System,
                exit_codes: vec![143, 15],
                stderr_patterns: vec!["terminated", "SIGTERM", "killed"],
                likely_cause: "Process terminated by external signal.",
                auto_fixable: false,
                fix_desc: Some("Process was externally terminated. Check system logs."),
                fix_config_path: None,
            },
            Pattern {
                id: "signal_interrupt",
                category: ErrorCategory::System,
                exit_codes: vec![130, 2],
                stderr_patterns: vec!["interrupt", "SIGINT", "cancelled"],
                likely_cause: "Process was interrupted (Ctrl+C or equivalent).",
                auto_fixable: false,
                fix_desc: Some("Re-run the workflow when ready."),
                fix_config_path: None,
            },
            Pattern {
                id: "shared_library",
                category: ErrorCategory::System,
                exit_codes: vec![127, 1],
                stderr_patterns: vec![
                    "error while loading shared libraries",
                    "lib",
                    "so.",
                    "cannot open shared object",
                ],
                likely_cause: "Missing shared library dependency.",
                auto_fixable: true,
                fix_desc: Some(
                    "Install missing shared libraries via conda or system package manager.",
                ),
                fix_config_path: None,
            },
            // ---- Config errors ----
            Pattern {
                id: "invalid_param",
                category: ErrorCategory::Config,
                exit_codes: vec![1],
                stderr_patterns: vec![
                    "invalid option",
                    "unrecognized",
                    "unknown option",
                    "bad argument",
                ],
                likely_cause: "Invalid or unrecognized command parameter.",
                auto_fixable: false,
                fix_desc: Some("Check tool documentation for correct parameter spelling."),
                fix_config_path: None,
            },
            Pattern {
                id: "missing_required_param",
                category: ErrorCategory::Config,
                exit_codes: vec![1],
                stderr_patterns: vec!["required", "must specify", "missing", "argument expected"],
                likely_cause: "Required parameter is missing.",
                auto_fixable: false,
                fix_desc: Some("Add the missing parameter to the rule command."),
                fix_config_path: None,
            },
            Pattern {
                id: "wildcard_empty",
                category: ErrorCategory::Config,
                exit_codes: vec![],
                stderr_patterns: vec!["no matches", "no files", "wildcard", "empty", "no input"],
                likely_cause: "Wildcard pattern matched no files.",
                auto_fixable: false,
                fix_desc: Some("Check file naming matches the wildcard pattern."),
                fix_config_path: None,
            },
            Pattern {
                id: "conda_env_fail",
                category: ErrorCategory::Config,
                exit_codes: vec![1],
                stderr_patterns: vec!["conda", "environment", "environment.yml", "create failed"],
                likely_cause: "Conda environment creation or activation failed.",
                auto_fixable: false,
                fix_desc: Some("Verify conda is installed and environment name is correct."),
                fix_config_path: None,
            },
            Pattern {
                id: "docker_fail",
                category: ErrorCategory::Config,
                exit_codes: vec![125, 126],
                stderr_patterns: vec![
                    "docker",
                    "cannot connect",
                    "daemon",
                    "permission denied.*docker",
                ],
                likely_cause: "Docker daemon not running or no permission.",
                auto_fixable: false,
                fix_desc: Some("Start Docker daemon or add user to docker group."),
                fix_config_path: None,
            },
            Pattern {
                id: "singularity_fail",
                category: ErrorCategory::Config,
                exit_codes: vec![255],
                stderr_patterns: vec!["singularity", "apptainer", "image not found"],
                likely_cause: "Singularity/Apptainer image not found or daemon issue.",
                auto_fixable: false,
                fix_desc: Some("Pull the container image or check Singularity installation."),
                fix_config_path: None,
            },
            Pattern {
                id: "shell_syntax",
                category: ErrorCategory::Config,
                exit_codes: vec![2],
                stderr_patterns: vec!["syntax error", "unexpected token", "parse error"],
                likely_cause: "Shell syntax error in the rule command.",
                auto_fixable: false,
                fix_desc: Some("Review the shell command for syntax errors."),
                fix_config_path: None,
            },
        ];
        Self { patterns }
    }

    /// Analyze log output and optional exit code for a given rule.
    /// Returns zero or more diagnostic results.
    pub fn analyze(
        &self,
        rule_name: &str,
        log_output: &str,
        exit_code: Option<i32>,
    ) -> Vec<DiagnosticResult> {
        let log_lower = log_output.to_lowercase();
        let mut results = Vec::new();

        for p in &self.patterns {
            let exit_match =
                exit_code.is_none_or(|ec| p.exit_codes.is_empty() || p.exit_codes.contains(&ec));
            let stderr_match = p
                .stderr_patterns
                .iter()
                .any(|pat| log_lower.contains(&pat.to_lowercase()));

            if exit_match && stderr_match {
                let relevant: Vec<String> = log_output
                    .lines()
                    .filter(|line| {
                        p.stderr_patterns
                            .iter()
                            .any(|pat| line.to_lowercase().contains(&pat.to_lowercase()))
                    })
                    .take(10)
                    .map(|s| s.to_string())
                    .collect();

                results.push(DiagnosticResult {
                    rule: rule_name.to_string(),
                    error_pattern: Some(p.id.to_string()),
                    category: p.category.clone(),
                    likely_cause: p.likely_cause.to_string(),
                    suggestions: vec![
                        p.fix_desc
                            .unwrap_or("Manual investigation required.")
                            .to_string(),
                    ],
                    auto_fixable: p.auto_fixable,
                    fix_action: p.fix_desc.map(|desc| FixAction {
                        description: desc.to_string(),
                        config_change: p.fix_config_path.map(|path| ConfigChange {
                            path: path.to_string(),
                            old_value: "current".into(),
                            new_value: "increase".into(),
                        }),
                        command: None,
                    }),
                    relevant_log_lines: relevant,
                });
            }
        }

        // Generic fallback for non-zero exit without pattern match
        if results.is_empty() && exit_code.unwrap_or(0) != 0 {
            results.push(DiagnosticResult {
                rule: rule_name.to_string(),
                error_pattern: Some("unknown_error".into()),
                category: ErrorCategory::Tool,
                likely_cause: format!(
                    "Process exited with code {}. Check logs.",
                    exit_code.unwrap()
                ),
                suggestions: vec![
                    "Review full execution log.".into(),
                    "Check input file integrity.".into(),
                ],
                auto_fixable: false,
                fix_action: None,
                relevant_log_lines: log_output.lines().take(20).map(|s| s.to_string()).collect(),
            });
        }

        results
    }
}

impl Default for DiagnosticsEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oom_detection() {
        let engine = DiagnosticsEngine::new();
        let log = "STAR exiting: FATAL error, OUT OF MEMORY\nEXITING because of FATAL ERROR: 137";
        let results = engine.analyze("star_align", log, Some(137));
        assert_eq!(results[0].error_pattern.as_deref(), Some("oom_killed"));
        assert!(results[0].auto_fixable);
    }

    #[test]
    fn test_command_not_found() {
        let engine = DiagnosticsEngine::new();
        let log = "/bin/bash: fastqc: command not found";
        let results = engine.analyze("fastqc", log, Some(127));
        assert_eq!(
            results[0].error_pattern.as_deref(),
            Some("command_not_found")
        );
    }

    #[test]
    fn test_permission_denied() {
        let engine = DiagnosticsEngine::new();
        let log = "error: permission denied: /data/output/results.txt";
        let results = engine.analyze("report", log, Some(13));
        assert_eq!(
            results[0].error_pattern.as_deref(),
            Some("permission_denied")
        );
        assert!(!results[0].auto_fixable);
    }

    #[test]
    fn test_file_not_found() {
        let engine = DiagnosticsEngine::new();
        let log = "Error: no such file or directory: /data/sample.fastq";
        let results = engine.analyze("align", log, Some(1));
        assert_eq!(results[0].error_pattern.as_deref(), Some("file_not_found"));
    }

    #[test]
    fn test_timeout() {
        let engine = DiagnosticsEngine::new();
        let log = "Error: process timed out after 3600 seconds";
        let results = engine.analyze("slow_rule", log, Some(124));
        assert_eq!(results[0].error_pattern.as_deref(), Some("timeout"));
        assert!(results[0].auto_fixable);
    }

    #[test]
    fn test_disk_full() {
        let engine = DiagnosticsEngine::new();
        let log = "OSError: [Errno 28] No space left on device";
        let results = engine.analyze("samtools_sort", log, None);
        assert_eq!(results[0].error_pattern.as_deref(), Some("disk_full"));
    }

    #[test]
    fn test_truncated_file() {
        let engine = DiagnosticsEngine::new();
        let log = "gzip: stdin: unexpected end of file";
        let results = engine.analyze("fastqc", log, Some(1));
        assert_eq!(results[0].error_pattern.as_deref(), Some("truncated_file"));
    }

    #[test]
    fn test_unknown_error_fallback() {
        let engine = DiagnosticsEngine::new();
        let log = "something went wrong";
        let results = engine.analyze("mystery", log, Some(99));
        assert_eq!(results[0].error_pattern.as_deref(), Some("unknown_error"));
    }
}
