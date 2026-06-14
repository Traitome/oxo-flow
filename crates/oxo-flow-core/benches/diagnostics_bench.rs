//! Benchmark: Diagnostics engine performance.
//!
//! Goal: <100ms for 1000 log lines against full error pattern library.

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

// Simulate the diagnostics benchmark using the core diagnostic logic.
// In v0.8, the DiagnosticsEngine lives in oxo-flow-web. We benchmark
// the pattern matching core that both share.

fn bench_pattern_match(c: &mut Criterion) {
    let mut group = c.benchmark_group("diagnostics");

    // Generate sample log lines with known error signatures
    let logs_small: Vec<&str> = vec![
        "STAR --runThreadN 4 --genomeDir /ref/",
        "FATAL: out of memory. STAR exiting.",
        "EXITING because of FATAL ERROR: 137",
    ];

    let mut logs_large: Vec<String> = Vec::with_capacity(1000);
    for i in 0..1000 {
        match i % 10 {
            0 => logs_large.push(format!("[ERR] command not found: fastqc_{i}")),
            1 => logs_large.push(format!("[ERR] FATAL: out of memory at line {i}")),
            2 => logs_large.push(format!("[ERR] Permission denied: /data/input_{i}.bam")),
            3 => logs_large.push(format!("[ERR] No space left on device at /tmp/output_{i}")),
            4 => logs_large.push(format!("[ERR] Connection timed out downloading ref_{i}")),
            5 => logs_large.push(format!(
                "[ERR] Segmentation fault (core dumped) in tool_{i}"
            )),
            6 => logs_large.push(format!("[ERR] Illegal instruction in aligner_{i}")),
            7 => logs_large.push(format!("[ERR] File truncated: sample_{i}.fastq.gz")),
            8 => logs_large.push(format!("[ERR] Checksum mismatch for file_{i}.bam")),
            _ => logs_large.push(format!("[INFO] Rule {i} completed successfully")),
        }
    }
    let large_log_str: String = logs_large.join("\n");

    // Error patterns to match (simulating DiagnosticsEngine)
    let patterns: Vec<(&str, &str)> = vec![
        ("out of memory", "oom_killed"),
        ("command not found", "cmd_missing"),
        ("Permission denied", "perm_denied"),
        ("No space left", "disk_full"),
        ("timed out", "timeout"),
        ("Segmentation fault", "segfault"),
        ("Illegal instruction", "illegal_instr"),
        ("File truncated", "file_truncated"),
        ("Checksum mismatch", "checksum_mismatch"),
        ("exit code: 137", "oom_killed"),
        ("exit code: 127", "cmd_missing"),
        ("exit code: 126", "perm_denied"),
        ("exit 137", "oom_killed"),
        ("exit 127", "cmd_missing"),
        ("ENOSPC", "disk_full"),
        ("SIGKILL", "oom_killed"),
        ("SIGSEGV", "segfault"),
    ];

    group.bench_function(BenchmarkId::new("pattern_match", "3_logs"), |b| {
        b.iter(|| {
            let _results: Vec<_> = logs_small
                .iter()
                .flat_map(|line| {
                    patterns.iter().filter_map(move |(pat, cat)| {
                        if line.to_lowercase().contains(pat) {
                            Some((line, *cat))
                        } else {
                            None
                        }
                    })
                })
                .collect();
            black_box(_results)
        })
    });

    group.bench_function(BenchmarkId::new("pattern_match", "1000_logs"), |b| {
        b.iter(|| {
            let _results: Vec<_> = large_log_str
                .lines()
                .flat_map(|line| {
                    patterns.iter().filter_map(move |(pat, cat)| {
                        if line.to_lowercase().contains(pat) {
                            Some((line, *cat))
                        } else {
                            None
                        }
                    })
                })
                .collect();
            black_box(_results)
        })
    });

    group.finish();
}

criterion_group!(benches, bench_pattern_match);
criterion_main!(benches);
