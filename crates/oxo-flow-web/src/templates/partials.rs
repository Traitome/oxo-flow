//! Shared template components for oxo-flow-web.
//!
//! Provides reusable HTML fragments including header, navigation, footer,
//! and card components following the GitHub dark theme.

use maud::{DOCTYPE, Markup, html};

/// GitHub dark theme colors.
pub mod theme {
    pub const BG: &str = "#0d1117";
    pub const SURFACE: &str = "#161b22";
    pub const SURFACE_ALT: &str = "#21262d";
    pub const BORDER: &str = "#30363d";
    pub const ACCENT: &str = "#58a6ff";
    pub const ACCENT_ALT: &str = "#8b949e";
    pub const TEXT: &str = "#c9d1d9";
    pub const TEXT_MUTED: &str = "#8b949e";
    pub const TEXT_SUBTLE: &str = "#6e7681";
    pub const SUCCESS: &str = "#3fb950";
    pub const ERROR: &str = "#f85149";
    pub const WARNING: &str = "#d29922";
}

/// Base page layout with common head elements.
pub fn base_page(title: &str, content: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="UTF-8";
                meta name="viewport" content="width=device-width, initial-scale=1.0";
                title { (title) " — oxo-flow" }

                // Alpine.js for interactivity
                script src="https://unpkg.com/alpinejs@3.x.x/dist/cdn.min.js" defer {}

                // Chart.js for charts
                script src="https://cdn.jsdelivr.net/npm/chart.js@4.4.1/dist/chart.umd.min.js" {}

                // HTMX for AJAX interactions
                script src="https://unpkg.com/htmx.org@1.9.10/dist/htmx.min.js" {}

                // HTMX SSE extension for real-time updates
                script src="https://unpkg.com/htmx.org@1.9.10/dist/ext/sse.js" {}

                // Base styles
                style { (base_styles()) }
            }
            body {
                (content)
            }
        }
    }
}

/// Base CSS styles using GitHub dark theme.
fn base_styles() -> String {
    format!(
        r#"
:root {{
    --bg: {bg};
    --surface: {surface};
    --surface-alt: {surface_alt};
    --border: {border};
    --accent: {accent};
    --accent-alt: {accent_alt};
    --text: {text};
    --text-muted: {text_muted};
    --text-subtle: {text_subtle};
    --success: {success};
    --error: {error};
    --warning: {warning};
    --radius: 6px;
    --font: -apple-system, BlinkMacSystemFont, 'Segoe UI', 'Noto Sans', Helvetica, Arial, sans-serif;
    --mono: ui-monospace, SFMono-Regular, 'SF Mono', Menlo, Consolas, monospace;
}}
* {{ margin: 0; padding: 0; box-sizing: border-box; }}
body {{
    font-family: var(--font);
    background: var(--bg);
    color: var(--text);
    min-height: 100vh;
    line-height: 1.5;
}}
::selection {{ background: var(--accent); color: var(--bg); }}
a {{ color: var(--accent); text-decoration: none; }}
a:hover {{ text-decoration: underline; }}
"#,
        bg = theme::BG,
        surface = theme::SURFACE,
        surface_alt = theme::SURFACE_ALT,
        border = theme::BORDER,
        accent = theme::ACCENT,
        accent_alt = theme::ACCENT_ALT,
        text = theme::TEXT,
        text_muted = theme::TEXT_MUTED,
        text_subtle = theme::TEXT_SUBTLE,
        success = theme::SUCCESS,
        error = theme::ERROR,
        warning = theme::WARNING,
    )
}

/// Sidebar navigation component.
pub fn sidebar(active_view: &str, username: &str) -> Markup {
    html! {
        aside class="sidebar" {
            div class="sidebar-logo" {
                h1 { "oxo-" span { "flow" } }
                div class="ver" { "Command Center" }
            }
            nav class="sidebar-nav" {
                @let views = [
                    ("dashboard", "Dashboard", "&#9632;"),
                    ("editor", "Workflow Editor", "&#9998;"),
                    ("runs", "Run History", "&#9678;"),
                    ("workflows", "Saved Workflows", "&#9644;"),
                    ("system", "System", "&#9881;"),
                ];
                @for (view, label, icon) in views {
                    button
                        class=(if view == active_view { "active" } else { "" })
                        data-view=(view)
                        hx-get={"/" (view)}
                        hx-target="#main-content"
                        hx-push-url="true"
                    {
                        span class="nav-icon" { (maud::PreEscaped(icon)) }
                        (label)
                    }
                }
            }
            div class="sidebar-footer" {
                div class="user-info" {
                    div class="avatar" title="User" { (username.chars().next().unwrap_or('G').to_uppercase()) }
                    span { (username) }
                    span class="status-dot ok" {}
                }
            }
        }
    }
}

