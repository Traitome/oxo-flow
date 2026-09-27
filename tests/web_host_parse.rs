//! Issue #573: the standalone binary parsed `--host` as an IP only, so
//! `oxo-flow-web --host localhost` (or `OXO_FLOW_HOST=localhost`) died at
//! startup with a bare parse error, while `oxo-flow serve` bound the same
//! value happily. `resolve_bind_addr` now resolves hostnames via
//! `ToSocketAddrs` — the regression test is the real binary accepting
//! `--host localhost` and serving `/api/health`.

mod common;

#[test]
fn web_server_binds_host_localhost() {
    let dir = tempfile::tempdir().expect("tempdir");
    let port = common::free_port();
    let _server = common::spawn_web_server_with_args(
        dir.path(),
        port,
        // `--host` overrides the helper's default OXO_FLOW_HOST=127.0.0.1.
        &[],
        &["--host", "localhost"],
    );
    let resp = reqwest::blocking::get(format!("{}/api/health", _server.base))
        .expect("health request over the localhost bind");
    assert!(
        resp.status().is_success(),
        "server bound to --host localhost must answer /api/health"
    );
}
