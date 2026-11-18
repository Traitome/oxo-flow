//! Cross-backend conda-family lifecycle evidence (issue #1155, ledger WF-05).
//!
//! Proves, with NO real toolchain and NO network, that conda, mamba, and pixi
//! travel the SAME production lifecycle — create -> use -> decommission/gc —
//! instead of divergent copies:
//!
//! - `oxo-flow run` drives `EnvironmentResolver` (setup, verify, wrap) and the
//!   executor's env-store re-home exactly as in production; only the CLI
//!   binaries are fakes on a temp PATH. Each fake records its argv and
//!   materializes the prefix/marker the resolver checks, so a closed loop of
//!   generate -> execute -> verify -> use is exercised end to end.
//! - conda and mamba commands must be identical modulo the binary token (plus
//!   the documented conda-only `--no-capture-output` on the wrap), asserting
//!   one shared `conda_family_*` builder rather than two hand-written copies.
//! - pixi's create -> use -> store re-home -> `env gc` chain is exercised
//!   through the executor and the CLI, including the checkpoint-reference and
//!   age gates and the documented broken-link rebuild.
//!
//! What this does NOT prove: real solver/network behavior, real conda/pixi CLI
//! compatibility, container backends, or `env gc` for conda/mamba (they are
//! deliberately not store-managed — their decommission step is `env remove`,
//! exercised here through the verify-failure teardown path).

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use oxo_flow_core::environment::CondaBackend;

/// Command builder for the CLI binary under test.
fn oxo_flow() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

/// Quote a path for embedding in a generated `/bin/sh` script.
fn sh_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

/// Write a fake conda-family binary.
///
/// The script records every invocation (`<binary>\t<arg>...`) and emulates the
/// CLI contract the engine depends on: `--version`, `env create|update` (makes
/// `bin/fake-tool`), `env remove`, and `run -n|-p ... bash -c` (sets
/// `CONDA_PREFIX` and the env bin on PATH, then execs the command). All paths
/// are embedded literally because the rule shell runs with a cleared
/// environment by design (issue #1113) — a fake reading its log path from an
/// env var would fail exactly where the engine is supposed to be exercised.
///
/// `broken_once`, when present, makes the next create leave an EMPTY `bin/`
/// (the interrupted-transaction shape) so verification fails once and the
/// engine's teardown-and-retry path runs.
fn write_fake_conda_family(
    fakebin: &Path,
    binary: &str,
    log: &Path,
    root: &Path,
    broken_once: &Path,
) {
    std::fs::create_dir_all(fakebin).unwrap();
    let script = format!(
        r##"#!/bin/sh
bin=$(basename "$0")
printf '%s' "$bin" >> {log}
for a in "$@"; do printf '\t%s' "$a" >> {log}; done
printf '\n' >> {log}
case "$1" in
  --version) echo "$bin 99.0"; exit 0 ;;
  env)
    shift
    sub=$1; shift
    name=""; prefix=""
    while [ $# -gt 0 ]; do
      case "$1" in
        -n) name=$2; shift 2 ;;
        -p) prefix=$2; shift 2 ;;
        *) shift ;;
      esac
    done
    [ -n "$prefix" ] || prefix={root}/envs/$name
    case "$sub" in
      create|update)
        mkdir -p "$prefix/bin"
        if [ -f {broken} ]; then rm -f {broken}; exit 0; fi
        printf 'x' > "$prefix/bin/fake-tool"
        chmod 755 "$prefix/bin/fake-tool"
        exit 0 ;;
      remove) rm -rf "$prefix"; exit 0 ;;
      list) echo "# conda environments:"; exit 0 ;;
      *) exit 0 ;;
    esac ;;
  run)
    shift
    while [ $# -gt 0 ]; do
      case "$1" in
        --*) shift ;;
        -n) name=$2; shift 2 ;;
        -p) prefix=$2; shift 2 ;;
        bash)
          shift
          if [ "$1" = "-c" ]; then
            shift
            [ -n "$prefix" ] || prefix={root}/envs/$name
            CONDA_PREFIX="$prefix" PATH="$prefix/bin:$PATH" exec bash -c "$1"
          fi
          break ;;
        *) break ;;
      esac
    done
    [ -n "$prefix" ] || prefix={root}/envs/$name
    CONDA_PREFIX="$prefix" PATH="$prefix/bin:$PATH" exec "$@" ;;
esac
exit 0
"##,
        log = sh_quote(log),
        root = sh_quote(root),
        broken = sh_quote(broken_once),
    );
    write_executable(&fakebin.join(binary), &script);
}

