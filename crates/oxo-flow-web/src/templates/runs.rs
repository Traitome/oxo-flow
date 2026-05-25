//! Run detail and log templates for oxo-flow-web.
//!
//! Provides templates for viewing run history, run details, and live logs.

use maud::{DOCTYPE, Markup, html};

use super::partials::{
    badge, card, component_styles, layout_styles, log_viewer, sidebar, table, theme,
};

/// Run history page listing all runs.
pub fn runs_page(username: &str, runs: &[RunSummary]) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="UTF-8";
                meta name="viewport" content="width=device-width, initial-scale=1.0";
                title { "Run History — oxo-flow" }

                // HTMX for interactions
                script src="https://unpkg.com/htmx.org@1.9.10/dist/htmx.min.js" {}

                // HTMX SSE extension for real-time updates
                script src="https://unpkg.com/htmx.org@1.9.10/dist/ext/sse.js" {}

                // Styles
                style {
                    (layout_styles())
                    (component_styles())
                }
            }
            body {
                // SSE connection for run updates
                div
                    hx-ext="sse"
                    sse-connect="/api/events"
                    hx-trigger="sse:workflow_started, sse:workflow_completed, sse:run_cancelled"
                    hx-get="/api/runs"
                    hx-target="#runs-table tbody"
                {}

                (sidebar("runs", username))

                div class="main" {
                    div class="topbar" {
                        h2 { "Run History" }
                        div class="topbar-meta" {
                            span { (runs.len()) " total runs" }
                        }
                    }

                    div class="content" {
                        // Filter controls
                        div class="filter-bar" {
                            select
                                id="status-filter"
                                hx-get="/api/runs"
                                hx-trigger="change"
                                hx-target="#runs-table tbody"
                            {
                                option value="" { "All statuses" }
                                option value="running" { "Running" }
                                option value="completed" { "Completed" }
                                option value="failed" { "Failed" }
                                option value="pending" { "Pending" }
                            }

                            input
                                type="search"
                                id="search-filter"
                                placeholder="Search workflows..."
                                hx-get="/api/runs"
                                hx-trigger="keyup changed delay:300ms"
                                hx-target="#runs-table tbody"
                            {}
                        }

                        // Runs table
                        (card("All Runs",
                            html! {
                                div id="runs-table" {
                                    (table(&["Run ID", "Workflow", "Status", "Started", "Duration", "Actions"],
                                        html! {
                                            tbody
                                                hx-get="/api/runs"
                                                hx-trigger="load, every 30s"
                                            {
                                                @if runs.is_empty() {
                                                    tr {
                                                        td colspan="6" style={"color: " (theme::TEXT_MUTED)} {
                                                            "No runs yet"
                                                        }
                                                    }
                                                } @else {
                                                    @for run in runs {
                                                        (run_row(run))
                                                    }
                                                }
                                            }
                                        }
                                    ))
                                }
                            },
                            Some(html! {
                                button
                                    class="btn btn-sm btn-outline"
                                    hx-get="/api/runs"
                                    hx-target="#runs-table tbody"
                                { "Refresh" }
                            })
                        ))
                    }
                }
            }
        }
    }
}

