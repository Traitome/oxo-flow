//! Deterministic diagnostics engine for pipeline execution errors.
//!
//! Matches exit codes and stderr signatures against 30+ known error
//! patterns to produce structured, actionable diagnostic results —
//! including auto-fixable configuration changes when a known remedy
//! exists.
//!
//! Matching semantics (issue #708):
//!
//! - `stderr_patterns` are case-insensitive regexes, compiled once per
//!   engine. Entries must stay specific (loader lines, tool error
//!   prefixes): the old bare-substring matching fired on near-miss words
//!   (`"so."` matched "also.", `"version"` matched every tool banner) and
//!   never matched regex-shaped entries at all.
//! - A pattern with no `exit_codes` fires on any non-zero or unknown exit
//!   code — never on exit 0. A rule that exited 0 has nothing to diagnose.
//!
//! The table is covered by a per-pattern trigger test and a benign-line
//! test below, and is shared with `monitor_agent::is_known_error_pattern`
//! so the AI monitor and the diagnostics service never disagree.

use std::sync::OnceLock;

use regex::Regex;

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

/// One error signature: exit codes + case-insensitive stderr regexes.
struct Pattern {
    id: &'static str,
    category: ErrorCategory,
    exit_codes: Vec<i32>,
    /// Case-insensitive regexes. Only for signatures specific enough that
    /// they cannot appear in benign output — see the module docs.
    stderr_patterns: Vec<Regex>,
    likely_cause: &'static str,
    auto_fixable: bool,
    fix_desc: Option<&'static str>,
    fix_config_path: Option<&'static str>,
}

