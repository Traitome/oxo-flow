//! oxo-flow desktop shell — a native window around the local web server.
//!
//! The single `oxo-flow-desktop` binary is the whole product: the workflow
//! engine, web server, and SPA frontend ship inside it (via `oxo-flow-web`).
//! On launch it
//!
//! 1. moves the working directory to a stable data directory
//!    (`~/.oxo-flow/desktop`, the same convention as the CLI's `~/.oxo-flow`
//!    locks/plugins) so the SQLite database, logs, and workspace files land
//!    in one predictable place no matter how the app was launched — Finder
//!    starts `.app` bundles in `/`, terminals in whatever directory the user
//!    was in,
//! 2. takes an exclusive single-instance lock in that directory,
//! 3. starts the axum server (personal mode, loopback) on a free port,
//! 4. waits for the listener to accept connections, then
//! 5. opens a native webview window pointed at the server URL.
//!
//! Rendering happens in the OS webview (WKWebView on macOS, WebView2 on
//! Windows, WebKitGTK on Linux) — no bundled browser, no Electron. Closing
//! the window shuts the server down and exits.
//!
//! The main loop idles in `ControlFlow::Wait` (true sleep, no CPU spin), so
//! lifecycle events arrive through an [`tao::event_loop::EventLoopProxy`]: a
//! watchdog task on the tokio runtime races the server future against
//! SIGINT/SIGTERM and forwards the outcome as a `ShellEvent`. This covers
//! server crashes (exit non-zero), `kill <pid>` (graceful exit), and Ctrl-C.
//!
//! External links (anything off the loopback origin, e.g. the GitHub docs
//! links in the UI) open in the system browser instead of navigating the
//! app window away from the interface — for both plain navigations and
//! new-window requests (`target="_blank"`, `window.open`).

#[cfg(target_os = "linux")]
use tao::platform::unix::WindowExtUnix;
#[cfg(target_os = "linux")]
use wry::WebViewBuilderExtUnix;

use anyhow::{Context, Result};
use fs2::FileExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::{Window, WindowBuilder};
use wry::{NewWindowResponse, WebViewBuilder};

/// Loopback host the server binds to. `personal` mode enforces loopback
/// anyway (see `effective_bind_host`), this keeps the window URL in sync.
const HOST: &str = "127.0.0.1";

/// Events forwarded from the tokio runtime into the tao main loop.
enum ShellEvent {
    /// The embedded server future completed (normally or with an error).
    ServerFinished(Result<()>),
    /// SIGINT or SIGTERM was received; shut down gracefully.
    Signalled,
}

/// Set by the watchdog when it observes a signal or a server exit, and read
/// by the blocking startup steps below (window build, server wait): while
/// this thread is inside `WindowBuilder::build()` no event loop is running
/// to receive proxy events, so a signal landing there would otherwise only
/// wake oxo-flow-web's own shutdown handler — closing the listener out from
/// under `wait_for_server` and turning a `kill` into a bogus startup
/// failure. The flag makes the shutdown observable from this thread.
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// First error reported by the watchdog when the server task itself failed
/// (crash or startup bind error), for the early-exit path below.
static SERVER_FAILURE: OnceLock<String> = OnceLock::new();