/// Single run row for the table.
fn run_row(run: &RunSummary) -> Markup {
    html! {
        tr id={"run-" (run.id)} {
            td style="font-family: var(--mono); font-size: 0.72rem" {
                a
                    href={"/runs/" (run.id)}
                    hx-get={"/runs/" (run.id)}
                    hx-target="#main-content"
                    hx-push-url="true"
                {
                    (truncate_id(&run.id, 12)) "..."
                }
            }
            td { (run.workflow_name) }
            td { (badge(&run.status)) }
            td style={"font-size: 0.78rem; color: " (theme::TEXT_MUTED)} {
                (format_timestamp(&run.started_at))
            }
            td style={"font-size: 0.78rem; color: " (theme::TEXT_MUTED)} {
                (format_duration(&run.started_at, &run.finished_at))
            }
            td {
                button
                    class="btn btn-sm btn-outline"
                    hx-get={"/api/runs/" (run.id)}
                    hx-target="#detail-modal .modal-body"
                    onclick="document.getElementById('detail-modal').classList.remove('hidden')"
                { "Detail" }

                button
                    class="btn btn-sm btn-outline"
                    hx-get={"/api/runs/" (run.id) "/logs"}
                    hx-target="#log-modal .log-viewer"
                    onclick="document.getElementById('log-modal').classList.remove('hidden')"
                { "Logs" }

                @if run.status == "running" {
                    button
                        class="btn btn-sm btn-danger"
                        hx-delete={"/api/runs/" (run.id)}
                        hx-target="closest tr"
                        hx-swap="outerHTML swap:1s"
                        hx-confirm="Cancel this run?"
                    { "Cancel" }
                }
            }
        }
    }
}

/// Run detail page showing full run information.
pub fn run_detail_page(username: &str, run: &RunDetail) -> Markup {
    let pid_display = run
        .pid
        .map(|p| p.to_string())
        .unwrap_or_else(|| "--".to_string());
    let rules_display = run
        .rules_total
        .map(|r| r.to_string())
        .unwrap_or_else(|| "--".to_string());

    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="UTF-8";
                meta name="viewport" content="width=device-width, initial-scale=1.0";
                title { "Run " (truncate_id(&run.id, 8)) " — oxo-flow" }

                // HTMX
                script src="https://unpkg.com/htmx.org@1.9.10/dist/htmx.min.js" {}

                // HTMX SSE for live updates
                script src="https://unpkg.com/htmx.org@1.9.10/dist/ext/sse.js" {}

                // Styles
                style {
                    (layout_styles())
                    (component_styles())
                    (detail_styles())
                }
            }
            body {
                // SSE for live log updates
                div
                    hx-ext="sse"
                    sse-connect="/api/events"
                    sse-swap="message"
                    hx-trigger="sse:log_update"
                    hx-get={"/api/runs/" (run.id) "/logs"}
                    hx-target="#live-log"
                {}

                (sidebar("runs", username))

                div class="main" {
                    div class="topbar" {
                        h2 { "Run " (truncate_id(&run.id, 8)) }
                        div class="topbar-meta" {
                            (badge(&run.status))
                            span { (run.workflow_name) }
                        }
                    }

                    div class="content" {
                        // Run metrics
                        div class="stats-grid" {
                            div class="stat-card" {
                                div class="value" { (truncate_id(&run.id, 8)) }
                                div class="label" { "Run ID" }
                            }
                            div class="stat-card" {
                                div class="value" { (pid_display) }
                                div class="label" { "Process ID" }
                            }
                            div class="stat-card" {
                                div class="value" {
                                    @if run.started_at.is_some() && run.finished_at.is_some() {
                                        (format_duration(&run.started_at, &run.finished_at))
                                    } @else {
                                        "--"
                                    }
                                }
                                div class="label" { "Duration" }
                            }
                            div class="stat-card" {
                                div class="value" { (rules_display) }
                                div class="label" { "Rules" }
                            }
                        }

                        // Timing details
                        (card("Timing",
                            html! {
                                div class="timing-grid" {
                                    @if let Some(started) = &run.started_at {
                                        div {
                                            span class="timing-label" { "Started:" }
                                            span { (started) }
                                        }
                                    }
                                    @if let Some(finished) = &run.finished_at {
                                        div {
                                            span class="timing-label" { "Finished:" }
                                            span { (finished) }
                                        }
                                    }
                                    @if run.started_at.is_some() {
                                        div
                                            hx-get={"/api/runs/" (run.id) "/logs"}
                                            hx-trigger="every 5s"
                                            hx-target="#timing-update"
                                        {
                                            span id="timing-update" class="timing-label" {
                                                "Elapsed: " (format_elapsed(&run.started_at))
                                            }
                                        }
                                    }
                                }
                            },
                            None
                        ))

                        // Output files
                        @if let Some(files) = &run.output_files {
                            @if !files.is_empty() {
                                (card("Output Files",
                                    html! {
                                        ul class="output-list" {
                                            @for file in files {
                                                li {
                                                    code { (file) }
                                                }
                                            }
                                        }
                                    },
                                    Some(html! {
                                        button
                                            class="btn btn-sm btn-outline"
                                            hx-get={"/api/runs/" (run.id) "/outputs"}
                                            hx-target=".output-list"
                                        { "Refresh" }
                                    })
                                ))
                            }
                        }

                        // Live log viewer
                        (card("Live Log",
                            html! {
                                div
                                    id="live-log"
                                    hx-get={"/api/runs/" (run.id) "/logs"}
                                    hx-trigger="load, every 2s"
                                {
                                    (log_viewer(run.log_tail.as_deref().unwrap_or("Loading...")))
                                }
                            },
                            Some(html! {
                                button
                                    class="btn btn-sm btn-outline"
                                    hx-get={"/api/runs/" (run.id) "/logs"}
                                    hx-target="#live-log"
                                { "Refresh" }
                            })
                        ))

                        // Actions
                        div class="action-bar" {
                            @if run.status == "running" {
                                button
                                    class="btn btn-danger"
                                    hx-delete={"/api/runs/" (run.id)}
                                    hx-confirm="Cancel this run?"
                                    hx-target="#run-status"
                                { "Cancel Run" }
                            }

                            button
                                class="btn btn-outline"
                                hx-get="/runs"
                                hx-push-url="true"
                                hx-target="#main-content"
                            { "Back to History" }
                        }
                    }
                }
            }
        }
    }
}