/// Compile a pattern's static regex table. A malformed entry is a
/// programming error caught by the table-driven tests below, so failing
/// loudly at construction beats silently disabling a diagnostic.
fn compile(patterns: &[&str]) -> Vec<Regex> {
    patterns
        .iter()
        .map(|p| Regex::new(&format!("(?i){p}")).expect("static diagnostics regex must compile"))
        .collect()
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
                stderr_patterns: compile(&["command not found", "no such file"]),
                likely_cause: "Required tool not installed or not in PATH.",
                auto_fixable: true,
                fix_desc: Some("Install missing tool via conda or package manager."),
                fix_config_path: None,
            },
            Pattern {
                id: "segfault",
                category: ErrorCategory::Tool,
                exit_codes: vec![139, 11],
                stderr_patterns: compile(&["segmentation fault", "sigsegv", "segfault"]),
                likely_cause: "Tool crashed with segfault. Possible binary incompatibility or bug.",
                auto_fixable: false,
                fix_desc: Some("Try a different tool version or check for known issues."),
                fix_config_path: None,
            },
            Pattern {
                id: "illegal_instruction",
                category: ErrorCategory::Tool,
                exit_codes: vec![132, 4],
                stderr_patterns: compile(&["illegal instruction", "sigill"]),
                likely_cause: "Binary compiled for different CPU architecture.",
                auto_fixable: false,
                fix_desc: Some("Rebuild tool for this CPU or use a compatible binary."),
                fix_config_path: None,
            },
            Pattern {
                id: "bus_error",
                category: ErrorCategory::Tool,
                exit_codes: vec![138, 10],
                stderr_patterns: compile(&["bus error", "sigbus"]),
                likely_cause: "Memory alignment error. Input file may be on corrupt filesystem.",
                auto_fixable: false,
                fix_desc: Some("Check filesystem integrity. Move data to reliable storage."),
                fix_config_path: None,
            },
            Pattern {
                // A bare "version" matched every tool banner (issue #708);
                // only phrases that state an actual incompatibility count.
                id: "tool_version_mismatch",
                category: ErrorCategory::Tool,
                exit_codes: vec![],
                stderr_patterns: compile(&[
                    "version (mismatch|conflict)",
                    "incompatible.{0,30}version",
                    "version.{0,30}incompatible",
                    "unsupported version",
                    "version.{0,30}unsupported",
                    "version (is )?too (old|new|low)",
                ]),
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
                stderr_patterns: compile(&[
                    "out of memory",
                    "outofmemoryerror",
                    "memoryerror",
                    "bad_alloc",
                    "cannot allocate memory",
                    "memory allocation of",
                    "oom-killed",
                    "oom killer",
                    // Word boundary: "zoom", "room" must not match (#708/#709).
                    "\\boom\\b",
                ]),
                likely_cause: "Process killed due to insufficient memory (OOM).",
                auto_fixable: true,
                fix_desc: Some("Increase memory in [resources] section."),
                fix_config_path: Some("resources.memory"),
            },
            Pattern {
                id: "timeout",
                category: ErrorCategory::Resource,
                exit_codes: vec![124],
                stderr_patterns: compile(&["timed out", "timeout", "time limit"]),
                likely_cause: "Process exceeded time limit.",
                auto_fixable: true,
                fix_desc: Some("Increase time_limit in [resources] section."),
                fix_config_path: Some("resources.time_limit"),
            },
            Pattern {
                id: "disk_full",
                category: ErrorCategory::Resource,
                exit_codes: vec![],
                stderr_patterns: compile(&[
                    "no space left on device",
                    "disk quota exceeded",
                    "enospc",
                ]),
                likely_cause: "Disk full. Free up space or redirect output.",
                auto_fixable: false,
                fix_desc: Some("Clean temp files or increase disk allocation."),
                fix_config_path: None,
            },
            Pattern {
                id: "cpu_limit",
                category: ErrorCategory::Resource,
                exit_codes: vec![],
                stderr_patterns: compile(&[
                    "cpu time limit",
                    "cpu limit",
                    "resource temporarily unavailable",
                    "eagain",
                ]),
                likely_cause: "CPU time limit exceeded or resource temporarily unavailable.",
                auto_fixable: true,
                fix_desc: Some("Increase thread count or CPU allocation."),
                fix_config_path: Some("resources.threads"),
            },
            Pattern {
                id: "ulimit",
                category: ErrorCategory::Resource,
                exit_codes: vec![],
                stderr_patterns: compile(&[
                    "too many open files",
                    "emfile",
                    "enfile",
                    // A bare "ulimit" also matched help text and setup notes;
                    // require it to appear together with a failure word.
                    "ulimit.{0,60}(exceed|too many|denied|cannot|error)",
                ]),
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
                stderr_patterns: compile(&[
                    "no such file or directory",
                    "cannot open",
                    "enoent",
                    // A bare "not found" also matched "command not found"
                    // and "image not found" — require file context (#708).
                    "\\bfiles? not found\\b",
                    "\\bdoes not exist\\b",
                ]),
                likely_cause: "Input file not found. Check paths and wildcard expansion.",
                auto_fixable: false,
                fix_desc: Some("Verify input file exists at expected path."),
                fix_config_path: None,
            },
            Pattern {
                id: "truncated_file",
                category: ErrorCategory::Data,
                exit_codes: vec![1],
                stderr_patterns: compile(&[
                    "truncated",
                    "unexpected end",
                    "premature",
                    "corrupt",
                    "incomplete",
                ]),
                likely_cause: "Input file truncated or corrupted.",
                auto_fixable: false,
                fix_desc: Some("Verify file integrity with checksum. Re-download if needed."),
                fix_config_path: None,
            },
            Pattern {
                id: "empty_file",
                category: ErrorCategory::Data,
                exit_codes: vec![],
                stderr_patterns: compile(&[
                    "empty file",
                    "zero length",
                    "zero bytes",
                    "\\bis empty\\b",
                ]),
                likely_cause: "Input file is empty.",
                auto_fixable: false,
                fix_desc: Some("Check upstream steps produced valid output."),
                fix_config_path: None,
            },
            Pattern {
                // "per base sequence quality.*fail" was written as a regex
                // but matched with `contains` — it could never fire (#708).
                // Compiled as a real regex it catches FastQC's module
                // summary lines again.
                id: "low_quality_fastq",
                category: ErrorCategory::Data,
                exit_codes: vec![],
                stderr_patterns: compile(&[
                    "per base sequence quality\\s+(fail|warn)",
                    "low quality",
                    "poor quality",
                ]),
                likely_cause: "FASTQ files have low quality scores.",
                auto_fixable: true,
                fix_desc: Some("Insert fastp or Trimmomatic step before this rule."),
                fix_config_path: None,
            },
            Pattern {
                id: "gzip_corrupt",
                category: ErrorCategory::Data,
                exit_codes: vec![1],
                stderr_patterns: compile(&[
                    "not in gzip format",
                    "invalid compressed data",
                    "corrupt input",
                    // A bare "gzip" matched every echoed gzip command line;
                    // pair it with a corruption word instead (#708).
                    "gzip.{0,40}(corrupt|invalid)",
                ]),
                likely_cause: "Gzipped file is corrupt or not actually gzipped.",
                auto_fixable: false,
                fix_desc: Some("Verify file is valid gzip: 'gzip -t file.gz'"),
                fix_config_path: None,
            },
            Pattern {
                id: "bam_truncated",
                category: ErrorCategory::Data,
                exit_codes: vec![1],
                stderr_patterns: compile(&["truncated file", "eof marker", "bgzf"]),
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
                stderr_patterns: compile(&[
                    "permission denied",
                    "eacces",
                    "not writable",
                    "access denied",
                ]),
                likely_cause: "Insufficient file permissions.",
                auto_fixable: false,
                fix_desc: Some("Check file permissions. Ensure user has r/w access."),
                fix_config_path: None,
            },
            Pattern {
                id: "broken_pipe",
                category: ErrorCategory::System,
                exit_codes: vec![141, 13],
                stderr_patterns: compile(&["broken pipe", "sigpipe"]),
                likely_cause: "Downstream process exited early, breaking the pipe.",
                auto_fixable: false,
                fix_desc: Some("Check all pipeline commands produce valid output."),
                fix_config_path: None,
            },
            Pattern {
                id: "network_error",
                category: ErrorCategory::System,
                exit_codes: vec![],
                stderr_patterns: compile(&[
                    "connection refused",
                    "could not resolve",
                    "cannot resolve",
                    "name or service not known",
                    "network is unreachable",
                    "connection reset by peer",
                    "failed to connect",
                    "connection timed out",
                    "temporary failure in name resolution",
                ]),
                // Bare "network" and "timeout" were removed: "timeout" also
                // matched CLI flags and duplicated the exit-124 timeout
                // pattern; "network" matched ordinary capacity reports (#708).
                likely_cause: "Network resource unavailable.",
                auto_fixable: false,
                fix_desc: Some("Check network connectivity and remote resource availability."),
                fix_config_path: None,
            },
            Pattern {
                id: "signal_kill",
                category: ErrorCategory::System,
                exit_codes: vec![143, 15],
                stderr_patterns: compile(&["terminated", "sigterm", "\\bkilled\\b"]),
                likely_cause: "Process terminated by external signal.",
                auto_fixable: false,
                fix_desc: Some("Process was externally terminated. Check system logs."),
                fix_config_path: None,
            },
            Pattern {
                id: "signal_interrupt",
                category: ErrorCategory::System,
                exit_codes: vec![130, 2],
                stderr_patterns: compile(&["interrupt", "sigint", "cancelled", "canceled"]),
                likely_cause: "Process was interrupted (Ctrl+C or equivalent).",
                auto_fixable: false,
                fix_desc: Some("Re-run the workflow when ready."),
                fix_config_path: None,
            },
            Pattern {
                id: "shared_library",
                category: ErrorCategory::System,
                exit_codes: vec![127, 1],
                stderr_patterns: compile(&[
                    // Bare "lib" and "so." matched every log line containing
                    // "also.", "person.", any /lib/ path — the loader emits
                    // these canonical lines instead (#708).
                    "error while loading shared libraries",
                    "cannot open shared object",
                    "undefined symbol",
                    "wrong elf class",
                    "glibc[_0-9.]+.{0,5}not found",
                ]),
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
                stderr_patterns: compile(&[
                    "invalid option",
                    "unrecognized",
                    "unknown option",
                    "bad argument",
                ]),
                likely_cause: "Invalid or unrecognized command parameter.",
                auto_fixable: false,
                fix_desc: Some("Check tool documentation for correct parameter spelling."),
                fix_config_path: None,
            },
            Pattern {
                id: "missing_required_param",
                category: ErrorCategory::Config,
                exit_codes: vec![1],
                stderr_patterns: compile(&[
                    // Bare "required"/"missing" matched any log line, e.g.
                    // "all required files were validated" (#708).
                    "must specify",
                    "argument expected",
                    "arguments? (are|is|was) required",
                    "required (argument|parameter|option|value|input)",
                    "missing (required )?(argument|parameter|option|value|input)",
                ]),
                likely_cause: "Required parameter is missing.",
                auto_fixable: false,
                fix_desc: Some("Add the missing parameter to the rule command."),
                fix_config_path: None,
            },
            Pattern {
                id: "wildcard_empty",
                category: ErrorCategory::Config,
                exit_codes: vec![],
                stderr_patterns: compile(&[
                    // Bare "wildcard"/"empty"/"no files" matched ordinary
                    // prose; keep the messages tools actually emit (#708).
                    "no (files|matches|input files)",
                    "wildcard.{0,60}(no match|unmatched|did not match|not resolve|error)",
                    "did not match any (files|inputs)",
                ]),
                likely_cause: "Wildcard pattern matched no files.",
                auto_fixable: false,
                fix_desc: Some("Check file naming matches the wildcard pattern."),
                fix_config_path: None,
            },
            Pattern {
                id: "conda_env_fail",
                category: ErrorCategory::Config,
                exit_codes: vec![1],
                stderr_patterns: compile(&[
                    // Bare "conda"/"environment" matched every log line
                    // mentioning the tool even when the failure was
                    // elsewhere (#708).
                    "condaerror",
                    "condahttperror",
                    "packagesnotfounderror",
                    "environmentnotfound",
                    "environment\\.yml",
                    "solving environment.{0,80}(fail|error|conflict)",
                    "failed to create (the )?(conda )?environment",
                ]),
                likely_cause: "Conda environment creation or activation failed.",
                auto_fixable: false,
                fix_desc: Some("Verify conda is installed and environment name is correct."),
                fix_config_path: None,
            },
            Pattern {
                id: "docker_fail",
                category: ErrorCategory::Config,
                exit_codes: vec![125, 126],
                stderr_patterns: compile(&[
                    // Bare "docker"/"daemon" matched every echoed docker
                    // command; the daemon connection errors carry these
                    // phrases (#708).
                    "docker daemon",
                    "error response from daemon",
                    "no such container",
                    "docker.{0,60}(permission denied|cannot connect)",
                ]),
                likely_cause: "Docker daemon not running or no permission.",
                auto_fixable: false,
                fix_desc: Some("Start Docker daemon or add user to docker group."),
                fix_config_path: None,
            },
            Pattern {
                id: "singularity_fail",
                category: ErrorCategory::Config,
                exit_codes: vec![255],
                stderr_patterns: compile(&["singularity", "apptainer", "image not found"]),
                likely_cause: "Singularity/Apptainer image not found or daemon issue.",
                auto_fixable: false,
                fix_desc: Some("Pull the container image or check Singularity installation."),
                fix_config_path: None,
            },
            Pattern {
                id: "shell_syntax",
                category: ErrorCategory::Config,
                exit_codes: vec![2],
                stderr_patterns: compile(&["syntax error", "unexpected token", "parse error"]),
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
                stderr_patterns: compile(&[
                    "failed to open index",
                    "could not open index file",
                    "no index available",
                    "bai file not found",
                    "index file is missing",
                    "could not retrieve index",
                ]),
                likely_cause: "BAM index file (.bai) not found. Most tools (samtools, IGV) require a sorted and indexed BAM.",
                auto_fixable: true,
                fix_desc: Some("Generate index with: samtools index <file.bam>"),
                fix_config_path: None,
            },
        ];
        Self { patterns }
    }

    /// Shared process-wide engine — the static pattern table's regexes are
    /// compiled once and reused by every caller (the web diagnostics
    /// service and the AI monitor agent).
    pub fn global() -> &'static Self {
        static GLOBAL: OnceLock<DiagnosticsEngine> = OnceLock::new();
        GLOBAL.get_or_init(Self::new)
    }

    /// Analyze log output and optional exit code for a given rule.
    /// Returns zero or more diagnostic results.
    ///
    /// A rule that exited 0 is never diagnosed: empty-`exit_codes`
    /// patterns otherwise fired on any exit status as soon as one regex
    /// matched (issue #708).
    pub fn analyze(
        &self,
        rule_name: &str,
        log_output: &str,
        exit_code: Option<i32>,
    ) -> Vec<DiagnosticResult> {
        let mut results = Vec::new();

        for p in &self.patterns {
            let exit_match = match exit_code {
                Some(0) => false,
                Some(ec) => p.exit_codes.is_empty() || p.exit_codes.contains(&ec),
                None => true,
            };
            let stderr_match = p.stderr_patterns.iter().any(|re| re.is_match(log_output));

            if exit_match && stderr_match {
                let relevant: Vec<String> = log_output
                    .lines()
                    .filter(|line| p.stderr_patterns.iter().any(|re| re.is_match(line)))
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

    /// Canonical trigger line per pattern — the table-driven gate from
    /// issue #708. Every pattern must fire on a real-world signature, so a
    /// dead entry (like the old `"per base sequence quality.*fail"` that
    /// could never match as a substring) fails CI instead of shipping.
    const CANONICAL_TRIGGERS: &[(&str, &str)] = &[
        ("command_not_found", "/bin/bash: fastqc: command not found"),
        (
            "segfault",
            "Program terminated with signal SIGSEGV, Segmentation fault",
        ),
        ("illegal_instruction", "Illegal instruction (core dumped)"),
        ("bus_error", "Bus error (core dumped)"),
        (
            "tool_version_mismatch",
            "ERROR: incompatible library version: libhts required 1.10, found 1.4",
        ),
        (
            "oom_killed",
            "Out of memory: Killed process 12345 (star) total-vm:1234kB",
        ),
        ("timeout", "Command timed out after 3600 seconds"),
        ("disk_full", "OSError: [Errno 28] No space left on device"),
        (
            "cpu_limit",
            "CPU time limit exceeded, terminating the process",
        ),
        (
            "ulimit",
            "awk: too many open files while opening 'input.fastq'",
        ),
        (
            "file_not_found",
            "Error: no such file or directory: /data/sample.fastq",
        ),
        ("truncated_file", "gzip: stdin: unexpected end of file"),
        ("empty_file", "Error: input FASTQ file is empty"),
        ("low_quality_fastq", ">>Per base sequence quality\tFAIL"),
        (
            "gzip_corrupt",
            "gzip: stdin: invalid compressed data--format violated",
        ),
        (
            "bam_truncated",
            "[E::bgzf_read] Read block failed: EOF marker is absent",
        ),
        (
            "permission_denied",
            "touch: cannot touch '/data/out.txt': Permission denied",
        ),
        ("broken_pipe", "head: error writing to stdout: Broken pipe"),
        (
            "network_error",
            "curl: (7) Failed to connect to ftp.example.com port 21: Connection refused",
        ),
        ("signal_kill", "Killed"),
        ("signal_interrupt", "KeyboardInterrupt"),
        (
            "shared_library",
            "error while loading shared libraries: libhts.so.2: cannot open shared object file",
        ),
        ("invalid_param", "fastqc: unrecognized option '--threads=4'"),
        (
            "missing_required_param",
            "error: the following arguments are required: -i/--input",
        ),
        (
            "wildcard_empty",
            "No files found matching pattern: data/{sample}.fastq",
        ),
        (
            "conda_env_fail",
            "CondaError: Cannot link an item that already exists",
        ),
        (
            "docker_fail",
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock",
        ),
        (
            "singularity_fail",
            "FATAL: image not found: /containers/bwa.sif",
        ),
        (
            "shell_syntax",
            "/bin/sh: -c: line 1: syntax error: unexpected token `fi'",
        ),
        (
            "bam_index_missing",
            "[E::idx_find_and_load] Could not retrieve index file for 'sample.bam'",
        ),
    ];

    /// Ordinary tool output that must never be diagnosed at any exit code:
    /// version banners, prose near-misses ("also", "zoom"), and lines that
    /// merely mention a tool.
    const BENIGN_LOG_LINES: &[&str] = &[
        "STAR version: 2.7.10a compiled: 2023-01-15T00:00:00",
        "bwa 0.7.17-r1188 is also supported here.",
        "Loading library dependencies ... done",
        "The zoom level was set to 5.",
        "docker version 24.0.7, build afdd53b",
        "conda 24.1.2 activated environment: rnaseq",
        "Input files found: 6",
        "Reading so. many records took a while.",
        "INFO: processing 4 samples with 8 threads",
    ];

    #[test]
    fn every_pattern_fires_on_its_canonical_trigger() {
        let engine = DiagnosticsEngine::new();
        for p in &engine.patterns {
            let (_, line) = CANONICAL_TRIGGERS
                .iter()
                .find(|(id, _)| *id == p.id)
                .unwrap_or_else(|| panic!("pattern '{}' has no canonical trigger row", p.id));
            let exit = p.exit_codes.first().copied();
            let results = engine.analyze("rule", line, exit);
            let hits: Vec<&str> = results
                .iter()
                .filter_map(|r| r.error_pattern.as_deref())
                .collect();
            assert!(
                hits.contains(&p.id),
                "pattern '{}' did not fire on canonical trigger {line:?} (exit {exit:?}); got {hits:?}",
                p.id
            );
        }
    }

    #[test]
    fn every_pattern_has_a_canonical_trigger_row() {
        let engine = DiagnosticsEngine::new();
        assert_eq!(
            engine.patterns.len(),
            CANONICAL_TRIGGERS.len(),
            "a pattern was added or removed without updating CANONICAL_TRIGGERS"
        );
        for p in &engine.patterns {
            assert!(
                CANONICAL_TRIGGERS.iter().any(|(id, _)| *id == p.id),
                "pattern '{}' has no canonical trigger row",
                p.id
            );
        }
    }

    #[test]
    fn benign_lines_stay_silent_at_every_exit_code() {
        let engine = DiagnosticsEngine::new();
        let exits = [
            None,
            Some(0),
            Some(1),
            Some(2),
            Some(13),
            Some(124),
            Some(125),
            Some(127),
            Some(137),
            Some(139),
            Some(143),
            Some(255),
        ];
        for line in BENIGN_LOG_LINES {
            for exit in exits {
                let results = engine.analyze("rule", line, exit);
                let hits: Vec<&str> = results
                    .iter()
                    .filter_map(|r| r.error_pattern.as_deref())
                    .filter(|id| *id != "unknown_error")
                    .collect();
                assert!(
                    hits.is_empty(),
                    "benign line {line:?} at exit {exit:?} triggered {hits:?}"
                );
            }
        }
    }

    #[test]
    fn exit_zero_is_never_diagnosed() {
        let engine = DiagnosticsEngine::new();
        // A strong signature ("no space left on device") must stay silent
        // for a rule that exited 0 — it succeeded (issue #708).
        let results = engine.analyze(
            "sort",
            "OSError: [Errno 28] No space left on device",
            Some(0),
        );
        assert!(results.is_empty(), "exit 0 produced {results:?}");
    }

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
