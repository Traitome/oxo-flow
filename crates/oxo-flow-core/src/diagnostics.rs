//! Deterministic diagnostics engine for pipeline execution errors and self-healing.
//!
//! Matches exit codes, OS signals, runtime resource telemetry (RSS, duration),
//! and stderr signatures against known error patterns to produce structured,
//! actionable diagnostics and bounded `RepairAction` plans.

use std::path::PathBuf;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// High-level category of execution failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ErrorCategory {
    Tool,
    Resource,
    Data,
    System,
    Config,
}

/// A structured diagnostic report for a failed or abnormal rule execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticResult {
    pub rule: String,
    pub error_pattern: Option<String>,
    pub category: ErrorCategory,
    pub likely_cause: String,
    pub suggestions: Vec<String>,
    pub auto_fixable: bool,
    pub fix_action: Option<FixAction>,
    pub repair_action: Option<RepairAction>,
    pub relevant_log_lines: Vec<String>,
}

/// Suggested configuration or command adjustment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixAction {
    pub description: String,
    pub config_change: Option<ConfigChange>,
    pub command: Option<String>,
}

/// Targeted key-value configuration modification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigChange {
    pub path: String,
    pub old_value: String,
    pub new_value: String,
}

/// Deterministic, bounded self-healing remediation plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RepairAction {
    /// Rescale allocated memory or time limit by given multipliers.
    ScaleResources {
        memory_multiplier: f64,
        time_multiplier: f64,
    },
    /// Inject a deterministic adapter rule into the execution plan.
    InsertBridgeRule {
        rule_name: String,
        command: String,
        input: Vec<String>,
        output: Vec<String>,
    },
    /// Redirect scratch workspace to a distinct path under disk pressure.
    RerouteScratch { tmp_dir: PathBuf },
}

/// Rich contextual input for causal fault isolation and telemetry analysis.
#[derive(Debug, Clone, Default)]
pub struct DiagnosticContext {
    pub rule_name: String,
    pub log_output: String,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub max_rss_mb: Option<u64>,
    pub cpu_seconds: Option<f64>,
    pub duration_ms: Option<u64>,
    pub header_mismatch: Option<String>,
}

impl DiagnosticContext {
    /// Create a basic context from rule name, log, and exit code.
    #[must_use]
    pub fn new(
        rule_name: impl Into<String>,
        log_output: impl Into<String>,
        exit_code: Option<i32>,
    ) -> Self {
        Self {
            rule_name: rule_name.into(),
            log_output: log_output.into(),
            exit_code,
            ..Default::default()
        }
    }

    /// Attach OS signal info (e.g. 9 for SIGKILL, 11 for SIGSEGV).
    #[must_use]
    pub fn with_signal(mut self, signal: i32) -> Self {
        self.signal = Some(signal);
        self
    }

    /// Attach benchmark telemetry metrics.
    #[must_use]
    pub fn with_telemetry(
        mut self,
        max_rss_mb: Option<u64>,
        cpu_secs: Option<f64>,
        duration_ms: Option<u64>,
    ) -> Self {
        self.max_rss_mb = max_rss_mb;
        self.cpu_seconds = cpu_secs;
        self.duration_ms = duration_ms;
        self
    }

    /// Attach biological header introspection finding.
    #[must_use]
    pub fn with_header_mismatch(mut self, mismatch: impl Into<String>) -> Self {
        self.header_mismatch = Some(mismatch.into());
        self
    }
}

/// Internal pattern rule definition.
struct Pattern {
    id: &'static str,
    category: ErrorCategory,
    exit_codes: Vec<i32>,
    stderr_patterns: Vec<Regex>,
    likely_cause: &'static str,
    auto_fixable: bool,
    fix_desc: Option<&'static str>,
    fix_config_path: Option<&'static str>,
}

fn compile(patterns: &[&str]) -> Vec<Regex> {
    patterns
        .iter()
        .map(|p| Regex::new(&format!("(?i){p}")).expect("static diagnostics regex must compile"))
        .collect()
}