fn main() {
    if let Err(e) = run() {
        eprintln!("oxo-flow desktop: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    // All server state (SQLite db, logs/, workspace/) is created relative to
    // the process working directory, so anchor it to a stable data directory
    // before anything else starts. Finder launches .app bundles with cwd=/
    // (where the SQLite open fails) and terminals launch with the user's
    // current directory (where state would silently scatter across folders).
    let data_dir = app_data_dir().context("failed to locate a home directory for app data")?;
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("failed to create data directory {}", data_dir.display()))?;
    std::env::set_current_dir(&data_dir)
        .with_context(|| format!("failed to change working directory to {}", data_dir.display()))?;

    // The embedded server writes logs/, oxo-flow.db, and serves the SPA —
    // mirror the CLI's tracing setup (human-readable, env-filterable).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    // One shell per data directory: two instances would run two servers
    // against the same oxo-flow.db. The advisory lock releases automatically
    // when the process dies (no stale-lock cleanup needed).
    let instance_lock = take_instance_lock().context(
        "another oxo-flow desktop instance is already running for this user account",
    )?;

    // Grab an ephemeral port, then start the embedded server on a tokio
    // runtime that outlives the event loop below.
    let port = pick_free_port()?;
    let server_url = format!("http://{HOST}:{port}");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    let server_task = runtime.spawn(oxo_flow_web::start_server_with_mode(
        "personal", HOST, port, "",
    ));

    // tao 0.37: EventLoopBuilder::build() returns EventLoop<T> directly, not
    // a Result — construction failures panic inside tao with a platform-
    // specific message (no display / missing Xcode SDK), so there is nothing
    // to map into anyhow here.
    let event_loop = EventLoopBuilder::<ShellEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // Watchdog: once the loop below is running it idles in
    // ControlFlow::Wait, where neither platform emits NewEvents(Poll) we
    // could poll JoinHandles from — so the runtime pushes lifecycle events
    // through the proxy instead (both backends wake the loop for proxy
    // events even under Wait). Race the server against SIGINT/SIGTERM so
    // `kill` shuts the app down cleanly.
    //
    // This MUST be spawned before the window build below: cold GTK/webkit
    // initialization blocks this thread for seconds, and a signal landing in
    // that window would otherwise only wake oxo-flow-web's own shutdown
    // handler — closing the listener out from under `wait_for_server` and
    // turning a `kill` into a bogus startup failure. The watchdog also sets
    // SHUTDOWN_REQUESTED, which the blocking steps check.
    runtime.spawn(async move {
        #[cfg(unix)]
        let interrupted = async {
            use tokio::signal::unix::{signal, SignalKind as TkSignalKind};
            let mut term = signal(TkSignalKind::terminate())
                .expect("failed to install SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        };
        #[cfg(not(unix))]
        let interrupted = async {
            let _ = tokio::signal::ctrl_c().await;
        };

        tokio::select! {
            res = server_task => {
                let result = match res {
                    Ok(inner) => inner,
                    Err(join) => Err(anyhow::anyhow!("server task failed: {join}")),
                };
                if let Err(e) = &result {
                    let _ = SERVER_FAILURE.set(format!("{e:#}"));
                }
                SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
                let _ = proxy.send_event(ShellEvent::ServerFinished(result));
            }
            _ = interrupted => {
                SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
                let _ = proxy.send_event(ShellEvent::Signalled);
            }
        }
    });

    let window = WindowBuilder::new()
        .with_title("oxo-flow")
        .with_inner_size(LogicalSize::new(1440.0, 900.0))
        .with_min_inner_size(LogicalSize::new(960.0, 600.0))
        .build(&event_loop)
        .context("failed to create window")?;

    // A signal (or server exit) that landed while the window was building
    // never reaches the event loop — it is not running yet, and the proxy
    // event is only queued. The watchdog flagged it in SHUTDOWN_REQUESTED,
    // so honor the flag here instead of bringing up a window whose server
    // is already shutting down; the exit code reports a server failure.
    if SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
        return early_exit_result();
    }

    // Wait until the listener accepts TCP connections (bounded), so the
    // first page load never races server startup. 10s covers cold starts
    // (SQLite init, first-run migrations) on slow disks. The wait aborts
    // early once the watchdog flags a shutdown — otherwise a signal arriving
    // mid-wait would sit out the full deadline against a listener that is
    // already closing.
    if let ServerWait::ShuttingDown = wait_for_server(HOST, port)
        .context("the embedded web server did not become ready in time")?
    {
        return early_exit_result();
    }

    let webview = build_webview(&window, &server_url, port)
        .context("failed to create webview")?;
    let _ = webview; // kept alive by wry's internal association with the window

    // The runtime is owned by the event-loop closure (tao's `run` never
    // returns — it has signature `-> !` — so all teardown happens inside
    // the handler). The Option lets the FnMut closure consume it once.
    let mut runtime = Some(runtime);
    // Dropping the lock file handle releases the single-instance lock; keep
    // it alive for the whole run (this closure outlives the loop).
    let _instance_lock = instance_lock;

    let mut exiting = false;
    let mut server_result: Option<Result<()>> = None;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            // The runtime told us the server ended (crash or clean exit):
            // record the outcome and leave, instead of freezing on a stale
            // page that can no longer reach its server.
            Event::UserEvent(ShellEvent::ServerFinished(result)) => {
                server_result = Some(result);
                exiting = true;
            }
            Event::UserEvent(ShellEvent::Signalled) => {
                server_result = Some(Ok(()));
                exiting = true;
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                // Ask the server to shut down gracefully (flush audit logs,
                // finish in-flight responses) rather than relying on the 3s
                // runtime timeout below, which cancels tasks mid-flight.
                // SIGTERM is already wired through the watchdog above —
                // `Signalled` records a clean result and both the watchdog
                // and `oxo-flow-web`'s own `shutdown_signal()` handle it.
                exiting = true;
                #[cfg(unix)]
                request_graceful_shutdown();
            }
            Event::MainEventsCleared => {
                if exiting {
                    // A server failure becomes the process exit code (tao's
                    // macOS backend checks should_exit right after
                    // MainEventsCleared, so ExitWithCode here is honored).
                    let code = match &server_result {
                        Some(Err(e)) => {
                            eprintln!("oxo-flow web server exited with an error: {e:#}");
                            1
                        }
                        _ => 0,
                    };
                    if let Some(rt) = runtime.take() {
                        rt.shutdown_timeout(std::time::Duration::from_secs(3));
                    }
                    *control_flow = ControlFlow::ExitWithCode(code);
                }
            }
            // Backstop for exit paths that skip MainEventsCleared (display
            // disconnection and similar): never leave worker threads running.
            Event::LoopDestroyed => {
                if let Some(rt) = runtime.take() {
                    rt.shutdown_background();
                }
            }
            _ => {}
        }
    });
}

