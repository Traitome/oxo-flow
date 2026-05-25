//! Dashboard page template for oxo-flow-web.
//!
//! Displays system metrics, active runs, quick actions, and recent workflows
//! with real-time updates via HTMX and SSE.

use maud::{DOCTYPE, Markup, html};

use super::partials::{
    badge, card, component_styles, dashboard_styles, layout_styles, sidebar, stat_card, table,
    theme,
};

/// Dashboard page with all components.
pub fn dashboard_page(username: &str, metrics_json: &str) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="UTF-8";
                meta name="viewport" content="width=device-width, initial-scale=1.0";
                title { "Dashboard — oxo-flow" }

                // Alpine.js for client-side interactivity
                script src="https://unpkg.com/alpinejs@3.x.x/dist/cdn.min.js" defer {}

                // Chart.js for resource charts
                script src="https://cdn.jsdelivr.net/npm/chart.js@4.4.1/dist/chart.umd.min.js" {}

                // HTMX for AJAX interactions
                script src="https://unpkg.com/htmx.org@1.9.10/dist/htmx.min.js" {}

                // HTMX SSE extension for real-time updates
                script src="https://unpkg.com/htmx.org@1.9.10/dist/ext/sse.js" {}

                // Styles
                style {
                    (layout_styles())
                    (component_styles())
                    (dashboard_styles())
                }
            }
            body {
                // SSE connection for real-time metrics
                div
                    hx-ext="sse"
                    sse-connect="/api/events"
                    sse-swap="message"
                    hx-trigger="sse:metrics"
                    hx-get="/api/metrics"
                    hx-target="#metrics-panel"
                {}

                (sidebar("dashboard", username))

                div class="main" {
                    // Topbar with live metrics (polling fallback)
                    div class="topbar" {
                        h2 { "Dashboard" }
                        div class="topbar-meta"
                            hx-get="/api/metrics"
                            hx-trigger="every 5s"
                            hx-target="this"
                            hx-swap="innerHTML"
                        {
                            span { "CPU " span id="live-cpu" { "--" } }
                            span { "MEM " span id="live-mem" { "--" } }
                            span { "Runs " span id="live-runs" { "0" } }
                            span id="live-uptime" { "--" }
                        }
                    }

                    div class="content" x-data=(format!("{{ chartData: {} }}", metrics_json)) {
                        // Resource metrics panel
                        div id="metrics-panel" class="stats-grid"
                            hx-get="/api/metrics"
                            hx-trigger="load, every 30s"
                        {
                            // CPU usage card
                            (stat_card("--%", "CPU Usage", Some("System load"), Some(0)))

                            // Memory card
                            (stat_card("-- / -- GB", "Memory Used / Total", Some("Physical memory"), Some(0)))

                            // Active workflows card
                            (stat_card("0", "Active Workflows", Some("Total: 0"), None))

                            // Uptime card
                            (stat_card("--", "Server Uptime", Some("Requests: 0"), None))
                        }

                        // Dashboard layout with cards
                        div class="dashboard-grid" {
                            // Run status card
                            (card("Run Status",
                                html! {
                                    div id="run-status-content"
                                        hx-get="/api/runs?status=running"
                                        hx-trigger="load, every 10s"
                                    {
                                        p class="text-muted" { "Loading..." }
                                    }
                                },
                                Some(html! {
                                    button
                                        class="btn btn-sm btn-outline"
                                        hx-get="/api/runs"
                                        hx-target="#run-status-content"
                                    { "Refresh" }
                                })
                            ))

                            // Quick actions card
                            (card("Quick Actions",
                                html! {
                                    div class="quick-actions" {
                                        button
                                            class="btn btn-primary"
                                            hx-get="/editor"
                                            hx-push-url="true"
                                            hx-target="#main-content"
                                        { "New Workflow" }

                                        button
                                            class="btn btn-outline"
                                            hx-get="/workflows"
                                            hx-push-url="true"
                                            hx-target="#main-content"
                                        { "Browse Library" }

                                        button
                                            class="btn btn-outline"
                                            hx-post="/api/workflows/run"
                                            hx-target="#notification"
                                        { "Quick Run" }
                                    }
                                },
                                None
                            ))

                            // Resource chart card
                            (card("Resource Chart",
                                html! {
                                    canvas id="resource-chart" width="400" height="200" {}
                                    script {
                                        "new Chart(document.getElementById('resource-chart'), {"
                                        "  type: 'line',"
                                        "  data: { labels: [], datasets: [] },"
                                        "  options: {"
                                        "    responsive: true,"
                                        "    plugins: { legend: { labels: { color: '" (theme::TEXT) "' } } },"
                                        "    scales: {"
                                        "      x: { grid: { color: '" (theme::BORDER) "' }, ticks: { color: '" (theme::TEXT_MUTED) "' } },"
                                        "      y: { grid: { color: '" (theme::BORDER) "' }, ticks: { color: '" (theme::TEXT_MUTED) "' } }"
                                        "    }"
                                        "  }"
                                        "});"
                                    }
                                },
                                Some(html! {
                                    button
                                        class="btn btn-sm btn-outline"
                                        hx-get="/api/metrics/history"
                                        hx-target="#resource-chart"
                                    { "Refresh" }
                                })
                            ))
                        }

                        // Recent workflows card
                        (card("Recent Workflows",
                            html! {
                                (table(&["ID", "Workflow", "Status", "Started", "Actions"],
                                    html! {
                                        tbody
                                            id="recent-runs"
                                            hx-get="/api/runs?limit=10"
                                            hx-trigger="load, every 30s, sse:workflow_started, sse:workflow_completed"
                                        {
                                            tr {
                                                td colspan="5" class="text-muted" { "Loading recent runs..." }
                                            }
                                        }
                                    }
                                ))
                            },
                            Some(html! {
                                button
                                    class="btn btn-sm btn-outline"
                                    hx-get="/runs"
                                    hx-push-url="true"
                                    hx-target="#main-content"
                                { "View All" }
                            })
                        ))

                        // Notification area
                        div id="notification" class="notification" {}
                    }
                }
            }
        }
    }
}

