//! Workflow editor and list templates for oxo-flow-web.
//!
//! Provides templates for creating, editing, validating, and managing workflows.

use maud::{DOCTYPE, Markup, html};

use super::partials::{card, component_styles, layout_styles, sidebar, table, theme};

/// Workflow editor page with TOML editor and action sidebar.
pub fn workflow_editor_page(username: &str, initial_content: Option<&str>) -> Markup {
    let default_content = initial_content.unwrap_or(
        "[workflow]\nname = \"my-pipeline\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"hello\"\noutput = [\"hello.txt\"]\nshell = \"echo Hello, oxo-flow! > {output[0]}\"\n",
    );

    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="UTF-8";
                meta name="viewport" content="width=device-width, initial-scale=1.0";
                title { "Workflow Editor — oxo-flow" }

                // Alpine.js for editor state
                script src="https://unpkg.com/alpinejs@3.x.x/dist/cdn.min.js" defer {}

                // HTMX for API interactions
                script src="https://unpkg.com/htmx.org@1.9.10/dist/htmx.min.js" {}

                // Styles
                style {
                    (layout_styles())
                    (component_styles())
                    (editor_styles())
                }
            }
            body {
                (sidebar("editor", username))

                div class="main" {
                    div class="topbar" {
                        h2 { "Workflow Editor" }
                        div class="topbar-meta" {
                            span id="editor-status" { "Ready" }
                        }
                    }

                    div class="content" x-data="editorState()" {
                        div class="editor-layout" {
                            // Main editor panel
                            div class="editor-panel" {
                                textarea
                                    id="toml-editor"
                                    x-model="content"
                                    spellcheck="false"
                                    placeholder="Enter TOML workflow content..."
                                    hx-post="/api/workflows/validate"
                                    hx-trigger="change delayed:500ms"
                                    hx-target="#validation-output"
                                    hx-indicator="#editor-indicator"
                                {
                                    (default_content)
                                }

                                // Validation output area
                                div id="validation-output" class="output hidden" {}

                                // Loading indicator
                                div id="editor-indicator" class="htmx-indicator" {
                                    span { "Validating..." }
                                }
                            }

                            // Sidebar with actions and info
                            div class="editor-sidebar" {
                                // Actions card
                                (card("Actions",
                                    html! {
                                        div class="action-buttons" {
                                            button
                                                class="btn btn-primary"
                                                style="width: 100%; margin-bottom: 0.5rem"
                                                hx-post="/api/workflows/run"
                                                hx-target="#run-output"
                                                hx-include="#toml-editor"
                                            { "Launch Workflow" }

                                            button
                                                class="btn btn-outline"
                                                style="width: 100%; margin-bottom: 0.35rem"
                                                hx-post="/api/workflows/validate"
                                                hx-target="#validation-output"
                                                hx-include="#toml-editor"
                                            { "Validate" }

                                            button
                                                class="btn btn-outline"
                                                style="width: 100%; margin-bottom: 0.35rem"
                                                hx-post="/api/workflows/dry-run"
                                                hx-target="#validation-output"
                                                hx-include="#toml-editor"
                                            { "Dry Run" }

                                            button
                                                class="btn btn-outline"
                                                style="width: 100%; margin-bottom: 0.35rem"
                                                hx-post="/api/workflows/format"
                                                hx-target="#toml-editor"
                                                hx-swap="innerHTML"
                                                hx-include="#toml-editor"
                                            { "Format" }

                                            button
                                                class="btn btn-outline"
                                                style="width: 100%; margin-bottom: 0.35rem"
                                                hx-post="/api/workflows/lint"
                                                hx-target="#validation-output"
                                                hx-include="#toml-editor"
                                            { "Lint" }

                                            button
                                                class="btn btn-outline"
                                                style="width: 100%; margin-bottom: 0.35rem"
                                                hx-post="/api/workflows/dag"
                                                hx-target="#dag-modal .modal-body"
                                                hx-include="#toml-editor"
                                                onclick="document.getElementById('dag-modal').classList.remove('hidden')"
                                            { "View DAG" }

                                            button
                                                class="btn btn-outline"
                                                style="width: 100%; margin-bottom: 0.35rem"
                                                hx-post="/api/workflows/export"
                                                hx-target="#export-modal .modal-body"
                                                hx-include="#toml-editor"
                                                hx-vals="{\"format\": \"docker\"}"
                                                onclick="document.getElementById('export-modal').classList.remove('hidden')"
                                            { "Export Dockerfile" }
                                        }
                                    },
                                    None
                                ))

                                // Templates card
                                (card("Templates",
                                    html! {
                                        button
                                            class="btn btn-sm btn-outline"
                                            style="width: 100%; margin-bottom: 0.5rem"
                                            onclick="clearEditor()"
                                        { "+ New Workflow" }

                                        select
                                            id="template-select"
                                            style="width: 100%"
                                            onchange="loadTemplate(this)"
                                        {
                                            option value="" { "-- Quick template --" }
                                            option value="hello" { "Hello World" }
                                            option value="wgs" { "WGS Germline" }
                                            option value="rnaseq" { "RNA-seq" }
                                            option value="paired" { "Tumor-Normal Paired" }
                                            option value="cohort" { "Multi-Sample Cohort" }
                                            option value="scatter" { "Scatter-Gather" }
                                            option value="conditional" { "Conditional Rules" }
                                        }
                                    },
                                    None
                                ))

                                // Save card
                                (card("Save to Library",
                                    html! {
                                        input
                                            id="workflow-name"
                                            type="text"
                                            placeholder="Workflow name"
                                            style="width: 100%; margin-bottom: 0.5rem"
                                        {}

                                        button
                                            class="btn btn-outline"
                                            style="width: 100%"
                                            hx-post="/api/workflows/save"
                                            hx-target="#save-result"
                                            hx-include="#toml-editor,#workflow-name"
                                            hx-vals="{\"version\": \"1.0.0\"}"
                                        { "Save Workflow" }

                                        div id="save-result" {}
                                    },
                                    None
                                ))

                                // Statistics card
                                (card("Statistics",
                                    html! {
                                        div
                                            id="editor-stats"
                                            hx-post="/api/workflows/stats"
                                            hx-trigger="load, change from:#toml-editor"
                                            hx-include="#toml-editor"
                                            hx-target="this"
                                        {
                                            p style={"color: " (theme::TEXT_MUTED); "font-size: 0.78rem"} {
                                                "Open a workflow to see stats"
                                            }
                                        }
                                    },
                                    None
                                ))
                            }
                        }

                        // Run output notification
                        div id="run-output" class="notification" {}

                        // DAG modal
                        (dag_modal())

                        // Export modal
                        (export_modal())
                    }
                }

                // JavaScript helpers
                script {
                    (editor_helpers_script())
                }
            }
        }
    }
}

