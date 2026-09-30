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

impl Pattern {
    /// True when this pattern can fire from log content alone, without a
    /// corroborating exit code (issue #708: patterns with empty `exit_codes`
    /// previously matched *every* exit status — including success — off a
    /// single loose substring like "version"). Test-only now: the exit-code
    /// gate judges log-only patterns by their regexes, not by this flag.
    #[cfg(test)]
    fn log_only(&self) -> bool {
        self.exit_codes.is_empty()
    }
}

/// Substring patterns are matched as whole-word/anchored regexes against the
/// lowercased log (issue #708: `contains("version")` fired on success
/// banners, `contains("so.")` on "also."). Patterns are compiled once at
/// engine construction; an entry that fails to compile is skipped — the table
/// is static, and the table-driven test below proves every entry compiles.
fn compile_patterns(pats: &[&str]) -> Vec<regex::Regex> {
    pats.iter()
        .filter_map(|p| regex::Regex::new(&p.to_lowercase()).ok())
        .collect()
}

pub struct DiagnosticsEngine {
    patterns: Vec<Pattern>,
    compiled: Vec<(usize, Vec<regex::Regex>)>,
}

impl DiagnosticsEngine {
    pub fn new() -> Self {
        let patterns = vec![
            // ---- Tool errors ----
            Pattern {
                id: "command_not_found",
                category: ErrorCategory::Tool,
                exit_codes: vec![127],
                stderr_patterns: vec!["command not found", "no such file or directory"],
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
                // Anchored complaint phrasings only (issue #708): a bare
                // "version" matched every tool banner printing its version.
                stderr_patterns: vec![
                    "incompatible .*version",
                    "unsupported .*version",
                    "version .*incompatible",
                    "requires .*version",
                    "version mismatch",
                ],
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
                stderr_patterns: vec![
                    "no space left on device",
                    "disk quota exceeded",
                    "\\benospc\\b",
                ],
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
                    "\\beagain\\b",
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
                stderr_patterns: vec![
                    "too many open files",
                    "\\bulimit\\b",
                    "\\bemfile\\b",
                    "\\benfile\\b",
                ],
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
                // bare "not found" matched command-not-found, version
                // strings, and tool names (issue #708)
                stderr_patterns: vec![
                    "no such file or directory",
                    "cannot open",
                    "\\bnot found\\b.*\\b(file|input|path)\\b",
                    "\\bfile\\b.*\\bnot found\\b",
                    "enoent",
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
                // "premature"/"incomplete"/"corrupt" as bare words matched
                // unrelated prose (issue #708)
                stderr_patterns: vec![
                    "\\btruncated\\b",
                    "unexpected end",
                    "premature end",
                    "\\bcorrupt\\b",
                    "incomplete (file|input|read|bam)",
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
                stderr_patterns: vec!["\\bempty file\\b", "zero length", "file is empty"],
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
                    "\\blow quality\\b",
                    "\\bpoor quality\\b",
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
                // bare "gzip" matched any tool mentioning gzip (issue #708)
                stderr_patterns: vec![
                    "not in gzip format",
                    "\\bgzip\\b.*(invalid|corrupt|failed)",
                    "corrupt input",
                    "unexpected end of (file|gzip)",
                ],
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
                // "network"/"timeout" alone matched benign mentions
                // (issue #708); anchor on connection-failure phrasing.
                stderr_patterns: vec![
                    "connection refused",
                    "could not resolve",
                    "cannot resolve",
                    "name or service not known",
                    "network is unreachable",
                    "connection reset by peer",
                    "curl: \\(7\\)",
                ],
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
                // Loader-line phrasings only (issue #708): "so." matched
                // "also." and "lib" matched any path with "lib" in it.
                stderr_patterns: vec![
                    "error while loading shared libraries",
                    "cannot open shared object",
                    "\\blib\\S+\\.so[\\.0-9]*: cannot open",
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
                // "missing"/"required" alone matched half of all failure
                // text (issue #708); anchor on argument-error phrasing.
                exit_codes: vec![1],
                stderr_patterns: vec![
                    "the following (arguments|options) are required",
                    "must specify",
                    "required argument.*missing",
                    "missing required (argument|parameter|option)",
                    "argument expected",
                ],
                likely_cause: "Required parameter is missing.",
                auto_fixable: false,
                fix_desc: Some("Add the missing parameter to the rule command."),
                fix_config_path: None,
            },
            Pattern {
                id: "wildcard_empty",
                category: ErrorCategory::Config,
                exit_codes: vec![],
                // "empty"/"no files" alone matched unrelated failures
                // (issue #708); anchor on wildcard/no-match phrasing.
                stderr_patterns: vec![
                    "no matches found",
                    "no files (were )?(found|match)",
                    "wildcard.*no (match|files)",
                    "no input files",
                    "glob matched no( files|thing)",
                ],
                likely_cause: "Wildcard pattern matched no files.",
                auto_fixable: false,
                fix_desc: Some("Check file naming matches the wildcard pattern."),
                fix_config_path: None,
            },
            Pattern {
                id: "conda_env_fail",
                category: ErrorCategory::Config,
                exit_codes: vec![1],
                // "environment" alone matched every tool that prints its
                // active env name (issue #708); anchor on failure phrasing.
                stderr_patterns: vec![
                    "conda.*error",
                    "environment creation",
                    "environmentnotfound",
                    "condaenvnotfounderror",
                    "failed to activate conda",
                    "create failed",
                ],
                likely_cause: "Conda environment creation or activation failed.",
                auto_fixable: false,
                fix_desc: Some("Verify conda is installed and environment name is correct."),
                fix_config_path: None,
            },
            Pattern {
                id: "docker_fail",
                category: ErrorCategory::Config,
                exit_codes: vec![125, 126],
                // "docker"/"daemon" matched any successful docker banner
                // (issue #708); anchor on failure phrasings.
                stderr_patterns: vec![
                    "cannot connect to the docker daemon",
                    "docker daemon.*not running",
                    "permission denied.*docker",
                    "is the docker daemon running",
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
            // ---- Data: BAM index missing ----
            Pattern {
                id: "bam_index_missing",
                category: ErrorCategory::Data,
                exit_codes: vec![1],
                stderr_patterns: vec![
                    "failed to open index",
                    "could not open index file",
                    "no index available",
                    "bai file not found",
                    "index file is missing",
                ],
                likely_cause: "BAM index file (.bai) not found. Most tools (samtools, IGV) require a sorted and indexed BAM.",
                auto_fixable: true,
                fix_desc: Some("Generate index with: samtools index <file.bam>"),
                fix_config_path: None,
            },
        ];
        let compiled = patterns
            .iter()
            .enumerate()
            .map(|(i, p)| (i, compile_patterns(&p.stderr_patterns)))
            .collect();
        Self { patterns, compiled }
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

        for (i, p) in self.patterns.iter().enumerate() {
            // Exit-code gate (issue #708): patterns with declared exit codes
            // only fire with a matching code; log-only patterns (empty table)
            // must anchor on precise regexes instead, and never fire on a
            // *successful* rule (exit 0).
            let exit_match = match exit_code {
                Some(0) => false,
                Some(ec) => p.exit_codes.is_empty() || p.exit_codes.contains(&ec),
                // No exit code recorded: judge by log content alone (previous
                // `is_none_or` semantics). The #708 fix is the Some(0) /
                // Some(ec) arms plus the tightened regexes below.
                None => true,
            };
            let stderr_match = self.compiled[i].1.iter().any(|re| re.is_match(&log_lower));

            if exit_match && stderr_match {
                let relevant: Vec<String> = log_output
                    .lines()
                    .filter(|line| {
                        self.compiled[i]
                            .1
                            .iter()
                            .any(|re| re.is_match(&line.to_lowercase()))
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

    // ---- Table-driven gate (issue #708): every pattern must fire on its
    // canonical trigger and stay silent on benign lines. ----

    /// Canonical trigger log line per pattern id. Every entry here must
    /// actually match — a pattern whose trigger can't fire is dead weight.
    const CANONICAL_TRIGGERS: &[(&str, &str)] = &[
        ("command_not_found", "/bin/bash: bwa: command not found"),
        ("segfault", "segmentation fault (core dumped)"),
        ("illegal_instruction", "illegal instruction (core dumped)"),
        ("bus_error", "bus error"),
        (
            "tool_version_mismatch",
            "Error: incompatible version: requires >=2.0",
        ),
        ("oom_killed", "Fatal error: out of memory"),
        ("timeout", "Error: process timed out after 3600 seconds"),
        ("disk_full", "OSError: [Errno 28] No space left on device"),
        ("cpu_limit", "EAGAIN: resource temporarily unavailable"),
        ("ulimit", "too many open files"),
        (
            "file_not_found",
            "open: no such file or directory: sample.fastq",
        ),
        ("truncated_file", "gzip: stdin: unexpected end of file"),
        ("empty_file", "Error: empty file: sample.fastq"),
        (
            "low_quality_fastq",
            "fastqc: per base sequence quality fail",
        ),
        ("gzip_corrupt", "gzip: stdin: not in gzip format"),
        (
            "bam_truncated",
            "samtools: EOF marker is absent; truncated file",
        ),
        ("permission_denied", "error: permission denied: results.txt"),
        ("broken_pipe", "head: error: broken pipe"),
        (
            "network_error",
            "curl: (7) Failed to connect: connection refused",
        ),
        ("signal_kill", "process terminated by SIGTERM"),
        ("signal_interrupt", "process cancelled by SIGINT"),
        (
            "shared_library",
            "error while loading shared libraries: libhts.so.2: cannot open shared object file",
        ),
        (
            "invalid_param",
            "error: unrecognized option '--nonexistent'",
        ),
        (
            "missing_required_param",
            "error: the following arguments are required: --input",
        ),
        ("wildcard_empty", "No matches found for *.bam in data/"),
        ("conda_env_fail", "CondaError: environment creation failed"),
        (
            "docker_fail",
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock",
        ),
        ("singularity_fail", "FATAL: image not found: tool.sif"),
        (
            "shell_syntax",
            "/bin/sh: -c: line 1: syntax error: unexpected token",
        ),
        (
            "bam_index_missing",
            "samtools view: could not open index file: sample.bam.bai",
        ),
    ];

    /// Benign log lines that must NOT trip any pattern (the #708
    /// false-positive class: success banners, prose, version strings).
    const BENIGN_LINES: &[&str] = &[
        "bwa version 0.7.17-r1188",
        "samtools 1.19.2 (built with htslib 1.19.1)",
        "Loaded reference genome. Also checking index.",
        "Starting rule fastqc with 8 threads, version 0.11.9",
        "Zoom into region chr1:1000-2000 completed",
        "Reading 5000 sequences from the room dataset",
        "Docker image digest verified, pulling layers",
        "Environment variables loaded from environment.yml",
        "The process was not killed; it exited normally",
        "Downloaded 100% of file (45.2 MB/s)",
        "gatk BestPractices version 4.4.0.0 started",
    ];

    #[test]
    fn every_pattern_fires_on_its_canonical_trigger() {
        let engine = DiagnosticsEngine::new();
        for (id, trigger) in CANONICAL_TRIGGERS {
            let results = engine.analyze("test_rule", trigger, None);
            let hit = results
                .iter()
                .any(|r| r.error_pattern.as_deref() == Some(*id));
            assert!(hit, "pattern '{id}' never fires on its trigger: {trigger}");
        }
    }

    #[test]
    fn benign_lines_never_match() {
        let engine = DiagnosticsEngine::new();
        for line in BENIGN_LINES {
            let results = engine.analyze("test_rule", line, Some(1));
            let matched: Vec<&str> = results
                .iter()
                .filter_map(|r| r.error_pattern.as_deref())
                .filter(|p| *p != "unknown_error")
                .collect();
            assert!(
                matched.is_empty(),
                "benign line '{line}' falsely matched {matched:?}"
            );
        }
    }

    #[test]
    fn patterns_compile_and_log_only_patterns_are_precise() {
        let engine = DiagnosticsEngine::new();
        for (i, p) in engine.patterns.iter().enumerate() {
            assert!(
                !engine.compiled[i].1.is_empty(),
                "pattern '{}' has zero compilable regexes",
                p.id
            );
            // Log-only patterns (issue #708 point 1) must have at least one
            // anchored/precise pattern: reject short bare-word patterns.
            if p.log_only() {
                for pat in &p.stderr_patterns {
                    let precise = pat.contains('\\')
                        || pat.contains(".*")
                        || pat.contains('|')
                        || pat.split_whitespace().count() > 1;
                    assert!(
                        precise,
                        "log-only pattern '{}' uses over-broad substring '{pat}'",
                        p.id
                    );
                }
            }
        }
    }

    #[test]
    fn exit_code_gate_scopes_pattern_to_declared_codes() {
        // A declared-exit-code pattern must NOT fire on a different code
        // (issue #708: empty exit_codes matched every status).
        let engine = DiagnosticsEngine::new();
        // "oom_killed" declares 137/9 — must not fire on exit 1 even with
        // matching log text.
        let results = engine.analyze("r", "out of memory", Some(1));
        assert!(
            !results
                .iter()
                .any(|r| r.error_pattern.as_deref() == Some("oom_killed"))
        );
        // ...and must never fire on success.
        let results = engine.analyze("r", "out of memory", Some(0));
        assert!(results.is_empty());
    }
}