/// Metrics panel partial for HTMX swap.
pub fn metrics_panel_partial(
    cpu_pct: f64,
    mem_used_gb: f64,
    mem_total_gb: f64,
    active_runs: i64,
    uptime_secs: f64,
    total_requests: u64,
) -> Markup {
    let mem_pct = if mem_total_gb > 0.0 {
        ((mem_used_gb / mem_total_gb) * 100.0) as u32
    } else {
        0
    };
    let cpu_int = cpu_pct as u32;

    html! {
        div class="stats-grid" {
            (stat_card(&format!("{:.1}%", cpu_pct), "CPU Usage",
                Some("System load"), Some(cpu_int)))

            (stat_card(&format!("{:.0} / {:.0} GB", mem_used_gb, mem_total_gb),
                "Memory Used / Total", Some("Physical memory"), Some(mem_pct)))

            (stat_card(&format!("{}", active_runs), "Active Workflows",
                Some(&format!("Total: {}", total_requests)), None))

            (stat_card(&format_duration(uptime_secs), "Server Uptime",
                Some(&format!("Requests: {}", total_requests)), None))
        }
    }
}

/// Recent runs table partial for HTMX swap.
pub fn recent_runs_partial(runs: &[super::RunSummary]) -> Markup {
    html! {
        @if runs.is_empty() {
            tr {
                td colspan="5" style={"color: " (theme::TEXT_MUTED)} { "No runs yet" }
            }
        } @else {
            @for run in runs.iter().take(10) {
                tr {
                    td style={"font-family: var(--mono); font-size: 0.72rem"} {
                        (truncate_id(&run.id, 8)) "..."
                    }
                    td { (run.workflow_name) }
                    td { (badge(&run.status)) }
                    td style={"font-size: 0.78rem; color: " (theme::TEXT_MUTED)} {
                        (format_timestamp(&run.started_at))
                    }
                    td {
                        button
                            class="btn btn-sm btn-outline"
                            hx-get={"/api/runs/" (run.id)}
                            hx-target="#run-detail-modal .modal-body"
                            onclick="document.getElementById('run-detail-modal').classList.remove('hidden')"
                        { "Detail" }
                        button
                            class="btn btn-sm btn-outline"
                            hx-get={"/api/runs/" (run.id) "/logs"}
                            hx-target="#log-modal .log-viewer"
                            onclick="document.getElementById('log-modal').classList.remove('hidden')"
                        { "Logs" }
                    }
                }
            }
        }
    }
}

/// Run status content partial.
pub fn run_status_partial(runs: &[super::RunSummary]) -> Markup {
    let running_count = runs.iter().filter(|r| r.status == "running").count();
    let completed_count = runs.iter().filter(|r| r.status == "completed").count();
    let failed_count = runs.iter().filter(|r| r.status == "failed").count();

    html! {
        div class="run-status-grid" {
            div class="status-item" {
                span class="status-count running" { (running_count) }
                span class="status-label" { "Running" }
            }
            div class="status-item" {
                span class="status-count success" { (completed_count) }
                span class="status-label" { "Completed" }
            }
            div class="status-item" {
                span class="status-count failed" { (failed_count) }
                span class="status-label" { "Failed" }
            }
        }
    }
}

// Helper functions

/// Format duration in human-readable format.
fn format_duration(secs: f64) -> String {
    if secs < 0.0 {
        return "--".to_string();
    }
    let h = (secs / 3600.0) as u32;
    let m = ((secs % 3600.0) / 60.0) as u32;
    let s = (secs % 60.0) as u32;

    if h > 0 {
        format!("{}h {}m", h, m)
    } else if m > 0 {
        format!("{}m {}s", m, s)
    } else {
        format!("{}s", s)
    }
}

/// Truncate ID string to specified length.
fn truncate_id(id: &str, len: usize) -> String {
    if id.len() > len {
        id[..len].to_string()
    } else {
        id.to_string()
    }
}

/// Format timestamp for display.
fn format_timestamp(ts: &Option<String>) -> String {
    match ts {
        Some(t) => {
            // Try to parse and format nicely
            t.split('.').next().unwrap_or(t).to_string()
        }
        None => "--".to_string(),
    }
}