/// Workflow list page (saved workflows library).
pub fn workflow_list_page(username: &str, workflows: &[WorkflowListItem]) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="UTF-8";
                meta name="viewport" content="width=device-width, initial-scale=1.0";
                title { "Saved Workflows — oxo-flow" }

                // HTMX for interactions
                script src="https://unpkg.com/htmx.org@1.9.10/dist/htmx.min.js" {}

                // Styles
                style {
                    (layout_styles())
                    (component_styles())
                }
            }
            body {
                (sidebar("workflows", username))

                div class="main" {
                    div class="topbar" {
                        h2 { "Saved Workflows" }
                        div class="topbar-meta" {
                            span { (workflows.len()) " workflows" }
                        }
                    }

                    div class="content" {
                        (card("Workflow Library",
                            html! {
                                (table(&["Name", "Version", "Rules", "Updated", "Actions"],
                                    html! {
                                        @if workflows.is_empty() {
                                            tr {
                                                td colspan="5" style={"color: " (theme::TEXT_MUTED)} {
                                                    "No saved workflows"
                                                }
                                            }
                                        } @else {
                                            @for wf in workflows {
                                                tr {
                                                    td { (wf.name) }
                                                    td { (wf.version) }
                                                    td { (wf.rules_count) }
                                                    td style={"font-size: 0.78rem; color: " (theme::TEXT_MUTED)} {
                                                        (format_timestamp(&wf.updated_at))
                                                    }
                                                    td {
                                                        button
                                                            class="btn btn-sm btn-outline"
                                                            hx-get={"/api/workflows/saved/" (wf.id)}
                                                            hx-target="#toml-editor"
                                                            onclick="window.location.href='/editor'"
                                                        { "Load" }

                                                        button
                                                            class="btn btn-sm btn-danger"
                                                            style="margin-left: 0.25rem"
                                                            hx-delete={"/api/workflows/saved/" (wf.id)}
                                                            hx-target="closest tr"
                                                            hx-swap="outerHTML"
                                                            hx-confirm={"Delete workflow \"" (wf.name) "\"?"}
                                                        { "Del" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                ))
                            },
                            Some(html! {
                                button
                                    class="btn btn-sm btn-outline"
                                    hx-get="/api/workflows/saved"
                                    hx-target=".card tbody"
                                { "Refresh" }
                            })
                        ))
                    }
                }
            }
        }
    }
}

/// Workflow detail page showing parsed workflow information.
pub fn workflow_detail_page(username: &str, workflow: &WorkflowDetail) -> Markup {
    let workflow_id_json = format!("{{\"workflow_id\": \"{}\"}}", workflow.id);

    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="UTF-8";
                meta name="viewport" content="width=device-width, initial-scale=1.0";
                title { (workflow.name) " — oxo-flow" }

                // HTMX
                script src="https://unpkg.com/htmx.org@1.9.10/dist/htmx.min.js" {}

                // Styles
                style {
                    (layout_styles())
                    (component_styles())
                }
            }
            body {
                (sidebar("workflows", username))

                div class="main" {
                    div class="topbar" {
                        h2 { (workflow.name) " v" (workflow.version) }
                        div class="topbar-meta" {
                            span { (workflow.rules_count) " rules" }
                        }
                    }

                    div class="content" {
                        @if let Some(desc) = &workflow.description {
                            (card("Description",
                                html! { p { (desc) } },
                                None
                            ))
                        }

                        @if let Some(author) = &workflow.author {
                            (card("Author",
                                html! { p { (author) } },
                                None
                            ))
                        }

                        (card("Rules",
                            html! {
                                (table(&["Name", "Inputs", "Outputs", "Environment", "Threads"],
                                    html! {
                                        @for rule in &workflow.rules {
                                            tr {
                                                td { (rule.name) }
                                                td {
                                                    @for input in &rule.inputs {
                                                        code { (input) }
                                                        br;
                                                    }
                                                }
                                                td {
                                                    @for output in &rule.outputs {
                                                        code { (output) }
                                                        br;
                                                    }
                                                }
                                                td { (rule.environment) }
                                                td { (rule.threads) }
                                            }
                                        }
                                    }
                                ))
                            },
                            Some(html! {
                                button
                                    class="btn btn-primary"
                                    hx-post="/api/workflows/run"
                                    hx-target="#run-result"
                                    hx-vals=(workflow_id_json)
                                { "Run Workflow" }
                            })
                        ))
                    }
                }
            }
        }
    }
}