/// Write a fake `pixi`: `--version`, `install --manifest-path` (materializes
/// `.pixi/envs/default/bin/fake-pixi-tool` beside the manifest), and
/// `run [--as-is] --manifest-path ... bash -c` (execs the rule command with
/// that env's bin first on PATH).
fn write_fake_pixi(fakebin: &Path, log: &Path) {
    std::fs::create_dir_all(fakebin).unwrap();
    let script = format!(
        r##"#!/bin/sh
bin=$(basename "$0")
printf '%s' "$bin" >> {log}
for a in "$@"; do printf '\t%s' "$a" >> {log}; done
printf '\n' >> {log}
case "$1" in
  --version) echo "$bin 99.0"; exit 0 ;;
  install)
    shift; man=""
    while [ $# -gt 0 ]; do
      case "$1" in
        --manifest-path) man=$2; shift 2 ;;
        *) shift ;;
      esac
    done
    dir=$(dirname "$man")
    mkdir -p "$dir/.pixi/envs/default/bin"
    printf 'y' > "$dir/.pixi/envs/default/bin/fake-pixi-tool"
    chmod 755 "$dir/.pixi/envs/default/bin/fake-pixi-tool"
    exit 0 ;;
  run)
    shift; man=""
    while [ $# -gt 0 ]; do
      case "$1" in
        --as-is) shift ;;
        --manifest-path) man=$2; shift 2 ;;
        bash)
          shift
          if [ "$1" = "-c" ]; then
            shift
            dir=$(dirname "$man")
            PATH="$dir/.pixi/envs/default/bin:$PATH" exec bash -c "$1"
          fi
          break ;;
        *) break ;;
      esac
    done
    exit 0 ;;
esac
exit 0
"##,
        log = sh_quote(log),
    );
    write_executable(&fakebin.join("pixi"), &script);
}

fn write_executable(path: &Path, script: &str) {
    std::fs::write(path, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

/// Minimal conda/mamba workflow: the rule runs only if the environment was
/// created and the wrap put its `bin/` first on PATH.
fn write_conda_family_workflow(dir: &Path, backend: &str) {
    std::fs::create_dir_all(dir.join("envs")).unwrap();
    std::fs::write(
        dir.join("envs/tools.yaml"),
        "name: tools\ndependencies:\n  - python=3.11\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("wf.oxoflow"),
        format!(
            r#"
[workflow]
name = "{backend}_lifecycle"

[[rules]]
name = "probe"
shell = "printf '%s' \"$CONDA_PREFIX\" > used.txt && command -v fake-tool > tool.txt"

[rules.environment]
{backend} = "envs/tools.yaml"
"#
        ),
    )
    .unwrap();
}

/// Minimal pixi workflow: uses the manifest beside the workflow file.
fn write_pixi_workflow(dir: &Path) {
    std::fs::write(
        dir.join("pixi.toml"),
        "[project]\nname = \"fake\"\nchannels = [\"conda-forge\"]\nplatforms = [\"linux-64\", \"osx-arm64\"]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("wf.oxoflow"),
        r#"
[workflow]
name = "pixi_lifecycle"

[[rules]]
name = "probe"
shell = "command -v fake-pixi-tool > tool.txt && printf '%s' \"$PATH\" > used.txt"

[rules.environment]
pixi = "pixi.toml"
"#,
    )
    .unwrap();
}

/// Spawn the CLI with the fake bin dir first on PATH (no real toolchain or
/// network involved) and the test's own AI-session dir.
fn run_oxo(dir: &Path, fakebin: &Path, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
    let path = format!(
        "{}:{}",
        fakebin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut cmd = oxo_flow();
    cmd.current_dir(dir)
        .args(args)
        .env("PATH", path)
        .env("NO_COLOR", "1")
        .env("OXO_FLOW_AI_SESSION_DIR", dir.join("ai_sessions"));
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    cmd.output().expect("oxo-flow binary runs")
}

/// Parse the fake's argv log into one `Vec<argv>` per invocation.
fn invocations(log: &Path) -> Vec<Vec<String>> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.split('\t').map(str::to_string).collect())
        .collect()
}

/// All invocations whose argv starts with `prefix` (argv[0] + leading args).
fn matching<'a>(log: &'a [Vec<String>], prefix: &[&str]) -> Vec<&'a Vec<String>> {
    log.iter()
        .filter(|argv| {
            argv.len() >= prefix.len()
                && argv
                    .iter()
                    .zip(prefix)
                    .all(|(actual, expected)| actual == expected)
        })
        .collect()
}

fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} must succeed; status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The engine-derived env name for the fixture YAML: `tools-<8 hex>`.
fn assert_engine_env_name(name: &str) {
    let hash = name
        .strip_prefix("tools-")
        .unwrap_or_else(|| panic!("env name must derive from the YAML's `name:` field: {name}"));
    assert_eq!(
        hash.len(),
        8,
        "content-hash suffix must be 8 hex chars: {name}"
    );
    assert!(
        hash.chars().all(|c| c.is_ascii_hexdigit()),
        "content-hash suffix must be hex: {name}"
    );
}

/// The create argv both backends must share, minus the binary token.
fn create_tail(log: &[Vec<String>], binary: &str) -> Vec<String> {
    let creates = matching(log, &[binary, "env", "create"]);
    assert_eq!(
        creates.len(),
        1,
        "{binary} must run exactly one successful `env create` in the happy path; argv log: {log:?}"
    );
    creates[0][1..].to_vec()
}

/// conda and mamba lifecycle, create -> use -> decommission -> reuse, all
/// through the production resolver/executor path (`oxo-flow run`).
///
/// The first setup deliberately leaves an empty `bin/` so verification fails
/// once: the engine must tear the broken env down (`env remove`) and rebuild
/// it — the conda-family decommission step — before the rule may use it.
#[test]
fn conda_lifecycle_create_use_teardown_and_reuse_share_one_path() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let fakebin = dir.path().join("fakebin");
    let log = dir.path().join("conda.log");
    let root = dir.path().join("conda_root");
    let broken = dir.path().join("broken-once");
    write_fake_conda_family(&fakebin, "conda", &log, &root, &broken);
    write_conda_family_workflow(&ws, "conda");
    std::fs::write(&broken, b"1").unwrap();

    let first = run_oxo(&ws, &fakebin, &["run", "wf.oxoflow"], &[]);
    assert_success(&first, "conda lifecycle run");

    let log = invocations(&log);
    // The rule could only run inside the env: CONDA_PREFIX is set and the
    // env's own bin/ is first on PATH.
    let used = std::fs::read_to_string(ws.join("used.txt")).expect("rule wrote used.txt");
    let tool = std::fs::read_to_string(ws.join("tool.txt")).expect("rule resolved fake-tool");
    assert!(
        tool.trim().starts_with(&used),
        "the wrapped rule must see the env's own bin first (tool={tool}, prefix={used})"
    );

    // One derived name, shared by every command of the lifecycle.
    let creates = matching(&log, &["conda", "env", "create"]);
    assert_eq!(
        creates.len(),
        2,
        "one broken create + one rebuild expected; log: {log:?}"
    );
    let name = creates[0][4].clone(); // conda env create -n <name> -f ...
    assert_engine_env_name(&name);
    assert_eq!(
        creates[0], creates[1],
        "the rebuild must be the SAME command as the first create"
    );
    assert!(
        used.ends_with(&name),
        "rule must use the env the engine created ({used} vs {name})"
    );

    // The shared teardown helper ran between the two creates.
    let removes = matching(&log, &["conda", "env", "remove"]);
    assert_eq!(
        removes.len(),
        1,
        "verification failure must tear down the env exactly once; log: {log:?}"
    );
    assert_eq!(
        removes[0].last().map(String::as_str),
        Some("-y"),
        "teardown must be the non-interactive `env remove -n <name> -y` form"
    );

    // Use step: the wrap is the shared conda-family wrap.
    let wraps = matching(&log, &["conda", "run", "--no-capture-output"]);
    assert_eq!(
        wraps.len(),
        1,
        "one wrapped rule run expected; log: {log:?}"
    );
    assert_eq!(wraps[0][3], "-n");
    assert_eq!(wraps[0][4], name);

    // Reuse: a second run verifies the existing env in place and never
    // re-creates it (fresh process = cold cache; the checkpoint is removed so
    // the completed rule actually re-runs instead of being skipped).
    std::fs::remove_file(ws.join(".oxo-flow/checkpoint.json")).unwrap();
    let second = run_oxo(&ws, &fakebin, &["run", "wf.oxoflow"], &[]);
    assert_success(&second, "conda reuse run");
    let after = invocations(&dir.path().join("conda.log"));
    assert_eq!(
        matching(&after, &["conda", "env", "create"]).len(),
        2,
        "a repeat run must reuse the environment, not recreate it"
    );
}

