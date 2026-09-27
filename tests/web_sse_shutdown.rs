//! Issue #572: the infinite `/api/events` SSE stream must end on
//! shutdown, otherwise axum's graceful drain waits for it forever and
//! the process only dies when the supervisor SIGKILLs it. Spawn the
//! server, open the stream, send SIGTERM, and assert the process exits
//! well within a supervisor grace period.

mod common;

use std::time::{Duration, Instant};

fn wait_ready(server: &common::WebServer) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(resp) = reqwest::blocking::get(format!("{}/api/health", server.base)) {
            if resp.status().is_success() {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "server did not become ready in 30s\n{}",
            common::log_tail(server)
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn sse_connection_ends_before_grace_period_on_sigterm() {
    let dir = tempfile::tempdir().expect("tempdir");
    let port = common::free_port();
    let mut server = common::spawn_web_server(dir.path(), port, &[]);
    wait_ready(&server);

    // Open the SSE stream in personal mode (no ticket needed) and hold it.
    // reqwest's blocking client is fine here: we only need the connection
    // to be open when SIGTERM lands, not to read frames from it.
    let base = server.base.clone();
    let reader = std::thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("client");
        client
            .get(format!("{base}/api/events"))
            .header("Accept", "text/event-stream")
            .send()
    });

    // Wait until the response headers arrive: the stream is now pinned
    // open server-side.
    let resp = reader.join().expect("reader thread").expect("SSE response");
    assert_eq!(
        resp.headers()["content-type"],
        "text/event-stream",
        "GET /api/events must be streaming before SIGTERM"
    );

    // SIGTERM — what `docker stop` / `systemctl stop` deliver.
    let pid = server.child.id();
    let sent_at = Instant::now();
    let status = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .expect("kill -TERM");

    assert!(status.success(), "kill -TERM must succeed");

    // The graceful drain must complete on its own. 20 s is far inside the
    // 30 s compose stop_grace_period this change ships, and generous for
    // CI load; before the fix this only ended via SIGKILL.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match server.child.try_wait() {
            Ok(Some(exit)) => {
                let elapsed = sent_at.elapsed();
                assert!(
                    elapsed < Duration::from_secs(15),
                    "server took {elapsed:?} to exit — graceful drain did not complete"
                );
                // A completed drain exits cleanly; a SIGKILL would show up
                // as signal termination (code None) instead.
                assert_eq!(
                    exit.code(),
                    Some(0),
                    "server must exit cleanly after SIGTERM, not via kill signal"
                );
                break;
            }
            Ok(None) => {
                assert!(
                    Instant::now() < deadline,
                    "server still alive 20s after SIGTERM — SSE stream pins the drain\n{}",
                    common::log_tail(&server)
                );
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(err) => panic!("try_wait failed: {err}"),
        }
    }
}