/// Workflow statistics partial for HTMX swap.
pub fn workflow_stats_partial(stats: &WorkflowStats) -> Markup {
    html! {
        div style="font-size: 0.78rem; color: var(--text-muted)" {
            p { "Rules: " (stats.rule_count) }
            p { "Shell: " (stats.shell_rules) " | Script: " (stats.script_rules) }
            p { "Dependencies: " (stats.dependency_count) }
            p { "Parallel groups: " (stats.parallel_groups) }
            p { "Total threads: " (stats.total_threads) }
            p { "Environments: " (stats.environments.join(", ")) }
            @if stats.wildcard_count > 0 {
                p { "Wildcards: " (stats.wildcard_count) " (" (stats.wildcard_names.join(", ")) ")" }
            }
        }
    }
}

/// DAG modal component.
fn dag_modal() -> Markup {
    html! {
        div id="dag-modal" class="modal-overlay hidden" {
            div class="modal" {
                div class="modal-header" {
                    h3 { "Workflow DAG" }
                    button
                        class="btn btn-outline btn-sm"
                        onclick="document.getElementById('dag-modal').classList.add('hidden')"
                    { "Close" }
                }
                div class="modal-body" {
                    pre style={
                        "font-family: var(--mono); "
                        "font-size: 0.75rem; "
                        "color: " (theme::TEXT_MUTED) "; "
                        "white-space: pre-wrap; "
                        "max-height: 60vh; "
                        "overflow-y: auto"
                    } {
                        "Loading DAG..."
                    }
                }
            }
        }
    }
}

