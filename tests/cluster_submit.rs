//! Phase-1 regression tests for issue #74 — `cluster submit` honesty.
//!
//! 1. Wildcards expand into one script per rule *instance*, with the same
//!    instance names `run` produces.
//! 2. The dependency wrapper captures a bare scheduler job id instead of
//!    chaining raw submit output into `--dependency=afterok:`.
//! 3. `--walltime` / `--extra-arg` reach the generated directives.

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

const SCATTER: &str = r#"
[workflow]
name = "scatter"

[[sample_groups]]
name = "batch"
samples = ["S1", "S2", "S3"]

[[rules]]
name = "align"
input = ["data/{sample}.fq"]
output = ["aln/{sample}.bam"]
shell = "mkdir -p aln && cp data/{sample}.fq aln/{sample}.bam"

[[rules]]
name = "stats"
input = ["aln/{sample}.bam"]
output = ["stats/{sample}.txt"]
shell = "mkdir -p stats && wc -l aln/{sample}.bam > stats/{sample}.txt"
"#;

/// Locate a workspace binary (mirrors the helper in cluster_backend.rs).
fn workspace_bin(name: &str) -> PathBuf {
    let target_dir = std::env::current_exe()
        .expect("cannot find current test executable path")
        .parent()
        .expect("no parent dir for test exe")
        .parent()
        .expect("no grandparent dir for test exe")
        .to_path_buf();
    let candidate = target_dir.join(name);
    if candidate.exists() {
        return candidate;
    }
    panic!(
        "could not find binary '{name}' in target directory; run `cargo build --workspace` first"
    );
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock-scheduler")
}

fn write_scatter(dir: &Path) {
    std::fs::write(dir.join("wf.oxoflow"), SCATTER).unwrap();
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    for s in ["S1", "S2", "S3"] {
        std::fs::write(data.join(format!("{s}.fq")), format!(">{s}\nACGT\n")).unwrap();
    }
}

fn submit(dir: &Path, extra: &[&str]) -> PathBuf {
    submit_with_backend(dir, "slurm", extra)
}

fn submit_with_backend(dir: &Path, backend: &str, extra: &[&str]) -> PathBuf {
    let out_dir = dir.join("cluster_scripts");
    let mut args: Vec<&str> = vec!["cluster", "submit", "wf.oxoflow", "-b", backend];
    args.extend_from_slice(extra);
    let status = StdCommand::new(workspace_bin("oxo-flow"))
        .args(&args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "cluster submit failed");
    out_dir
}

#[test]
fn cluster_submit_expands_wildcards_into_per_instance_scripts() {
    let dir = tempfile::tempdir().unwrap();
    write_scatter(dir.path());
    let out_dir = submit(dir.path(), &["--with-dependencies"]);

    // Two template rules × three samples = six scripts (plus submit.sh).
    let mut scripts: Vec<String> = std::fs::read_dir(&out_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != "submit.sh")
        .collect();
    scripts.sort();
    assert_eq!(
        scripts,
        vec![
            "align_batch_S1.sh",
            "align_batch_S2.sh",
            "align_batch_S3.sh",
            "stats_batch_S1.sh",
            "stats_batch_S2.sh",
            "stats_batch_S3.sh",
        ],
        "expected one script per rule instance"
    );

    // No unexpanded placeholder survives into a submitted script.
    for name in &scripts {
        let body = std::fs::read_to_string(out_dir.join(name)).unwrap();
        assert!(
            !body.contains("{sample}"),
            "unexpanded {{sample}} in {name}:\n{body}"
        );
    }
    let s1 = std::fs::read_to_string(out_dir.join("align_batch_S1.sh")).unwrap();
    assert!(s1.contains("cp data/S1.fq aln/S1.bam"), "got:\n{s1}");

    // Dependencies chain per sample, not across samples.
    let wrapper = std::fs::read_to_string(out_dir.join("submit.sh")).unwrap();
    assert!(
        wrapper.contains(
            "JOB_IDS[stats_batch_S2]=$(oxo_submit --dependency=afterok:${JOB_IDS[align_batch_S2]}"
        ),
        "per-sample dependency chain missing:\n{wrapper}"
    );
    assert!(
        !wrapper.contains("afterok:${JOB_IDS[align_batch_S1]}:${JOB_IDS[align_batch_S2]}"),
        "stats must not wait on every sample's align:\n{wrapper}"
    );
}