/// Same lifecycle for mamba — the same shared builder with a different binary
/// token, proven operationally (not just by string comparison).
#[test]
fn mamba_lifecycle_create_use_teardown_and_reuse_share_one_path() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let fakebin = dir.path().join("fakebin");
    let log = dir.path().join("mamba.log");
    let root = dir.path().join("mamba_root");
    let broken = dir.path().join("broken-once");
    write_fake_conda_family(&fakebin, "mamba", &log, &root, &broken);
    write_conda_family_workflow(&ws, "mamba");
    std::fs::write(&broken, b"1").unwrap();

    let first = run_oxo(&ws, &fakebin, &["run", "wf.oxoflow"], &[]);
    assert_success(&first, "mamba lifecycle run");

    let log = invocations(&log);
    let used = std::fs::read_to_string(ws.join("used.txt")).expect("rule wrote used.txt");
    let creates = matching(&log, &["mamba", "env", "create"]);
    assert_eq!(creates.len(), 2, "broken create + rebuild; log: {log:?}");
    let name = creates[0][4].clone();
    assert_engine_env_name(&name);
    assert!(used.ends_with(&name));

    let removes = matching(&log, &["mamba", "env", "remove"]);
    assert_eq!(
        removes.len(),
        1,
        "teardown must run through the shared helper"
    );

    // mamba's wrap carries NO `--no-capture-output` (native mamba rejects it).
    let wraps: Vec<_> = matching(&log, &["mamba", "run"])
        .into_iter()
        .filter(|argv| {
            argv.last()
                .is_some_and(|script| script.contains("export PATH=\"$CONDA_PREFIX/bin:$PATH\""))
        })
        .collect();
    assert_eq!(
        wraps.len(),
        1,
        "one wrapped rule run expected; log: {log:?}"
    );
    assert_ne!(wraps[0][2], "--no-capture-output");
    assert_eq!(wraps[0][2], "-n");

    std::fs::remove_file(ws.join(".oxo-flow/checkpoint.json")).unwrap();
    let second = run_oxo(&ws, &fakebin, &["run", "wf.oxoflow"], &[]);
    assert_success(&second, "mamba reuse run");
    let after = invocations(&dir.path().join("mamba.log"));
    assert_eq!(
        matching(&after, &["mamba", "env", "create"]).len(),
        2,
        "a repeat run must reuse the environment, not recreate it"
    );
}

/// The cross-backend claim itself: for the same YAML, conda and mamba execute
/// the SAME argv shape (modulo the binary token and the documented conda-only
/// `--no-capture-output`), i.e. one `conda_family_*` builder, not two copies.
#[test]
fn conda_and_mamba_execute_one_shared_command_shape() {
    let dir = tempfile::tempdir().unwrap();

    let mut logs = Vec::new();
    for backend in ["conda", "mamba"] {
        let ws = dir.path().join(backend);
        std::fs::create_dir_all(&ws).unwrap();
        let fakebin = dir.path().join(format!("{backend}_bin"));
        let log = dir.path().join(format!("{backend}.log"));
        let root = dir.path().join(format!("{backend}_root"));
        let broken = dir.path().join(format!("{backend}_never"));
        write_fake_conda_family(&fakebin, backend, &log, &root, &broken);
        write_conda_family_workflow(&ws, backend);
        let out = run_oxo(&ws, &fakebin, &["run", "wf.oxoflow"], &[]);
        assert_success(&out, &format!("{backend} shape run"));
        logs.push((backend, invocations(&log)));
    }

    let conda = &logs[0].1;
    let mamba = &logs[1].1;

    // create: identical argv after the binary token.
    assert_eq!(
        create_tail(conda, "conda"),
        create_tail(mamba, "mamba"),
        "conda and mamba must create the environment through one builder"
    );

    // verify / wrap: the two `run` invocations differ only by their script
    // body (verify probes CONDA_PREFIX/bin; the wrap runs the rule), so pick
    // them by the marker the shared builders emit.
    let run_tail = |log: &[Vec<String>], binary: &str, marker: &str| {
        let runs: Vec<_> = matching(log, &[binary, "run"])
            .into_iter()
            .filter(|argv| argv.last().is_some_and(|script| script.contains(marker)))
            .map(|argv| argv[1..].to_vec())
            .collect();
        // Verify runs once before setup (cold-cache probe) and once after —
        // every instance must be the same generated command.
        assert!(!runs.is_empty(), "{binary} `{marker}` invocations: {log:?}");
        assert!(runs.iter().all(|tail| *tail == runs[0]));
        runs[0].clone()
    };
    let verify_marker = "test -d \"$CONDA_PREFIX/bin\"";
    assert_eq!(
        run_tail(conda, "conda", verify_marker),
        run_tail(mamba, "mamba", verify_marker),
        "conda and mamba must verify through one builder"
    );

    let wrap_marker = "export PATH=\"$CONDA_PREFIX/bin:$PATH\"";
    let mut conda_wrap = run_tail(conda, "conda", wrap_marker);
    let mamba_wrap = run_tail(mamba, "mamba", wrap_marker);
    assert_eq!(conda_wrap[1], "--no-capture-output");
    conda_wrap.remove(1);
    assert_eq!(
        conda_wrap, mamba_wrap,
        "conda and mamba must wrap through one builder modulo the conda-only flag"
    );
}

