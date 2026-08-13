# Build a Workflow on the Web Canvas

The web editor turns the `.oxoflow` TOML format into an interactive node
canvas. The TOML stays the single source of truth: every canvas action goes
through the engine's parse → edit → format → validate round-trip, so what you
see on the canvas is exactly what the CLI would run.

---

## The three panes

| Pane | What it does |
|------|--------------|
| **Tools / Assistant rail** (left) | Search the embedded Bioconda database (6,100+ tools) and add real tools as rules; or ask the AI assistant to draft a workflow |
| **Pipeline DAG** (center) | The node canvas: drag, connect, delete, double-click to edit |
| **Workflow TOML** (right) | The canonical `.oxoflow` text — always in sync with the canvas |

## Adding a rule from the tool palette

1. Open the **Tools** tab and search (e.g. `fastp`).
2. Click **+** on a result. A new node appears with a grounded command
   (`fastp {input} -o {output}`) and the tool's real name and version in its
   description — never a stub.

## Editing a rule

Double-click a node to open the inspector:

- **Shell command** — free-form, with `{input}`/`{output}` placeholders.
- **Inputs / Outputs** — file path patterns; wildcards (`{sample}`) allowed.
- **Environment** — conda / mamba / docker / singularity / venv / modules /
  system, with the spec string.
- **Resources** — threads, memory, GPU, disk, time limit.
- **Conditions, retries, tags, logs, benchmarks** — the workflow format's
  execution controls.

Fields the inspector doesn't cover stay editable in the TOML pane.

## Connecting rules

Drag from a node's right handle to another node's left handle. This adds an
explicit `depends_on` edge.

There are two kinds of edges, and the canvas draws them differently:

| Style | Kind | How to change it |
|-------|------|------------------|
| Solid | `depends_on` (declared) | Drag handles, or edit the TOML |
| Dashed | file-inferred | Edit the `input`/`output` paths — the engine infers the edge from exact string matches |

The distinction matters: the engine builds file edges by exact
input/output string matching only, so wildcard patterns infer no edges —
`depends_on` is the explicit ordering tool.

## Deleting

Select one or more nodes and press `Delete` (or `Backspace`). Removing a rule
whose outputs other rules consume will surface validation errors on those
rules — fix their inputs, and the workflow is valid again.

## Auto layout

Use **Auto layout** to re-run the layered Sugiyama layout; then drag nodes
wherever you like — positions are saved per pipeline (presentation only, never
part of the workflow file).

## Validation, dry-run, run

The badge above the TOML pane reports the engine's verdict live. **Dry-Run**
produces the execution plan without running anything; **Run** starts the real
pipeline and opens the monitor. Both honor the engine's checkpoint semantics:
re-runs reuse completed rules unless inputs, config, or rule definitions
changed.

## See also

- [DAG Edit API](../reference/dag-edit-api.md) — the command API behind the canvas
- [Workflow Format](../reference/workflow-format.md) — full rule field reference
- [DAG Engine](../reference/dag-engine.md) — how dependencies are inferred
