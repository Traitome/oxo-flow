//! Concurrency and reuse guarantees for shared workflows (issue #158, idea 2).
//!
//! One workflow file must be safely runnable by many users and many workdirs
//! at the same time, while a single workdir stays exclusive. These tests pin
//! that contract end-to-end through the compiled `oxo-flow` binary:
//!
//! 1. same workflow, different workdirs, truly concurrent — both succeed,
//!    each workdir carries its own outputs + checkpoint, no cross-talk;
//! 2. different HOME users sharing one module cache with a git-pinned
//!    `[[include]]` from a local file:// repo — both succeed concurrently
//!    (the cache clone is CloneLock-serialized, issue #136);
//! 3. same workdir, concurrent — the second run fails fast with the
//!    workdir-lock message instead of racing the checkpoint (issue #70);
//! 4. workflow in a read-only directory — concurrent runs with per-analysis
//!    `-d` workdirs both succeed (nothing writes next to the workflow file);
//! 5. sequential reuse — a second run from a fresh workdir is unaffected by
//!    the first (independent checkpoint, rules execute fresh).
//!
//! Kept in a dedicated crate so parallel sessions can own other integration
//! tests independently (each integration-test crate compiles and links on
//! its own).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Locate a workspace binary by name from the target directory.
///
/// This handles the case where binaries are defined in workspace sub-crates
/// rather than the root package, which means `CARGO_BIN_EXE_*` env vars
/// are not automatically set.
fn workspace_bin(name: &str) -> PathBuf {
    // Cargo sets OUT_DIR for build scripts and CARGO_MANIFEST_DIR for the package.
    // For integration tests, we can derive the target dir from the test binary location.
    let mut target_dir = std::env::current_exe()
        .expect("cannot find current test executable path")
        .parent()
        .expect("no parent dir for test exe")
        .parent()
        .expect("no grandparent dir for test exe")
        .to_path_buf();

    // Try the binary directly in the target/debug (or target/release) directory.
    let candidate = target_dir.join(name);
    if candidate.exists() {
        return candidate;
    }

    // On Windows, binaries have a .exe extension.
    let candidate_exe = target_dir.join(format!("{name}.exe"));
    if candidate_exe.exists() {
        return candidate_exe;
    }

    // Fall back to the deps subdirectory.
    target_dir = target_dir.join("deps");
    let candidate = target_dir.join(name);
    if candidate.exists() {
        return candidate;
    }

    panic!(
        "could not find binary '{name}' in target directory; \
         run `cargo build --workspace` first"
    );
}

fn oxo_flow_bin() -> &'static str {
    "oxo-flow"
}

/// Write a minimal workflow whose single rule records its working directory
/// (after an optional sleep) into `out.txt` — the workdir-relative shell
/// path makes each run's output prove which workdir it executed in.
fn write_pwd_workflow(dir: &Path, name: &str, sleep_secs: u64) -> PathBuf {
    let wf = dir.join(format!("{name}.oxoflow"));
    let sleep = if sleep_secs > 0 {
        format!("sleep {sleep_secs} && ")
    } else {
        String::new()
    };
    fs::write(
        &wf,
        format!(
            "[workflow]\nname = \"{name}\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"{sleep}pwd > {{output}}\"\n"
        ),
    )
    .unwrap();
    wf
}