/// Take an exclusive advisory lock on `oxo-flow.desktop.lock` in the current
/// directory. The OS releases it automatically when the process exits, so a
/// crash cannot leave a stale lock behind.
fn take_instance_lock() -> Result<std::fs::File> {
    let path = std::path::Path::new("oxo-flow.desktop.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.try_lock_exclusive().with_context(|| {
        format!(
            "failed to lock {} (is another instance running?)",
            path.display()
        )
    })?;
    Ok(file)
}

/// Stable per-user data directory the desktop server runs in.
///
/// Everything the embedded server touches is cwd-relative (`oxo-flow.db`,
/// `logs/`, `workspace/`) — plus our own `oxo-flow.desktop.lock` — so
/// anchoring the cwd here keeps all state in one predictable place.
/// Follows the CLI's `~/.oxo-flow` convention (locks, plugins).
fn app_data_dir() -> Result<std::path::PathBuf> {
    // Same env-var lookup as oxo-flow-core's env_create_lock: no external
    // home-dir dependency for one path.
    #[cfg(unix)]
    let home = std::env::var_os("HOME");
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(any(unix, windows)))]
    let home: Option<std::ffi::OsString> = None;

    let mut dir = std::path::PathBuf::from(home.ok_or_else(|| {
        anyhow::anyhow!("$HOME is not set (launched from a context without a user session)")
    })?);
    dir.push(".oxo-flow");
    dir.push("desktop");
    Ok(dir)
}

/// Ask this process to shut down gracefully via SIGTERM (unix only).
///
/// The watchdog task already listens for SIGTERM and forwards it as
/// `ShellEvent::Signalled`, and `oxo-flow-web`'s `shutdown_signal()` uses
/// the same signal to run axum's graceful shutdown — one signal wakes both.
/// Other platforms keep the plain timeout path (runtime.shutdown_timeout).
#[cfg(unix)]
fn request_graceful_shutdown() {
    // SAFETY: kill with signal 0 semantics — the target is our own pid, so
    // the call cannot fail for permission reasons.
    let pid = std::process::id() as libc::pid_t;
    let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
    if rc != 0 {
        // Last resort is the 3s timeout in MainEventsCleared; log only.
        eprintln!("oxo-flow desktop: failed to self-signal SIGTERM for graceful shutdown");
    }
}

/// Build the webview for the current platform. Linux needs the GTK path
/// (WebKitGTK) so both X11 and Wayland work; macOS/Windows build against
/// the tao window directly.
fn build_webview(window: &Window, url: &str, port: u16) -> Result<wry::WebView> {
    let builder = WebViewBuilder::new()
        .with_url(url)
        // Links to other origins (docs, GitHub, help pages) open in the
        // user's browser; same-origin navigations keep flowing in-app.
        .with_navigation_handler(move |uri| {
            if is_app_origin(&uri, port) {
                true
            } else {
                let _ = open_external(&uri);
                false
            }
        })
        // New-window requests (`target="_blank"`, `window.open`) bypass the
        // navigation handler entirely: without a handler here the OS default
        // is to deny them silently (verified in wry 0.56.1 — macOS returns
        // None from createWebViewWith..., Linux never connects create).
        // Route them to the system browser like other external links.
        .with_new_window_req_handler(|uri, _| {
            let _ = open_external(&uri);
            NewWindowResponse::Deny
        });

    #[cfg(not(target_os = "linux"))]
    let webview = builder.build(window)?;
    #[cfg(target_os = "linux")]
    let webview = builder.build_gtk(window.gtk_window())?;
    Ok(webview)
}