/// Top bar component with live metrics.
pub fn topbar(title: &str) -> Markup {
    html! {
        div class="topbar" {
            h2 { (title) }
            div class="topbar-meta" {
                span { "CPU " span id="live-cpu" { "--" } }
                span { "MEM " span id="live-mem" { "--" } }
                span { "Runs " span id="live-runs" { "0" } }
                span id="live-uptime" { "--" }
            }
        }
    }
}

/// Card component with optional header actions.
pub fn card(title: &str, content: Markup, actions: Option<Markup>) -> Markup {
    html! {
        div class="card" {
            div class="card-header" {
                span { (title) }
                @if let Some(action_buttons) = actions {
                    div class="card-actions" { (action_buttons) }
                }
            }
            div class="card-body" { (content) }
        }
    }
}

/// Stat card component for dashboard metrics.
pub fn stat_card(value: &str, label: &str, sub: Option<&str>, progress: Option<u32>) -> Markup {
    html! {
        div class="stat-card" {
            div class="value" { (value) }
            div class="label" { (label) }
            @if let Some(sub_text) = sub {
                div class="sub" { (sub_text) }
            }
            @if let Some(pct) = progress {
                div class="progress-bar" {
                    div class="fill" style={"width: " (pct) "%"} {}
                }
            }
        }
    }
}

/// Badge component for status indicators.
pub fn badge(status: &str) -> Markup {
    let class = match status {
        "running" => "badge-running",
        "success" | "completed" => "badge-success",
        "failed" | "error" => "badge-failed",
        _ => "badge-pending",
    };
    html! {
        span class={"badge " (class)} { (status) }
    }
}

/// Table component with headers.
pub fn table(headers: &[&str], rows: Markup) -> Markup {
    html! {
        table {
            thead {
                tr {
                    @for header in headers {
                        th { (header) }
                    }
                }
            }
            tbody { (rows) }
        }
    }
}

/// Button component with variants.
pub fn button(label: &str, variant: &str, hx_attrs: Option<(&str, &str)>) -> Markup {
    let class = match variant {
        "primary" => "btn btn-primary",
        "danger" => "btn btn-danger",
        _ => "btn btn-outline",
    };
    html! {
        @if let Some((attr, value)) = hx_attrs {
            button class=(class) hx-get=(value) hx-target=(attr) { (label) }
        } @else {
            button class=(class) { (label) }
        }
    }
}

/// Modal overlay component.
pub fn modal(id: &str, title: &str, content: Markup) -> Markup {
    html! {
        div id=(id) class="modal-overlay hidden" {
            div class="modal" {
                div class="modal-header" {
                    h3 { (title) }
                    button class="btn btn-outline btn-sm" onclick={"close" (id) "()"} { "Close" }
                }
                div class="modal-body" { (content) }
            }
        }
    }
}

/// Log viewer component.
pub fn log_viewer(content: &str) -> Markup {
    html! {
        div class="log-viewer" { (content) }
    }
}

/// Sidebar and main layout styles.
pub fn layout_styles() -> String {
    String::from(
        r#"
.sidebar {
    width: 240px;
    background: var(--surface);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    position: fixed;
    top: 0;
    left: 0;
    bottom: 0;
    z-index: 100;
}
.sidebar-logo {
    padding: 1.25rem;
    border-bottom: 1px solid var(--border);
}
.sidebar-logo h1 {
    font-size: 1.1rem;
    font-weight: 700;
    letter-spacing: -0.02em;
}
.sidebar-logo h1 span { color: var(--accent); }
.sidebar-logo .ver {
    font-size: 0.65rem;
    color: var(--text-subtle);
    margin-top: 0.15rem;
}
.sidebar-nav { flex: 1; padding: 0.75rem; }
.sidebar-nav button {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    width: 100%;
    padding: 0.6rem 0.75rem;
    background: transparent;
    border: none;
    border-radius: var(--radius);
    color: var(--text-muted);
    font-size: 0.85rem;
    cursor: pointer;
    transition: all 0.15s;
    text-align: left;
    margin-bottom: 0.15rem;
}
.sidebar-nav button:hover { background: var(--surface-alt); color: var(--text); }
.sidebar-nav button.active { background: var(--accent); color: var(--bg); font-weight: 600; }
.sidebar-nav .nav-icon { font-size: 1.1rem; width: 1.5rem; text-align: center; }
.sidebar-footer {
    padding: 0.75rem;
    border-top: 1px solid var(--border);
}
.user-info {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.8rem;
    color: var(--text-muted);
}
.user-info .avatar {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    background: var(--accent);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 0.7rem;
    font-weight: 700;
    color: white;
}
.status-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    display: inline-block;
    margin-left: auto;
}
.status-dot.ok { background: var(--success); }
.main {
    margin-left: 240px;
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 100vh;
}
.topbar {
    padding: 0.75rem 1.5rem;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
    display: flex;
    justify-content: space-between;
    align-items: center;
    position: sticky;
    top: 0;
    z-index: 50;
}
.topbar h2 { font-size: 1rem; font-weight: 600; }
.topbar-meta {
    display: flex;
    gap: 1.5rem;
    font-size: 0.75rem;
    color: var(--text-muted);
}
.content { padding: 1.5rem; flex: 1; }
"#,
    )
}