/// The instance names `cluster submit` writes must match the ones `run`
/// plans — phase 2 keys its run directory off them.
#[test]
fn cluster_submit_instance_names_match_dry_run() {
    let dir = tempfile::tempdir().unwrap();
    write_scatter(dir.path());
    let out_dir = submit(dir.path(), &[]);

    let preview = StdCommand::new(workspace_bin("oxo-flow"))
        .args(["dry-run", "wf.oxoflow"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let preview = String::from_utf8_lossy(&preview.stdout).into_owned()
        + &String::from_utf8_lossy(&preview.stderr);

    for name in ["align_batch_S1", "stats_batch_S3"] {
        assert!(
            out_dir.join(format!("{name}.sh")).exists(),
            "cluster submit did not write {name}.sh"
        );
        assert!(
            preview.contains(name),
            "dry-run does not plan '{name}':\n{preview}"
        );
    }
}

#[test]
fn submit_wrapper_captures_bare_job_ids() {
    let dir = tempfile::tempdir().unwrap();
    write_scatter(dir.path());
    let out_dir = submit(dir.path(), &["--with-dependencies"]);
    let wrapper = std::fs::read_to_string(out_dir.join("submit.sh")).unwrap();

    // SLURM needs --parsable: plain `sbatch` prints "Submitted batch job N",
    // which chained into --dependency=afterok: as a whole sentence.
    assert!(
        wrapper.contains("sbatch --parsable"),
        "wrapper must submit with --parsable:\n{wrapper}"
    );
    // Every per-rule capture goes through the helper. The old shape —
    // `JOB_IDS[align]=$(sbatch script.sh)` — put a whole sentence in the
    // next rule's --dependency flag.
    assert!(
        !wrapper.contains("]=$(sbatch"),
        "no raw sbatch capture may remain:\n{wrapper}"
    );
    assert_eq!(
        wrapper.matches("=$(oxo_submit").count(),
        6,
        "every rule instance must submit through the helper:\n{wrapper}"
    );

    // End-to-end through the mock scheduler: the wrapper is valid bash and
    // every rule reports a bare numeric id. NOTE: the mock's `sbatch` prints
    // its sentence to stderr and a bare id to stdout, so this run alone would
    // still pass without --parsable — the static assertions above are what
    // pin that. This case covers wrapper syntax and id capture.
    let scheduler_state = dir.path().join("scheduler-state");
    std::fs::create_dir_all(&scheduler_state).unwrap();
    let path = format!(
        "{}:{}",
        fixtures_dir().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = StdCommand::new("bash")
        .arg(out_dir.join("submit.sh"))
        .current_dir(dir.path())
        .env("PATH", path)
        .env("MOCK_SCHEDULER_DIR", &scheduler_state)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "wrapper failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let submitted: Vec<&str> = stdout
        .lines()
        .filter(|l| l.contains("as job ID:"))
        .collect();
    assert_eq!(submitted.len(), 6, "expected six submissions:\n{stdout}");
    for line in submitted {
        let id = line.rsplit("job ID:").next().unwrap().trim();
        assert!(
            !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()),
            "job id is not bare digits in line '{line}'"
        );
    }
}

/// Each backend gets its own id-capture body; all four must be valid bash
/// and use that scheduler's submit command.
#[test]
fn submit_wrapper_is_valid_bash_for_every_backend() {
    for (backend, submit_cmd) in [
        ("slurm", "sbatch --parsable"),
        ("pbs", "qsub"),
        ("sge", "qsub"),
        ("lsf", "bsub"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        write_scatter(dir.path());
        let out_dir = submit_with_backend(dir.path(), backend, &["--with-dependencies"]);
        let wrapper_path = out_dir.join("submit.sh");
        let wrapper = std::fs::read_to_string(&wrapper_path).unwrap();
        assert!(
            wrapper.contains(submit_cmd),
            "{backend} wrapper must submit with '{submit_cmd}':\n{wrapper}"
        );

        let syntax = StdCommand::new("bash")
            .arg("-n")
            .arg(&wrapper_path)
            .output()
            .unwrap();
        assert!(
            syntax.status.success(),
            "{backend} wrapper is not valid bash: {}",
            String::from_utf8_lossy(&syntax.stderr)
        );
    }
}

#[test]
fn cluster_submit_surfaces_walltime_and_extra_args() {
    let dir = tempfile::tempdir().unwrap();
    write_scatter(dir.path());
    let out_dir = submit(
        dir.path(),
        &[
            "-q",
            "compute",
            "--walltime",
            "24h",
            "--extra-arg",
            "--exclusive",
            "--extra-arg",
            "--constraint=haswell",
        ],
    );
    let body = std::fs::read_to_string(out_dir.join("align_batch_S1.sh")).unwrap();

    // Duration strings convert; `HH:MM:SS` would pass through as-is.
    assert!(body.contains("#SBATCH --time=1-00:00:00"), "got:\n{body}");
    assert!(body.contains("#SBATCH --partition=compute"), "got:\n{body}");
    assert!(body.contains("#SBATCH --exclusive"), "got:\n{body}");
    assert!(
        body.contains("#SBATCH --constraint=haswell"),
        "repeated --extra-arg must all land:\n{body}"
    );
}
