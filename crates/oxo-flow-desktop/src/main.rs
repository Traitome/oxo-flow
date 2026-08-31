//! oxo-flow desktop shell — a native window around the local web server.
//!
//! The single `oxo-flow-desktop` binary is the whole product: the workflow
//! engine, web server, and SPA frontend ship inside it (via `oxo-flow-web`).
//! On launch it
//!
//! 1. moves the working directory to `$HOME` when launched with `cwd=/`
//!    (Finder launches `.app` bundles with `cwd=/`, where the server cannot
//!    create its SQLite database),
//! 2. starts the axum server (personal mode, loopback) on a free port,
//! 3. waits for the listener to accept connections, then
//! 4. opens a native webview window pointed at the server URL.
//!
//! Rendering happens in the OS webview (WKWebView on macOS, WebView2 on
//! Windows, WebKitGTK on Linux) — no bundled browser, no Electron. Closing
//! the window shuts the server down and exits.
//!
//! External links (anything off the loopback origin, e.g. the GitHub docs
//! links in the UI) open in the system browser instead of navigating the
//! app window away from the interface.

use anyhow::{Context, Result};
use tao::dpi::LogicalSize;
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::{Window, WindowBuilder};
use wry::WebViewBuilder;

/// Loopback host the server binds to. `personal` mode enforces loopback
/// anyway (see `effective_bind_host`), this keeps the window URL in sync.
const HOST: &str = "127.0.0.1";

fn main() {
    if let Err(e) = run() {
        eprintln!("oxo-flow desktop: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    // macOS Finder launches .app bundles with cwd=/, where the server's
    // SQLite open fails (read-only root) and serve exits. The launcher
    // script used to `cd $HOME`; the desktop binary does the same natively.
    if std::env::current_dir()
        .map(|d| d == std::path::Path::new("/"))
        .unwrap_or(false)
    {
        let home = std::env::var("HOME")
            .context("launched with cwd=/ and no $HOME set — cannot pick a working directory")?;
        std::env::set_current_dir(&home)
            .with_context(|| format!("failed to change working directory to {home}"))?;
    }

    // The embedded server writes logs/, oxo-flow.db, and serves the SPA —
    // mirror the CLI's tracing setup (human-readable, env-filterable).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    // Grab an ephemeral port, then start the embedded server on a tokio
    // runtime that outlives the event loop below.
    let port = pick_free_port()?;
    let server_url = format!("http://{HOST}:{port}");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    let mut server_task = runtime.spawn(oxo_flow_web::start_server_with_mode(
        "personal", HOST, port, "",
    ));

    // tao 0.37: EventLoop::new() returns EventLoop<()>, not a Result —
    // construction failures panic inside tao with a platform-specific
    // message (no display / missing Xcode SDK), so there is nothing to
    // map into anyhow here.
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("oxo-flow")
        .with_inner_size(LogicalSize::new(1440.0, 900.0))
        .with_min_inner_size(LogicalSize::new(960.0, 600.0))
        .build(&event_loop)
        .context("failed to create window")?;

    // Wait until the listener accepts TCP connections (bounded), so the
    // first page load never races server startup. 10s covers cold starts
    // (SQLite init, first-run migrations) on slow disks.
    wait_for_server(&server_url).context("the embedded web server did not become ready in time")?;

    let webview = build_webview(&window, &server_url).context("failed to create webview")?;
    let _ = webview; // kept alive by wry's internal association with the window

    // The runtime is owned by the event-loop closure (tao's `run` never
    // returns — it has signature `-> !` — so all teardown happens inside
    // the handler). The Option lets the FnMut closure consume it once.
    let mut runtime = Some(runtime);

    let mut exiting = false;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            // Poll the server task so a server crash surfaces as an exit
            // instead of a frozen window silently showing the last page.
            Event::NewEvents(StartCause::Poll) => {
                if server_task.is_finished() {
                    exiting = true;
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                exiting = true;
            }
            Event::MainEventsCleared => {
                if exiting {
                    // A server failure is only readable once we decide to
                    // quit; drain it and turn it into the process exit code
                    // (tao's macOS backend checks should_exit right after
                    // MainEventsCleared, so ExitWithCode here is honored).
                    let code = if server_task.is_finished() {
                        match blocking_recv(&mut server_task) {
                            Some(Err(e)) => {
                                eprintln!("oxo-flow web server exited with an error: {e:#}");
                                1
                            }
                            _ => 0,
                        }
                    } else {
                        0
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

/// Receive a finished JoinHandle's result without blocking: the caller has
/// checked `is_finished()`, so the future is ready and a single poll with
/// a no-op waker resolves it (we are off the tokio runtime here — the
/// event loop thread — so `blocking_recv`/`now_or_never` would panic).
/// JoinErrors are dropped (the task is finished; a cancelled handle never
/// resolves to Ready), surfacing only the server's own result.
fn blocking_recv(
    handle: &mut tokio::task::JoinHandle<anyhow::Result<()>>,
) -> Option<anyhow::Result<()>> {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    // The task is finished, so the waker is never actually invoked; a
    // no-op waker is enough to drive the one poll.
    static NOOP_VTABLE: RawWakerVTable = RawWakerVTable::new(
        |_| RawWaker::new(std::ptr::null(), &NOOP_VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &NOOP_VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    match std::pin::Pin::new(handle).poll(&mut cx) {
        Poll::Ready(Ok(res)) => Some(res),
        _ => None,
    }
}

/// Build the webview for the current platform. Linux needs the GTK path
/// (WebKitGTK) so both X11 and Wayland work; macOS/Windows build against
/// the tao window directly.
fn build_webview(window: &Window, url: &str) -> Result<wry::WebView> {
    let builder = WebViewBuilder::new()
        .with_url(url)
        // Links to other origins (docs, GitHub, help pages) open in the
        // user's browser; same-origin navigations keep flowing in-app.
        .with_navigation_handler(|uri| {
            if is_app_origin(&uri) {
                true
            } else {
                let _ = open_external(&uri);
                false
            }
        });

    #[cfg(not(target_os = "linux"))]
    let webview = builder.build(window)?;
    #[cfg(target_os = "linux")]
    let webview = builder.build_gtk(window.gtk_window())?;
    Ok(webview)
}

/// Whether `uri` belongs to the app's own loopback origin.
fn is_app_origin(uri: &str) -> bool {
    // The server only ever serves from http://127.0.0.1:<port>; anything
    // else (https, file:, other hosts) is external.
    uri.starts_with(&format!("http://{HOST}:"))
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

/// Poll `GET {url}/` until it responds (any status) or the deadline passes.
fn wait_for_server(url: &str) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if std::net::TcpStream::connect(url).is_ok() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    anyhow::bail!("server at {url} did not start listening")
}