/// Component styles (cards, tables, badges, etc.).
pub fn component_styles() -> String {
    String::from(
        r#"
.card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 1.25rem;
    margin-bottom: 1rem;
}
.card-header {
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    margin-bottom: 0.75rem;
    display: flex;
    justify-content: space-between;
    align-items: center;
}
.stats-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 0.75rem;
    margin-bottom: 1.5rem;
}
.stat-card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 1rem;
}
.stat-card .value {
    font-size: 1.6rem;
    font-weight: 700;
    color: var(--accent);
    line-height: 1.2;
}
.stat-card .label {
    font-size: 0.7rem;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin-top: 0.25rem;
}
.stat-card .sub {
    font-size: 0.7rem;
    color: var(--text-subtle);
    margin-top: 0.5rem;
}
.progress-bar {
    background: var(--surface-alt);
    height: 3px;
    border-radius: 2px;
    margin-top: 0.5rem;
    overflow: hidden;
}
.progress-bar .fill {
    background: var(--accent);
    height: 100%;
    transition: width 0.5s;
    border-radius: 2px;
}
table { width: 100%; border-collapse: collapse; font-size: 0.82rem; }
th {
    text-align: left;
    padding: 0.6rem 0.75rem;
    color: var(--text-muted);
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    font-weight: 600;
    border-bottom: 1px solid var(--border);
}
td { padding: 0.6rem 0.75rem; border-bottom: 1px solid var(--border); }
tr:hover td { background: rgba(88, 166, 255, 0.03); }
.badge {
    display: inline-block;
    padding: 0.15rem 0.5rem;
    border-radius: 1rem;
    font-size: 0.7rem;
    font-weight: 600;
}
.badge-running { background: rgba(210, 153, 34, 0.2); color: var(--warning); }
.badge-success { background: rgba(63, 185, 80, 0.2); color: var(--success); }
.badge-failed { background: rgba(248, 81, 73, 0.2); color: var(--error); }
.badge-pending { background: rgba(139, 148, 158, 0.2); color: var(--text-muted); }
.btn {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.55rem 1rem;
    border-radius: var(--radius);
    border: none;
    cursor: pointer;
    font-size: 0.82rem;
    font-weight: 500;
    font-family: var(--font);
    transition: all 0.15s;
}
.btn-primary { background: var(--accent); color: var(--bg); }
.btn-primary:hover { filter: brightness(1.15); }
.btn-outline { background: transparent; border: 1px solid var(--border); color: var(--text); }
.btn-outline:hover { background: var(--surface-alt); }
.btn-danger { background: var(--error); color: white; }
.btn-sm { padding: 0.35rem 0.65rem; font-size: 0.75rem; }
.log-viewer {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    font-family: var(--mono);
    font-size: 0.78rem;
    line-height: 1.6;
    padding: 1rem;
    max-height: 500px;
    overflow-y: auto;
    white-space: pre-wrap;
    color: var(--text);
}
.modal-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.7);
    z-index: 200;
    display: flex;
    align-items: center;
    justify-content: center;
}
.modal-overlay.hidden { display: none; }
.modal {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    width: 90%;
    max-width: 1000px;
    max-height: 85vh;
    overflow-y: auto;
    box-shadow: 0 25px 60px rgba(0, 0, 0, 0.5);
}
.modal-header {
    padding: 1rem 1.25rem;
    border-bottom: 1px solid var(--border);
    display: flex;
    justify-content: space-between;
    align-items: center;
    position: sticky;
    top: 0;
    background: var(--surface);
}
.modal-body { padding: 1.25rem; }
.hidden { display: none; }
input, select, textarea {
    font-family: var(--font);
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text);
    padding: 0.5rem;
}
input:focus, select:focus, textarea:focus {
    border-color: var(--accent);
    outline: none;
}
"#,
    )
}

/// Dashboard-specific styles.
pub fn dashboard_styles() -> String {
    String::from(
        r#"
.dashboard-grid {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 1rem;
}
@media (max-width: 1200px) {
    .dashboard-grid { grid-template-columns: repeat(2, 1fr); }
}
@media (max-width: 768px) {
    .dashboard-grid { grid-template-columns: 1fr; }
}
.quick-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
}
"#,
    )
}