/// Spawn a run of `wf` in `workdir` with extra env vars, capturing output.
fn spawn_run(wf: &Path, workdir: &Path, envs: &[(&str, &str)]) -> Child {
    let mut cmd = Command::new(workspace_bin(oxo_flow_bin()));
    cmd.args(["run", wf.to_str().unwrap(), "-d", workdir.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.spawn().expect("failed to spawn oxo-flow run")
}

/// Wait for a child up to `deadline`; on expiry kill it and return `None`.
fn wait_with_deadline(child: &mut Child, deadline: Duration) -> Option<Output> {
    let start = Instant::now();
    loop {
        match child.try_wait().expect("failed to poll child") {
            Some(status) => {
                // Read the remaining piped output.
                let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
                let mut stderr = std::io::BufReader::new(child.stderr.take().unwrap());
                use std::io::Read;
                let mut out_buf = Vec::new();
                let mut err_buf = Vec::new();
                stdout.read_to_end(&mut out_buf).unwrap();
                stderr.read_to_end(&mut err_buf).unwrap();
                return Some(Output {
                    status,
                    stdout: out_buf,
                    stderr: err_buf,
                });
            }
            None => {
                if start.elapsed() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Poll until a path exists or the deadline passes.
fn wait_until_exists(path: &Path, deadline: Duration) -> bool {
    let start = Instant::now();
    while !path.exists() {
        if start.elapsed() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

fn canonical(s: &Path) -> String {
    fs::canonicalize(s)
        .unwrap_or_else(|e| panic!("cannot canonicalize {}: {e}", s.display()))
        .to_string_lossy()
        .into_owned()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

// ─── Scenario 1: same workflow, different workdirs, concurrent ──────────

/// One workflow file, two simultaneous runs in two workdirs: both must
/// succeed, each workdir must get its own checkpoint and its own outputs,
/// and the outputs must not be cross-contaminated — each out.txt must
/// contain exactly its own workdir's path (the shell `pwd` runs in the
/// workdir).
#[test]
fn cli_same_workflow_different_workdirs_concurrent() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_pwd_workflow(dir.path(), "shared", 1);
    let wd_a = dir.path().join("wdA");
    let wd_b = dir.path().join("wdB");
    fs::create_dir_all(&wd_a).unwrap();
    fs::create_dir_all(&wd_b).unwrap();

    // Spawn both ~simultaneously, then wait for both.
    let mut child_a = spawn_run(&wf, &wd_a, &[]);
    let mut child_b = spawn_run(&wf, &wd_b, &[]);
    let out_a = wait_with_deadline(&mut child_a, Duration::from_secs(30))
        .expect("run A must finish within 30s");
    let out_b = wait_with_deadline(&mut child_b, Duration::from_secs(30))
        .expect("run B must finish within 30s");

    assert!(
        out_a.status.success(),
        "run A must succeed: {}",
        stderr_of(&out_a)
    );
    assert!(
        out_b.status.success(),
        "run B must succeed: {}",
        stderr_of(&out_b)
    );

    // Each workdir has its own checkpoint and output.
    for wd in [&wd_a, &wd_b] {
        assert!(
            wd.join(".oxo-flow/checkpoint.json").exists(),
            "{} must have its own checkpoint",
            wd.display()
        );
        assert!(
            wd.join("out.txt").exists(),
            "{} must have its output",
            wd.display()
        );
    }

    // No cross-contamination: each out.txt names exactly its own workdir.
    let out_a_text = fs::read_to_string(wd_a.join("out.txt")).unwrap();
    let out_b_text = fs::read_to_string(wd_b.join("out.txt")).unwrap();
    let want_a = canonical(&wd_a);
    let want_b = canonical(&wd_b);
    assert_eq!(
        out_a_text.trim(),
        want_a,
        "run A output must contain workdir A's path"
    );
    assert_eq!(
        out_b_text.trim(),
        want_b,
        "run B output must contain workdir B's path"
    );
    assert!(
        !out_a_text.contains(&want_b),
        "run A output must not mention workdir B: {out_a_text}"
    );
    assert!(
        !out_b_text.contains(&want_a),
        "run B output must not mention workdir A: {out_b_text}"
    );
}

// ─── Scenario 2: different HOME, shared module cache, git-pinned include ─

fn run_git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git must be available to build the include repo");
    assert!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Two runs with distinct HOME env vars (different users) share one module
/// cache via `OXO_FLOW_MODULE_CACHE`; the workflow carries a git-pinned
/// `[[include]]` from a LOCAL file:// repo, so both processes clone into the
/// same cache dir concurrently — the CloneLock (issue #136) must serialize
/// them and both must succeed.
#[test]
fn cli_different_home_shared_module_cache_git_pinned_include() {
    let dir = tempfile::tempdir().unwrap();

    // A local git repo holding the included module. Filler files widen the
    // clone window so both runs genuinely contend on the cache entry.
    let repo_dir = dir.path().join("module-repo");
    fs::create_dir_all(repo_dir.join("data")).unwrap();
    fs::write(
        repo_dir.join("module.toml"),
        "[workflow]\nname = \"module\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"mod_gen\"\noutput = [\"mod_out.txt\"]\nshell = \"echo module > {output}\"\n",
    )
    .unwrap();
    for i in 0..200 {
        fs::write(repo_dir.join(format!("data/f{i:04}.txt")), "filler\n").unwrap();
    }
    run_git(&repo_dir, &["init", "-q"]);
    run_git(&repo_dir, &["config", "user.email", "test@example.com"]);
    run_git(&repo_dir, &["config", "user.name", "test"]);
    run_git(&repo_dir, &["add", "-A"]);
    run_git(&repo_dir, &["commit", "-qm", "init"]);
    run_git(&repo_dir, &["branch", "-M", "main"]);

    // Shared workflow (outside both HOMEs) with the pinned include.
    let wf = dir.path().join("homeiso.oxoflow");
    let repo_url = format!("file://{}", repo_dir.display());
    fs::write(
        &wf,
        format!(
            "[workflow]\nname = \"homeiso\"\nversion = \"1.0.0\"\n\n[[include]]\npath = \"module.toml\"\nrepo = \"{repo_url}\"\nref = \"main\"\n\n[[rules]]\nname = \"main_gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {{output}}\"\ndepends_on = [\"mod_gen\"]\n"
        ),
    )
    .unwrap();

    // Two users with distinct HOMEs, sharing ONE module cache.
    let home_a = dir.path().join("homeA");
    let home_b = dir.path().join("homeB");
    fs::create_dir_all(&home_a).unwrap();
    fs::create_dir_all(&home_b).unwrap();
    let cache = dir.path().join("module-cache");
    fs::create_dir_all(&cache).unwrap();
    let wd_a = dir.path().join("wdA");
    let wd_b = dir.path().join("wdB");
    fs::create_dir_all(&wd_a).unwrap();
    fs::create_dir_all(&wd_b).unwrap();

    let mut child_a = spawn_run(
        &wf,
        &wd_a,
        &[
            ("HOME", home_a.to_str().unwrap()),
            ("OXO_FLOW_MODULE_CACHE", cache.to_str().unwrap()),
        ],
    );
    let mut child_b = spawn_run(
        &wf,
        &wd_b,
        &[
            ("HOME", home_b.to_str().unwrap()),
            ("OXO_FLOW_MODULE_CACHE", cache.to_str().unwrap()),
        ],
    );
    let out_a = wait_with_deadline(&mut child_a, Duration::from_secs(120))
        .expect("run A must finish within 120s");
    let out_b = wait_with_deadline(&mut child_b, Duration::from_secs(120))
        .expect("run B must finish within 120s");

    assert!(
        out_a.status.success(),
        "run A (user A) must succeed: {}",
        stderr_of(&out_a)
    );
    assert!(
        out_b.status.success(),
        "run B (user B) must succeed: {}",
        stderr_of(&out_b)
    );

    // The included module's rule executed in BOTH workdirs, alongside the
    // host rule — no cache corruption, no cross-user bleed.
    for wd in [&wd_a, &wd_b] {
        assert!(
            wd.join("out.txt").exists(),
            "{} must have the host rule output",
            wd.display()
        );
        assert!(
            wd.join("mod_out.txt").exists(),
            "{} must have the included module's output (git-pinned include)",
            wd.display()
        );
        assert!(
            wd.join(".oxo-flow/checkpoint.json").exists(),
            "{} must have its own checkpoint",
            wd.display()
        );
    }
}

// ─── Scenario 3: same workdir, concurrent — second fails fast ───────────

/// Two runs on the SAME workdir: the first holds the exclusive workdir lock
/// (issue #70) for the whole run; the second must fail fast — nonzero exit,
/// within a bounded time, naming the lock — instead of racing the same
/// checkpoint. The first run must be unaffected and complete normally.
#[test]
fn cli_same_workdir_second_run_fails_fast_with_lock() {
    let dir = tempfile::tempdir().unwrap();
    // Long enough that the second run's fast-fail demonstrably happens
    // while the first is still executing.
    let wf = write_pwd_workflow(dir.path(), "exclusive", 6);

    let mut run1 = Command::new(workspace_bin(oxo_flow_bin()))
        .args(["run", wf.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn first run");

    // Wait until run1 actually holds the workdir lock (the lock file is
    // created on acquisition), then launch the contender.
    assert!(
        wait_until_exists(&dir.path().join(".oxo-flow/lock"), Duration::from_secs(10)),
        "run1 must acquire the workdir lock within 10s"
    );

    let mut run2 = Command::new(workspace_bin(oxo_flow_bin()))
        .args(["run", wf.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn second run");

    let start = Instant::now();
    let out2 = wait_with_deadline(&mut run2, Duration::from_secs(20))
        .expect("the second run must exit within 20s — the lock fails fast");
    let elapsed = start.elapsed();

    assert!(
        !out2.status.success(),
        "the second run on the same workdir must exit nonzero, got: {}",
        stderr_of(&out2)
    );
    let err2 = stderr_of(&out2);
    assert!(
        err2.contains("workdir is locked by another oxo-flow process"),
        "the second run must name the workdir lock, got: {err2}"
    );
    assert!(
        err2.contains("wait for it to finish"),
        "the lock error must advise waiting, got: {err2}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "the second run must fail within seconds, took {elapsed:?}"
    );

    // The first run is still executing (holds the lock) when the second
    // fails — this is a real fast-fail, not a post-hoc collision.
    assert!(
        run1.try_wait().unwrap().is_none(),
        "run1 must still be running while run2 fails fast"
    );

    let out1 = wait_with_deadline(&mut run1, Duration::from_secs(30))
        .expect("run1 must finish within 30s");
    assert!(
        out1.status.success(),
        "run1 (the lock holder) must complete normally: {}",
        stderr_of(&out1)
    );
    assert!(
        dir.path().join(".oxo-flow/checkpoint.json").exists(),
        "the lock holder must write its checkpoint"
    );
    assert!(
        dir.path().join("out.txt").exists(),
        "the lock holder must write its output"
    );
}

// ─── Scenario 4: read-only shared workflow location ─────────────────────

fn running_as_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

/// A workflow in a read-only (chmod 555) directory is the recommended
/// central-repo pattern: two concurrent runs pointing at the same file with
/// per-analysis `-d` workdirs must both succeed — the engine never writes
/// next to the workflow file.
#[test]
fn cli_readonly_workflow_dir_concurrent_runs() {
    if running_as_root() {
        eprintln!("skipping: running as root, chmod 555 does not restrict root");
        return;
    }
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let wf_dir = dir.path().join("central");
    fs::create_dir(&wf_dir).unwrap();
    let wf = write_pwd_workflow(&wf_dir, "central", 1);
    fs::set_permissions(&wf_dir, fs::Permissions::from_mode(0o555)).unwrap();

    let wd_a = dir.path().join("wdA");
    let wd_b = dir.path().join("wdB");
    fs::create_dir_all(&wd_a).unwrap();
    fs::create_dir_all(&wd_b).unwrap();

    let mut child_a = spawn_run(&wf, &wd_a, &[]);
    let mut child_b = spawn_run(&wf, &wd_b, &[]);
    let out_a = wait_with_deadline(&mut child_a, Duration::from_secs(30))
        .expect("run A must finish within 30s");
    let out_b = wait_with_deadline(&mut child_b, Duration::from_secs(30))
        .expect("run B must finish within 30s");

    // Restore permissions so the tempdir can be cleaned up.
    fs::set_permissions(&wf_dir, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(
        out_a.status.success(),
        "run A from the read-only workflow dir must succeed: {}",
        stderr_of(&out_a)
    );
    assert!(
        out_b.status.success(),
        "run B from the read-only workflow dir must succeed: {}",
        stderr_of(&out_b)
    );
    for wd in [&wd_a, &wd_b] {
        assert!(
            wd.join("out.txt").exists(),
            "{} must have its output",
            wd.display()
        );
        assert!(
            wd.join(".oxo-flow/checkpoint.json").exists(),
            "{} must have its own checkpoint",
            wd.display()
        );
    }
    // The read-only dir must be untouched: no .oxo-flow appeared next to
    // the shared workflow file.
    assert!(
        !wf_dir.join(".oxo-flow").exists(),
        "nothing may be written next to the shared workflow file"
    );
}

// ─── Scenario 5: sequential reuse from a fresh workdir ──────────────────

/// Running the same workflow again from a DIFFERENT workdir must be
/// unaffected by the first run: the second checkpoint is fresh, the rule
/// executes again (not skipped), and each workdir keeps its own outputs.
#[test]
fn cli_sequential_reuse_from_fresh_workdir() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_pwd_workflow(dir.path(), "reuse", 0);

    let wd_a = dir.path().join("wdA");
    let wd_b = dir.path().join("wdB");
    fs::create_dir_all(&wd_a).unwrap();
    fs::create_dir_all(&wd_b).unwrap();

    let mut run1 = spawn_run(&wf, &wd_a, &[]);
    let out1 = wait_with_deadline(&mut run1, Duration::from_secs(30))
        .expect("first run must finish within 30s");
    assert!(
        out1.status.success(),
        "first run must succeed: {}",
        stderr_of(&out1)
    );
    assert!(
        wd_a.join(".oxo-flow/checkpoint.json").exists(),
        "first run must leave its checkpoint in wdA"
    );
    assert_eq!(
        fs::read_to_string(wd_a.join("out.txt")).unwrap().trim(),
        canonical(&wd_a),
        "first run's output must name wdA"
    );

    // Second run from a fresh workdir: fresh checkpoint, rule executes.
    let mut run2 = spawn_run(&wf, &wd_b, &[]);
    let out2 = wait_with_deadline(&mut run2, Duration::from_secs(30))
        .expect("second run must finish within 30s");
    assert!(
        out2.status.success(),
        "second run must succeed: {}",
        stderr_of(&out2)
    );
    assert!(
        stderr_of(&out2).contains("1 succeeded"),
        "second run must execute the rule fresh (new workdir = fresh checkpoint): {}",
        stderr_of(&out2)
    );
    assert!(
        wd_b.join(".oxo-flow/checkpoint.json").exists(),
        "second run must leave its own checkpoint in wdB"
    );
    assert_eq!(
        fs::read_to_string(wd_b.join("out.txt")).unwrap().trim(),
        canonical(&wd_b),
        "second run's output must name wdB — unaffected by the first run"
    );
}