/// Whether `uri` belongs to the app's own loopback origin.
fn is_app_origin(uri: &str, port: u16) -> bool {
    // Strict scheme://host:port match. String-prefix matching would accept
    // lookalikes like `http://127.0.0.1:9999.evil.com/` or userinfo tricks
    // (`http://127.0.0.1:PORT@evil.com/`), so parse the authority instead.
    let Some(rest) = uri.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    authority == format!("{HOST}:{port}")
}

/// Open a URI in the system browser (best-effort; failures are ignored —
/// a missing `xdg-open` on a headless box must not crash the app).
fn open_external(uri: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(uri)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(uri)
            .spawn()
            .map(|_| ())
    }
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", "", uri])
            .spawn()
            .map(|_| ())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        let _ = uri;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no browser opener on this platform",
        ))
    }
}

/// Grab an ephemeral port from the OS without binding it for long.
///
/// Slightly racy by nature (another process could take the port between
/// close and the server's bind), but the failure window is microseconds
/// and the server surfaces a bind error loudly if it ever happens.
fn pick_free_port() -> Result<u16> {
    std::net::TcpListener::bind((HOST, 0))
        .context("failed to allocate a local port")?
        .local_addr()
        .map(|a| a.port())
        .context("failed to read the allocated port")
}

/// Outcome of the bounded server startup wait.
enum ServerWait {
    /// The listener accepts connections; safe to build the webview.
    Ready,
    /// The watchdog flagged a signal or server exit while we waited — the
    /// startup sequence must not continue.
    ShuttingDown,
}

/// Exit without ever showing a window, after a signal or server failure
/// landed during startup. Mirrors the event-loop exit: a recorded server
/// failure becomes exit code 1, otherwise 0 (a graceful `kill` during
/// startup should not look like a crash).
fn early_exit_result() -> Result<()> {
    if let Some(err) = SERVER_FAILURE.get() {
        anyhow::bail!("web server failed during startup: {err}");
    }
    Ok(())
}

/// Poll the listener until it accepts TCP connections or the deadline
/// passes, aborting as soon as the watchdog flags a shutdown. Polling
/// `TcpStream::connect` (sync, on this thread) keeps the startup path free
/// of runtime assumptions; the 50ms step is fast enough to notice a
/// shutdown request promptly. Note `TcpStream::connect` takes a socket
/// address pair, not the URL string: the `ToSocketAddrs` impl for `&str`
/// would try to DNS-resolve the host `"http://127.0.0.1"`, which fails on
/// Linux (glibc rejects the malformed hostname) even though macOS tolerated
/// it.
fn wait_for_server(host: &str, port: u16) -> Result<ServerWait> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
            return Ok(ServerWait::ShuttingDown);
        }
        if std::net::TcpStream::connect((host, port)).is_ok() {
            return Ok(ServerWait::Ready);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    anyhow::bail!("server at {host}:{port} did not start listening")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_origin_is_app() {
        assert!(is_app_origin("http://127.0.0.1:4173/", 4173));
        assert!(is_app_origin("http://127.0.0.1:4173/api/health", 4173));
        assert!(is_app_origin("http://127.0.0.1:4173/api/runs?x=1#frag", 4173));
    }

    #[test]
    fn different_port_is_external() {
        assert!(!is_app_origin("http://127.0.0.1:9999/", 4173));
        // Prefix matching would wrongly accept this one.
        assert!(!is_app_origin("http://127.0.0.1:41739/", 4173));
    }

    #[test]
    fn other_schemes_are_external() {
        assert!(!is_app_origin("https://127.0.0.1:4173/", 4173));
        assert!(!is_app_origin("file:///etc/passwd", 4173));
        assert!(!is_app_origin("about:blank", 4173));
    }

    #[test]
    fn lookalike_hosts_are_external() {
        assert!(!is_app_origin("http://127.0.0.1.evil.com:4173/", 4173));
        assert!(!is_app_origin("http://localhost:4173/", 4173));
        assert!(!is_app_origin("http://[::1]:4173/", 4173));
        // Userinfo trick: the host is evil.com, not the app.
        assert!(!is_app_origin("http://127.0.0.1:4173@evil.com/", 4173));
    }
}
