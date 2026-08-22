//! Issue #131: when the run aborts on a required failure, in-flight rule
//! processes must be killed — `abort_all()` alone cancels the tokio tasks
//! but orphans the OS children (live: auto-sra v40 exited while 5 merges
//! and 2 STAR kept running).

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

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
/// process tree, not just cancel its task. The pid is captured WHILE the
/// run is in flight — after the abort, the pid file itself is legitimately
/// removed by failed-output invalidation (#118), so the surviving-file
/// check would test the wrong property.
#[test]
fn abort_kills_inflight_rule_processes() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("abort.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"abort\"\n\n\
         [[rules]]\nname = \"bad\"\nshell = \"\"\"\nsleep 2\nexit 1\n\"\"\"\n\n\
         [[rules]]\nname = \"slow\"\noutput = [\"slow.pid\"]\nshell = \"\"\"\n\
         echo $$ > slow.pid\nsleep 30\n\"\"\"\n",
    )
    .unwrap();

    // Act — spawn the run in the background and capture slow's pid while
    // it is still alive.
    let mut child = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "-j", "2"])
        .current_dir(dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let pid_path = dir.path().join("slow.pid");
    let pid_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let pid: i32 = loop {
        if let Ok(content) = fs::read_to_string(&pid_path)
            && let Ok(pid) = content.trim().parse()
        {
            break pid;
        }
        assert!(
            std::time::Instant::now() < pid_deadline,
            "slow must have spawned before the abort"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };

    let status = child.wait().unwrap();
    assert!(!status.success(), "the run must fail on 'bad'");

    // Assert — the abort must have killed slow's process tree.
    let kill_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .is_ok_and(|s| s.success());
        if !alive {
            break;
        }
        if std::time::Instant::now() > kill_deadline {
            panic!("in-flight rule process {pid} survived the abort (orphaned)");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
