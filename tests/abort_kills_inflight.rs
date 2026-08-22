//! Issue #131: when the run aborts on a required failure, in-flight rule
//! processes must be killed — `abort_all()` alone cancels the tokio tasks
//! but orphans the OS children (live: auto-sra v40 exited while 5 merges
//! and 2 STAR kept running).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn workspace_bin(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("target/debug");
    path.push(name);
    path
}

fn oxo_flow_cmd() -> Command {
    Command::new(workspace_bin("oxo-flow"))
}

/// `bad` fails while `slow` is mid-sleep; the abort must signal `slow`'s
/// process tree, not just cancel its task.
#[test]
fn abort_kills_inflight_rule_processes() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("abort.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"abort\"\n\n\
         [[rules]]\nname = \"bad\"\nshell = \"\"\"\nsleep 10\nexit 1\n\"\"\"\n\n\
         [[rules]]\nname = \"slow\"\noutput = [\"slow.pid\"]\nshell = \"\"\"\n\
         echo $$ > slow.pid\nsleep 30\n\"\"\"\n",
    )
    .unwrap();

    // Act
    let run = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "-j", "2", "-v"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let run_log = fs::read_to_string(dir.path().join(".oxo-flow/logs/oxo-flow.log"))
        .unwrap_or_default();
    assert!(
        !run.status.success(),
        "the run must fail on 'bad':\n{}\n--- run log ---\n{}",
        String::from_utf8_lossy(&run.stderr),
        run_log
    );

    // Assert — slow spawned before the abort and must be dead after it.
    let pid_path = dir.path().join("slow.pid");
    assert!(
        pid_path.exists(),
        "slow must have spawned before the abort:\n{}\n--- run log ---\n{}",
        String::from_utf8_lossy(&run.stderr),
        run_log
    );
    let pid: i32 = fs::read_to_string(&pid_path)
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .is_ok_and(|s| s.success());
        if !alive {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("in-flight rule process {pid} survived the abort (orphaned)");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
