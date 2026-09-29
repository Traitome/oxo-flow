# Build a Workflow on the Web Canvas

The web editor turns the `.oxoflow` TOML format into an interactive node
canvas. The TOML stays the single source of truth: every canvas action goes
through the engine's parse → edit → format → validate round-trip, so what you
see on the canvas is exactly what the CLI would run.

---

## Guided vs Canvas modes

The editor offers two views for the same workflow (TOML stays the single
source of truth):

- **Guided** (default for new users): one form card per rule — name,
  command, inputs, outputs, threads, memory, environment. Every change is
  converted to workflow TOML immediately; cards round-trip through canvas
  or AI edits (the backend parser returns each rule's shell).
- **Canvas + TOML**: the graph editor and raw TOML, for power users.

The choice persists across sessions. Validation errors now carry line
numbers; clicking a `line N` chip scrolls the TOML editor to the failing
line.

## Canvas edits preserve your comments

Canvas, palette, and inspector edits are position-aware: the backend
patches the workflow TOML in place (via `toml_edit`), so hand-written
comments, blank lines, and formatting survive every edit — including
`remove_rule`/`connect`/`disconnect`/`add_rule` operations. Only the
edited rule's keys change; untouched rules keep their exact bytes.

## History

The left-rail History tab lists every saved revision (up to 50): load a
snapshot into the editor or roll the pipeline back — the current version
is preserved as a new revision first.

## The three panes

| Pane | What it does |
|------|--------------|
| **Tools / Assistant rail** (left) | Search the embedded Bioconda database (6,100+ tools) and add real tools as rules; or ask the AI assistant to draft a workflow |
| **Pipeline DAG** (center) | The node canvas: drag, connect, delete, double-click to edit |
| **Workflow TOML** (right) | The canonical `.oxoflow` text — always in sync with the canvas |

## Adding a rule from the tool palette

1. Open the **Tools** tab and search (e.g. `fastp`).
2. Click **+** on a result. A new node appears with the skeleton command
   (`<tool> {input} -o {output}` — a starting point to edit, not a real
   invocation) and the tool's real name and version in its description —
   never a stub.

## Editing a rule

Double-click a node to open the inspector:

- **Shell command** — free-form, with `{input}`/`{output}` placeholders.
- **Inputs / Outputs** — file path patterns; wildcards (`{sample}`) allowed.
- **Environment** — system / conda / mamba / docker / singularity / venv /
  modules, with the spec string (`pixi` is not offered in the dropdown — a
  rule that already uses it stays editable via the TOML pane, and other
  unknown backend keys surface as `conda` with the full spec preserved).
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
| Dashed | file-inferred | Edit the `input`/`output` paths — the engine infers the edge when an output pattern matches an input pattern |

The distinction matters: dashed edges are inferred from the
`input`/`output` paths — exact literal paths always connect, and wildcard
patterns connect too when they overlap the same files (for example
`aligned/{sample}.bam` feeding `dedup/{sample}.bam`). Patterns that can't
be resolved against each other leave the ordering ambiguous — use
`depends_on` to make it explicit.

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

## Look & feel

Dark mode (follows the OS or the header 🌙 toggle), Chinese interface
chrome (header 中文 toggle), and bilingual glossary tooltips on core
terms (pipeline, rule, wildcard, checkpoint, dry-run, depends_on).

## See also

- [DAG Edit API](../reference/dag-edit-api.md) — the command API behind the canvas
- [Workflow Format](../reference/workflow-format.md) — full rule field reference
- [DAG Engine](../reference/dag-engine.md) — how dependencies are inferred
