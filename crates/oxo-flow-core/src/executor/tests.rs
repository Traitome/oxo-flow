use super::checkpoint::*;
use super::process::*;
use super::security::*;
use crate::rule::{EnvironmentSpec, Resources, Rule, RuleBuilder};
use std::collections::HashMap;

fn make_rule(name: &str, shell: &str) -> Rule {
    Rule {
        name: name.to_string(),
        input: vec![].into(),
        output: vec![].into(),
        shell: Some(shell.to_string()),
        script: None,
        threads: None,
        memory: None,
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
    let result = render_shell_command("cat {input[0]} > {output[0]}", &rule, &HashMap::new());
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
    let result = render_shell_command("cat {input} > {output}", &rule, &HashMap::new());
    assert_eq!(result, "cat a.txt b.txt > out.txt");
}

#[test]
fn render_shell_threads() {
    let rule = Rule {
        name: "test".to_string(),
        threads: Some(8),
        output: vec!["out.bam".to_string()].into(),
        ..Default::default()
    };
    let result = render_shell_command(
        "bwa mem -t {threads} ref.fa > {output[0]}",
        &rule,
        &HashMap::new(),
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
    let result = render_shell_command("bwa mem {config.reference} > {output[0]}", &rule, &values);
    assert_eq!(result, "bwa mem /data/ref.fa > hello.txt");
}

#[tokio::test]
async fn execute_output_index_expansion() {
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
    let rule = Rule {
        name: "output_test".to_string(),
        input: vec![].into(),
        output: vec!["hello_output.txt".to_string()].into(),
        shell: Some("echo hello_oxoflow_{output[0]}".to_string()),
        ..Default::default()
    };
    let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
    assert_eq!(record.status, JobStatus::Success);
    let stdout = record.stdout.unwrap();
    assert!(
        stdout.contains("hello_oxoflow_hello_output.txt"),
        "stdout was: {stdout}"
    );
}

#[test]
fn benchmark_record_creation() {
    let b = BenchmarkRecord {
        rule: "fastqc".to_string(),
        wall_time_secs: 42.5,
        max_memory_mb: Some(1024),
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
fn validate_shell_safety_blocks_dangerous_deletion() {
    assert!(validate_shell_safety("rm -rf /").is_err());
}

#[test]
fn validate_wildcard_injection_blocks_command_substitution() {
    let mut values = HashMap::new();
    values.insert("sample".to_string(), "$(whoami)".to_string());
    assert!(validate_wildcard_injection(&values).is_err());
}