/// Export modal component.
fn export_modal() -> Markup {
    html! {
        div id="export-modal" class="modal-overlay hidden" {
            div class="modal" style="max-width: 800px" {
                div class="modal-header" {
                    h3 { "Dockerfile Export" }
                    button
                        class="btn btn-outline btn-sm"
                        onclick="document.getElementById('export-modal').classList.add('hidden')"
                    { "Close" }
                }
                div class="modal-body" {
                    pre style={
                        "font-family: var(--mono); "
                        "font-size: 0.75rem; "
                        "color: " (theme::TEXT_MUTED) "; "
                        "white-space: pre-wrap; "
                        "max-height: 60vh; "
                        "overflow-y: auto; "
                        "background: " (theme::BG) "; "
                        "padding: 1rem; "
                        "border-radius: var(--radius)"
                    } {
                        "Loading export..."
                    }
                }
            }
        }
    }
}

/// Editor-specific styles.
fn editor_styles() -> String {
    String::from(
        r#"
.editor-layout {
    display: grid;
    grid-template-columns: 1fr 380px;
    gap: 1rem;
}
@media (max-width: 1000px) {
    .editor-layout { grid-template-columns: 1fr; }
}
.editor-panel textarea {
    width: 100%;
    min-height: 500px;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 1rem;
    font-family: var(--mono);
    font-size: 0.82rem;
    line-height: 1.6;
    resize: vertical;
    outline: none;
}
.editor-panel textarea:focus { border-color: var(--accent); }
.editor-sidebar {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
}
.output {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 0.75rem;
    font-family: var(--mono);
    font-size: 0.78rem;
    white-space: pre-wrap;
    max-height: 300px;
    overflow-y: auto;
    color: var(--text-muted);
    margin-top: 0.5rem;
}
.output.error { border-color: var(--error); color: var(--error); }
.output.success { border-color: var(--success); }
.htmx-indicator { display: none; }
.htmx-request .htmx-indicator { display: inline; }
"#,
    )
}

/// JavaScript helper functions for editor.
fn editor_helpers_script() -> String {
    String::from(
        r#"
function clearEditor() {
    const editor = document.getElementById('toml-editor');
    if (editor) editor.value = '';
    const nameInput = document.getElementById('workflow-name');
    if (nameInput) nameInput.value = '';
    document.getElementById('validation-output').classList.add('hidden');
}

function loadTemplate(select) {
    const templates = {
        hello: '[workflow]\nname = "hello-world"\nversion = "1.0.0"\n\n[[rules]]\nname = "greet"\noutput = ["hello.txt"]\nshell = "echo Hello, oxo-flow! > {output[0]}"\n',
        wgs: '[workflow]\nname = "wgs-germline"\nversion = "1.0.0"\n\n[config]\nref = "/path/to/reference.fa"\ndata = "/path/to/fastq"\n\n[[rules]]\nname = "fastp_trim"\noutput = ["trimmed.fq.gz"]\nshell = "fastp -i {input} -o {output}"\n',
        rnaseq: '[workflow]\nname = "rnaseq"\nversion = "1.0.0"\n\n[[rules]]\nname = "salmon_quant"\noutput = ["quant.sf"]\nshell = "salmon quant -i index -l A -1 {input} -o output"\n',
    };
    const template = templates[select.value];
    if (template) {
        document.getElementById('toml-editor').value = template;
        document.getElementById('workflow-name').value = select.value;
    }
    select.value = '';
}
"#,
    )
}

// Helper functions

fn format_timestamp(ts: &Option<String>) -> String {
    ts.as_ref()
        .map(|t| t.split('.').next().unwrap_or(t).to_string())
        .unwrap_or_else(|| "--".to_string())
}

// Data types for templates

/// Workflow list item for the library view.
#[derive(Debug, Clone)]
pub struct WorkflowListItem {
    pub id: String,
    pub name: String,
    pub version: String,
    pub rules_count: usize,
    pub updated_at: Option<String>,
}

/// Full workflow detail including rules.
#[derive(Debug, Clone)]
pub struct WorkflowDetail {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub rules_count: usize,
    pub rules: Vec<RuleSummary>,
}

/// Rule summary for display.
#[derive(Debug, Clone)]
pub struct RuleSummary {
    pub name: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub environment: String,
    pub threads: u32,
}

/// Workflow statistics for the stats partial.
#[derive(Debug, Clone)]
pub struct WorkflowStats {
    pub rule_count: usize,
    pub shell_rules: usize,
    pub script_rules: usize,
    pub dependency_count: usize,
    pub parallel_groups: usize,
    pub max_depth: usize,
    pub total_threads: u32,
    pub environments: Vec<String>,
    pub wildcard_count: usize,
    pub wildcard_names: Vec<String>,
}