/// Global causal diagnostics engine.
pub struct DiagnosticsEngine {
    patterns: Vec<Pattern>,
}

impl DiagnosticsEngine {
    /// Construct a new diagnostics engine with standard pattern definitions.
    #[must_use]
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
                id: "unbound_wildcard",
                category: ErrorCategory::Config,
                exit_codes: vec![],
                stderr_patterns: compile(&["contains unbound wildcard"]),
                likely_cause: "An output pattern still contains a wildcard that has no value for this rule instance.",
                auto_fixable: false,
                fix_desc: Some(
                    "Declare a value source for the wildcard (sample_pattern, [[values]], [[sample_groups]], input_groups) or remove it from the output pattern.",
                ),
                fix_config_path: None,
            },
            Pattern {
                id: "conda_env_fail",
                category: ErrorCategory::Config,
                exit_codes: vec![1],
                stderr_patterns: compile(&[
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

    /// Access the global singleton instance.
    #[must_use]
    pub fn global() -> &'static Self {
        static GLOBAL: OnceLock<DiagnosticsEngine> = OnceLock::new();
        GLOBAL.get_or_init(Self::new)
    }

    /// Analyze log output and optional exit code for a given rule.
    pub fn analyze(
        &self,
        rule_name: &str,
        log_output: &str,
        exit_code: Option<i32>,
    ) -> Vec<DiagnosticResult> {
        let ctx = DiagnosticContext::new(rule_name, log_output, exit_code);
        self.analyze_context(&ctx)
    }

    /// Analyze a rich execution context including signals, telemetry, and headers.
    pub fn analyze_context(&self, ctx: &DiagnosticContext) -> Vec<DiagnosticResult> {
        // A rule that exited 0 has nothing to diagnose (issue #708).
        if ctx.exit_code == Some(0) {
            return Vec::new();
        }

        let mut results = Vec::new();

        // 1. Biological header mismatch check (TYPE-002, TYPE-003, etc.)
        if let Some(mismatch) = &ctx.header_mismatch {
            let likely_cause = format!("Biological contract violation: {mismatch}");
            let is_unsorted = mismatch.to_lowercase().contains("sort");
            let is_unindexed = mismatch.to_lowercase().contains("index");

            let (fix_desc, repair) = if is_unsorted {
                (
                    "Insert 'samtools sort' adapter rule before consumer.",
                    Some(RepairAction::InsertBridgeRule {
                        rule_name: format!("{}_sort_bridge", ctx.rule_name),
                        command: "samtools sort -@ {threads} -o {output} {input}".into(),
                        input: vec!["{input}".into()],
                        output: vec!["{output}.sorted.bam".into()],
                    }),
                )
            } else if is_unindexed {
                (
                    "Insert 'samtools index' companion builder before consumer.",
                    Some(RepairAction::InsertBridgeRule {
                        rule_name: format!("{}_index_bridge", ctx.rule_name),
                        command: "samtools index -@ {threads} {input}".into(),
                        input: vec!["{input}".into()],
                        output: vec!["{input}.bai".into()],
                    }),
                )
            } else {
                ("Align biological input formats and references.", None)
            };

            results.push(DiagnosticResult {
                rule: ctx.rule_name.clone(),
                error_pattern: Some("bio_contract_violation".into()),
                category: ErrorCategory::Data,
                likely_cause,
                suggestions: vec![fix_desc.into()],
                auto_fixable: repair.is_some(),
                fix_action: Some(FixAction {
                    description: fix_desc.into(),
                    config_change: None,
                    command: None,
                }),
                repair_action: repair,
                relevant_log_lines: vec![mismatch.clone()],
            });
        }

        // 2. Telemetry and signal-based causal classification (e.g. SIGKILL 137, SIGXCPU 140, ENOSPC)
        let is_explicit_sigkill_oom = ctx.signal == Some(9) || ctx.exit_code == Some(137);
        let is_explicit_sigxcpu_timeout = ctx.signal == Some(24) || ctx.exit_code == Some(140);

        // 3. Match against static pattern table
        for p in &self.patterns {
            let exit_match = match ctx.exit_code {
                Some(0) => false,
                Some(ec) => p.exit_codes.is_empty() || p.exit_codes.contains(&ec),
                None => true,
            };
            let stderr_match = p
                .stderr_patterns
                .iter()
                .any(|re| re.is_match(&ctx.log_output));

            if exit_match && stderr_match {
                let relevant: Vec<String> = ctx
                    .log_output
                    .lines()
                    .filter(|line| p.stderr_patterns.iter().any(|re| re.is_match(line)))
                    .take(10)
                    .map(|s| s.to_string())
                    .collect();

                let repair_action = match p.id {
                    "oom_killed" => Some(RepairAction::ScaleResources {
                        memory_multiplier: 2.0,
                        time_multiplier: 1.0,
                    }),
                    "timeout" => Some(RepairAction::ScaleResources {
                        memory_multiplier: 1.0,
                        time_multiplier: 2.0,
                    }),
                    "disk_full" => Some(RepairAction::RerouteScratch {
                        tmp_dir: std::env::temp_dir().join("oxoflow_scratch"),
                    }),
                    "bam_index_missing" => Some(RepairAction::InsertBridgeRule {
                        rule_name: format!("{}_index_bridge", ctx.rule_name),
                        command: "samtools index -@ {threads} {input}".into(),
                        input: vec!["{input}".into()],
                        output: vec!["{input}.bai".into()],
                    }),
                    _ => None,
                };

                results.push(DiagnosticResult {
                    rule: ctx.rule_name.clone(),
                    error_pattern: Some(p.id.to_string()),
                    category: p.category,
                    likely_cause: p.likely_cause.to_string(),
                    suggestions: vec![
                        p.fix_desc
                            .unwrap_or("Manual investigation required.")
                            .to_string(),
                    ],
                    auto_fixable: p.auto_fixable || repair_action.is_some(),
                    fix_action: p.fix_desc.map(|desc| FixAction {
                        description: desc.to_string(),
                        config_change: p.fix_config_path.map(|path| ConfigChange {
                            path: path.to_string(),
                            old_value: "current".into(),
                            new_value: "increase".into(),
                        }),
                        command: None,
                    }),
                    repair_action,
                    relevant_log_lines: relevant,
                });
            }
        }

        // If OOM signal/telemetry triggered but no pattern matched yet, inject OOM diagnostic
        if is_explicit_sigkill_oom
            && !results
                .iter()
                .any(|r| r.error_pattern.as_deref() == Some("oom_killed"))
        {
            results.push(DiagnosticResult {
                rule: ctx.rule_name.clone(),
                error_pattern: Some("oom_killed".into()),
                category: ErrorCategory::Resource,
                likely_cause:
                    "Process killed by OS/cgroup due to memory limit (OOM / SIGKILL 137).".into(),
                suggestions: vec!["Increase memory in [resources] section.".into()],
                auto_fixable: true,
                fix_action: Some(FixAction {
                    description: "Scale memory allocation.".into(),
                    config_change: Some(ConfigChange {
                        path: "resources.memory".into(),
                        old_value: "current".into(),
                        new_value: "increase".into(),
                    }),
                    command: None,
                }),
                repair_action: Some(RepairAction::ScaleResources {
                    memory_multiplier: 2.0,
                    time_multiplier: 1.0,
                }),
                relevant_log_lines: vec!["Exit code 137 or SIGKILL received".into()],
            });
        }

        // If timeout signal/telemetry triggered but not caught yet
        if is_explicit_sigxcpu_timeout
            && !results
                .iter()
                .any(|r| r.error_pattern.as_deref() == Some("timeout"))
        {
            results.push(DiagnosticResult {
                rule: ctx.rule_name.clone(),
                error_pattern: Some("timeout".into()),
                category: ErrorCategory::Resource,
                likely_cause: "Process exceeded walltime or CPU time limit.".into(),
                suggestions: vec!["Increase time_limit in [resources] section.".into()],
                auto_fixable: true,
                fix_action: Some(FixAction {
                    description: "Scale time limit.".into(),
                    config_change: Some(ConfigChange {
                        path: "resources.time_limit".into(),
                        old_value: "current".into(),
                        new_value: "increase".into(),
                    }),
                    command: None,
                }),
                repair_action: Some(RepairAction::ScaleResources {
                    memory_multiplier: 1.0,
                    time_multiplier: 2.0,
                }),
                relevant_log_lines: vec!["Exit code 124/140 or SIGXCPU received".into()],
            });
        }

        // Generic fallback for non-zero exit without pattern match
        if results.is_empty() && ctx.exit_code.unwrap_or(0) != 0 {
            results.push(DiagnosticResult {
                rule: ctx.rule_name.clone(),
                error_pattern: Some("unknown_error".into()),
                category: ErrorCategory::Tool,
                likely_cause: format!(
                    "Process exited with code {}. Check logs.",
                    ctx.exit_code.unwrap()
                ),
                suggestions: vec![
                    "Review full execution log.".into(),
                    "Check input file integrity.".into(),
                ],
                auto_fixable: false,
                fix_action: None,
                repair_action: None,
                relevant_log_lines: ctx
                    .log_output
                    .lines()
                    .take(20)
                    .map(|s| s.to_string())
                    .collect(),
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
            "unbound_wildcard",
            "output pattern 'results/{sample}/out.bam' contains unbound wildcard {sample} for this instance — declare a value source (sample_pattern, [[values]], [[sample_groups]], input_groups) or remove it",
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

    const BENIGN_LOG_LINES: &[&str] = &[
        "STAR version: 2.7.10a compiled: release-build",
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
        let results = engine.analyze(
            "sort",
            "OSError: [Errno 28] No space left on device",
            Some(0),
        );
        assert!(results.is_empty(), "exit 0 produced {results:?}");
    }

    #[test]
    fn test_oom_repair_action() {
        let engine = DiagnosticsEngine::new();
        let ctx = DiagnosticContext::new("star_align", "some log", Some(137)).with_signal(9);
        let results = engine.analyze_context(&ctx);
        assert!(!results.is_empty());
        assert_eq!(results[0].error_pattern.as_deref(), Some("oom_killed"));
        assert_eq!(
            results[0].repair_action,
            Some(RepairAction::ScaleResources {
                memory_multiplier: 2.0,
                time_multiplier: 1.0,
            })
        );
    }

    #[test]
    fn test_timeout_repair_action() {
        let engine = DiagnosticsEngine::new();
        let ctx = DiagnosticContext::new("bwa_mem", "time limit exceeded", Some(124));
        let results = engine.analyze_context(&ctx);
        assert!(!results.is_empty());
        assert_eq!(results[0].error_pattern.as_deref(), Some("timeout"));
        assert_eq!(
            results[0].repair_action,
            Some(RepairAction::ScaleResources {
                memory_multiplier: 1.0,
                time_multiplier: 2.0,
            })
        );
    }

    #[test]
    fn test_header_mismatch_bridge_repair() {
        let engine = DiagnosticsEngine::new();
        let ctx = DiagnosticContext::new("call_variants", "reading bam...", Some(1))
            .with_header_mismatch("BAM sort order is queryname, required coordinate");
        let results = engine.analyze_context(&ctx);
        assert!(!results.is_empty());
        assert_eq!(
            results[0].error_pattern.as_deref(),
            Some("bio_contract_violation")
        );
        assert!(matches!(
            results[0].repair_action,
            Some(RepairAction::InsertBridgeRule { .. })
        ));
    }
}