/// pixi lifecycle: create -> use -> store re-home -> `env gc` (checkpoint
/// reference gate, then the age gate), plus the documented broken-link
/// auto-rebuild after collection. All through production code: the executor
/// runs the setup/wrap it generated and re-homes the env with
/// `rehome_into_store`; `env gc` is the real CLI subcommand.
#[test]
fn pixi_lifecycle_create_use_store_rehome_and_env_gc() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let fakebin = dir.path().join("fakebin");
    let log = dir.path().join("pixi.log");
    let store = dir.path().join("store");
    std::fs::create_dir_all(&store).unwrap();
    write_fake_pixi(&fakebin, &log);
    write_pixi_workflow(&ws);

    let store_env = [("OXO_FLOW_ENV_STORE", store.to_str().unwrap())];

    // create + use + re-home
    let run = run_oxo(&ws, &fakebin, &["run", "wf.oxoflow"], &store_env);
    assert_success(&run, "pixi lifecycle run");

    let entries: Vec<PathBuf> = std::fs::read_dir(&store)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1, "one store dir expected: {entries:?}");
    let store_dir = &entries[0];
    let store_name = store_dir
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        store_name.starts_with("pixi-"),
        "store dir must be kind-prefixed: {store_name}"
    );
    assert!(
        store_dir.join(".oxo-env-ready").is_file(),
        "re-home must write the ready marker"
    );

    // The in-tree path became a symlink into the store; the rule used it.
    let in_tree = ws.join(".pixi");
    assert!(
        std::fs::symlink_metadata(&in_tree)
            .unwrap()
            .file_type()
            .is_symlink(),
        ".pixi must be a symlink into the shared store"
    );
    assert_eq!(
        std::fs::read_link(&in_tree).unwrap().as_path(),
        store_dir.as_path(),
        "the symlink must target this run's store entry"
    );
    let tool = std::fs::read_to_string(ws.join("tool.txt")).unwrap();
    assert!(tool.contains("fake-pixi-tool"));

    // The checkpoint records the store reference `env gc` honors.
    let checkpoint = std::fs::read_to_string(ws.join(".oxo-flow/checkpoint.json")).unwrap();
    let checkpoint: serde_json::Value = serde_json::from_str(&checkpoint).unwrap();
    assert_eq!(
        checkpoint["env_store_refs"]["probe"]["store_dir"],
        serde_json::Value::String(store_name.clone()),
        "the checkpoint must carry the store reference"
    );

    // gc gate 1: referenced by the checkpoint -> kept, even at age 0.
    let dry = run_oxo(
        &ws,
        &fakebin,
        &["env", "gc", "--dry-run", "--min-age-days", "0"],
        &store_env,
    );
    assert_success(&dry, "env gc dry run with a live checkpoint");
    let dry_err = String::from_utf8_lossy(&dry.stderr);
    assert!(
        dry_err.contains("referenced by a checkpoint — kept"),
        "gc must honor the checkpoint reference: {dry_err}"
    );
    assert!(
        store_dir.is_dir(),
        "gc must not delete a referenced store dir"
    );

    // gc gate 2: unreferenced AND older than --min-age-days -> deleted.
    std::fs::remove_file(ws.join(".oxo-flow/checkpoint.json")).unwrap();
    let status = Command::new("touch")
        .args(["-t", "200001010000"])
        .arg(store_dir)
        .status()
        .expect("touch must run");
    assert!(status.success(), "backdating the store dir must succeed");
    let gc = run_oxo(
        &ws,
        &fakebin,
        &["env", "gc", "--min-age-days", "1"],
        &store_env,
    );
    assert_success(&gc, "env gc with an aged, unreferenced store dir");
    let gc_err = String::from_utf8_lossy(&gc.stderr);
    assert!(
        gc_err.contains("1 store dir(s) deleted"),
        "gc must delete the aged unreferenced dir: {gc_err}"
    );
    assert!(!store_dir.exists(), "the store dir must be gone after gc");

    // Documented recovery: the dangling in-tree symlink is detected, detached,
    // and the environment rebuilt — through the same production path.
    let rebuilt = run_oxo(&ws, &fakebin, &["run", "wf.oxoflow"], &store_env);
    assert_success(&rebuilt, "run after gc must rebuild the environment");
    let log = invocations(&log);
    let installs = matching(&log, &["pixi", "install"]);
    assert_eq!(
        installs.len(),
        2,
        "the rebuild must re-run the shared pixi setup command"
    );
    assert!(
        std::fs::read_dir(&store).unwrap().count() == 1,
        "the rebuild must re-home a fresh store entry"
    );
}