/// Run detail modal for quick viewing from the runs table.
pub fn run_detail_modal(run: &RunDetail) -> Markup {
    html! {
        div class="modal-body" {
            div class="detail-header" {
                h3 { "Run " (truncate_id(&run.id, 8)) }
                (badge(&run.status))
            }

            div class="detail-info" {
                p { "Workflow: " (run.workflow_name) }
                @if let Some(pid) = run.pid {
                    p { "PID: " (pid) }
                }
                @if let Some(started) = &run.started_at {
                    p { "Started: " (started) }
                }
                @if let Some(finished) = &run.finished_at {
                    p { "Finished: " (finished) }
                }
                @if let Some(rules) = run.rules_total {
                    p { "Rules: " (rules) }
                }
            }

            @if let Some(files) = &run.output_files {
                @if !files.is_empty() {
                    div class="detail-output" {
                        h4 { "Output Files:" }
                        ul {
                            @for file in files {
                                li { code { (file) } }
                            }
                        }
                    }
                }
            }

            @if let Some(log) = &run.log_tail {
                div class="detail-log" {
                    h4 { "Log Tail:" }
                    (log_viewer(log))
                }
            }
        }
    }
}

/// Log viewer modal for streaming logs.
pub fn log_modal(run_id: &str) -> Markup {
    html! {
        div id="log-modal" class="modal-overlay hidden" {
            div class="modal" {
                div class="modal-header" {
                    h3 { "Run Logs — " (truncate_id(run_id, 8)) }
                    button
                        class="btn btn-outline btn-sm"
                        onclick="document.getElementById('log-modal').classList.add('hidden')"
                    { "Close" }
                }
                div class="modal-body" {
                    div class="log-viewer" hx-get={"/api/runs/" (run_id) "/logs"} {
                        "Loading logs..."
                    }
                }
            }
        }
    }
}

/// Runs table partial for HTMX swap.
pub fn runs_table_partial(runs: &[RunSummary]) -> Markup {
    html! {
        @if runs.is_empty() {
            tr {
                td colspan="6" style={"color: " (theme::TEXT_MUTED)} { "No runs found" }
            }
        } @else {
            @for run in runs {
                (run_row(run))
            }
        }
    }
}