/// Divergence regression (issue #1155): `batch -e "conda: <yaml>"` must wrap
/// with the SAME conda-family command `run` uses — before this fix it built
/// its own `conda run --no-banner -n <file stem>` line, which targeted an env
/// the engine never creates for a file-backed spec (`<name>-<hash8>`) and let
/// the rule command escape the env's shell.
#[test]
fn batch_conda_environment_wraps_with_the_engine_created_env() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let fakebin = dir.path().join("fakebin");
    let log = dir.path().join("conda.log");
    let root = dir.path().join("conda_root");
    let broken = dir.path().join("never");
    write_fake_conda_family(&fakebin, "conda", &log, &root, &broken);
    write_conda_family_workflow(&ws, "conda");

    // The engine creates and uses its env (the reference prefix).
    let run = run_oxo(&ws, &fakebin, &["run", "wf.oxoflow"], &[]);
    assert_success(&run, "engine run before batch");
    let engine_prefix = std::fs::read_to_string(ws.join("used.txt")).unwrap();
    assert!(!engine_prefix.is_empty());

    // `batch` must run the item inside that same environment.
    let batch = run_oxo(
        &ws,
        &fakebin,
        &[
            "batch",
            "-e",
            "conda: envs/tools.yaml",
            "printf '%s' \"$CONDA_PREFIX\" > batch_used.txt",
            "x",
        ],
        &[],
    );
    assert_success(&batch, "batch -e conda run");

    let batch_prefix = std::fs::read_to_string(ws.join("batch_used.txt"))
        .expect("batch item must write batch_used.txt inside the env");
    assert_eq!(
        batch_prefix, engine_prefix,
        "batch must use the SAME environment `run` created, not a hand-derived copy"
    );

    // And the wrap it executed is literally the engine's conda-family wrap.
    // The spec is resolved against the batch process's cwd (the workflow), so
    // the test process — whose cwd differs — must pass the absolute path to
    // derive the same content-hashed env name.
    let yaml = ws.join("envs/tools.yaml");
    let expected = CondaBackend
        .wrap_command_with_opts(
            "printf '%s' \"$CONDA_PREFIX\" > batch_used.txt",
            yaml.to_str().unwrap(),
            None,
        )
        .unwrap();
    assert!(
        expected.starts_with("conda run --no-capture-output -n tools-"),
        "shared builder shape changed: {expected}"
    );
    let expected_name = expected
        .split(" -n ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .expect("shared wrap must select the env by -n");
    let log = invocations(&log);
    let batch_wraps: Vec<_> = matching(&log, &["conda", "run", "--no-capture-output"])
        .into_iter()
        .filter(|argv| argv.iter().any(|a| a.contains("batch_used.txt")))
        .collect();
    assert_eq!(
        batch_wraps.len(),
        1,
        "batch must invoke the shared conda-family wrap; log: {log:?}"
    );
    let wrap = batch_wraps[0];
    assert_eq!(wrap[0], "conda");
    assert_eq!(wrap[1], "run");
    assert_eq!(wrap[2], "--no-capture-output");
    assert_eq!(wrap[3], "-n");
    assert_eq!(
        wrap[4], expected_name,
        "batch must target the derived env name from the shared builder"
    );
    assert_eq!(&wrap[5..7], &["bash".to_string(), "-c".to_string()]);
    assert!(
        wrap[7].contains("export PATH=\"$CONDA_PREFIX/bin:$PATH\";"),
        "batch must run the item inside the env shell: {}",
        wrap[7]
    );
}