/// Run status update partial for SSE updates.
pub fn run_status_partial(status: &str, id: &str) -> Markup {
    html! {
        tr id={"run-" (id)} {
            td colspan="6" {
                span class="badge" { (status) }
                span { "Run " (truncate_id(id, 8)) " updated" }
            }
        }
    }
}

// Helper functions

fn truncate_id(id: &str, len: usize) -> String {
    if id.len() > len {
        id[..len].to_string()
    } else {
        id.to_string()
    }
}

fn format_timestamp(ts: &Option<String>) -> String {
    ts.as_ref()
        .map(|t| t.split('.').next().unwrap_or(t).to_string())
        .unwrap_or_else(|| "--".to_string())
}

fn format_duration(start: &Option<String>, end: &Option<String>) -> String {
    match (start, end) {
        (Some(s), Some(e)) => {
            // Parse timestamps and compute duration
            let start_ts = chrono::DateTime::parse_from_rfc3339(s);
            let end_ts = chrono::DateTime::parse_from_rfc3339(e);
            match (start_ts, end_ts) {
                (Ok(s_dt), Ok(e_dt)) => {
                    let diff = e_dt.signed_duration_since(s_dt);
                    let h = diff.num_hours();
                    let m = diff.num_minutes() % 60;
                    let sec = diff.num_seconds() % 60;
                    if h > 0 {
                        format!("{}h {}m", h, m)
                    } else if m > 0 {
                        format!("{}m {}s", m, sec)
                    } else {
                        format!("{}s", sec)
                    }
                }
                _ => "--".to_string(),
            }
        }
        _ => "--".to_string(),
    }
}

fn format_elapsed(start: &Option<String>) -> String {
    match start {
        Some(s) => {
            let start_ts = chrono::DateTime::parse_from_rfc3339(s);
            match start_ts {
                Ok(s_dt) => {
                    let now = chrono::Utc::now();
                    let diff = now.signed_duration_since(s_dt);
                    let h = diff.num_hours();
                    let m = diff.num_minutes() % 60;
                    let sec = diff.num_seconds() % 60;
                    if h > 0 {
                        format!("{}h {}m {}s", h, m, sec)
                    } else if m > 0 {
                        format!("{}m {}s", m, sec)
                    } else {
                        format!("{}s", sec)
                    }
                }
                _ => "--".to_string(),
            }
        }
        None => "--".to_string(),
    }
}

// Detail page specific styles
fn detail_styles() -> String {
    String::from(
        r#"
.timing-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 1rem;
}
.timing-label {
    color: var(--text-muted);
    font-size: 0.78rem;
}
.output-list {
    list-style: none;
    padding: 0;
}
.output-list li {
    padding: 0.25rem 0;
    border-bottom: 1px solid var(--border);
}
.output-list li:last-child {
    border-bottom: none;
}
.action-bar {
    display: flex;
    gap: 0.5rem;
    margin-top: 1rem;
}
.filter-bar {
    display: flex;
    gap: 1rem;
    margin-bottom: 1rem;
}
.filter-bar select,
.filter-bar input {
    padding: 0.5rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text);
    font-family: var(--font);
}
.filter-bar input[type="search"] {
    flex: 1;
    max-width: 300px;
}
"#,
    )
}

// Data types

/// Summary of a run for list display.
#[derive(Debug, Clone)]
pub struct RunSummary {
    pub id: String,
    pub workflow_name: String,
    pub status: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

/// Full run detail for detail page.
#[derive(Debug, Clone)]
pub struct RunDetail {
    pub id: String,
    pub workflow_name: String,
    pub status: String,
    pub pid: Option<u32>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub rules_total: Option<usize>,
    pub output_files: Option<Vec<String>>,
    pub log_tail: Option<String>,
}
