# oxo-flow Pipeline — VS Code extension

[![CI](https://github.com/Traitome/oxo-flow/actions/workflows/ci.yml/badge.svg)](https://github.com/Traitome/oxo-flow/actions/workflows/ci.yml)
[![Open VSX Version](https://img.shields.io/open-vsx/v/traitome/oxo-flow)](https://open-vsx.org/extension/traitome/oxo-flow)
[![VS Marketplace Version](https://img.shields.io/visual-studio-marketplace/v/traitome.oxo-flow)](https://marketplace.visualstudio.com/items?itemName=traitome.oxo-flow)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)

First-class [oxo-flow](https://traitome.github.io/oxo-flow/) support for
Visual Studio Code: language support for `.oxoflow` pipeline files, smart
schema-driven completion, background diagnostics, and one-click access to the
CLI lifecycle (run, dry-run, validate, lint, format, resume).

## Features

### Language support

- **Syntax highlighting** for `.oxoflow` files: full TOML grammar plus
  oxo-flow specifics — `[[rules]]` section emphasis, `{sample}`-style
  wildcards, and `{input[0]}` / `{output}` template placeholders inside
  shell commands.
- **Section folding**, bracket matching, comment toggling, and auto-closing.
- **Snippets** for pipeline skeletons, rules (basic, wildcard, conda, docker,
  when-gated), config knobs, sample/environment groups, and cluster settings.
- **Onboarding walkthrough** — a four-step *Get Started with oxo-flow*
  checklist on first launch (connect the CLI, open a pipeline, validate, run).

### Smart completion & hover docs

Completion data is **generated from the canonical `oxoflow-v1` JSON Schema**
(the same artifact `oxo-flow schema` exports), so it cannot drift from the
engine:

- top-level tables and their properties (`[workflow]`, `[ai]`,
  `[wildcard_constraints]`, …)
- all 51 `[[rules]]` properties with documentation on hover
- enum values where the schema constrains them (`checksum`, cluster
  `backend`, …)
- `depends_on` / `extends` complete **rule names defined in the current file**
- `env_group` completes `[env_groups.*]` groups from the current file
- environment backend keys (`conda`, `docker`, `singularity`, `modules`, …)
  inside `environment = { … }`

### Diagnostics & formatting

- Background diagnostics via `oxo-flow validate --json` and
  `oxo-flow lint --json`; findings are anchored to the failing rule's
  `name = "..."` line and deduplicated across the two commands.
- **Document formatting** through the engine's canonical TOML formatter
  (`oxo-flow format`), including unsaved buffers — with an opt-in
  `oxo-flow.formatOnSave` mode.
- **Run CodeLens** above every `[[rules]]` header: ▶ *Run this rule*
  (`-t <name>`) and ▶ *Run to here* (run everything up to this rule in
  file order).

### One-click lifecycle

- **Editor title buttons** (Run ▶ / Validate ✓) and a right-click context
  menu inside `.oxoflow` editors — Run, Dry Run, Validate, Lint, Format.
- Long-running operations show a progress notification; every error toast
  carries a **Report Issue** button that opens a pre-filled GitHub issue
  with sanitized diagnostics (versions, platform, remote, settings, and the
  output-channel tail — home paths are masked; pipeline content is never
  attached).

### CLI lifecycle

| Command | What it does |
| --- | --- |
| `oxo-flow: Run Pipeline` | Runs the active (or selected) pipeline as a task |
| `oxo-flow: Run Rule Targets…` | Runs explicit `-t` targets (also via CodeLens) |
| `oxo-flow: Dry Run (plan only)` | Plans the DAG without executing |
| `oxo-flow: Validate Pipeline` | Validates and shows the JSON report |
| `oxo-flow: Lint Pipeline` | Best-practice lint report |
| `oxo-flow: Format Document` | Canonical TOML formatting |
| `oxo-flow: Show DAG Graph` | Renders the rule graph (ascii, mermaid, dot, dot-clustered, tree, metro) |
| `oxo-flow: Resume from Checkpoint` | Picks a `.oxo-flow/checkpoint.json` and resumes |
| `oxo-flow: Show Run Status` | `oxo-flow status` over the latest checkpoint, with timings |
| `oxo-flow: Clean Outputs…` | Preview → confirm → delete rule outputs (or orphan chunks only) |
| `oxo-flow: Generate Pipeline with AI…` | `oxo-flow template "<desc>" --ai` |
| `oxo-flow: Show AI Provider Status` | `oxo-flow ai` status |
| `oxo-flow: Export JSON Schema` | Writes `oxo-flow-schema.json` for [Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml) |
| `oxo-flow: Report Issue / Suggest Improvement` | Pre-filled GitHub issue with sanitized diagnostics |

Keybindings (inside `.oxoflow` files; <kbd>⌘</kbd> on macOS,
<kbd>Ctrl</kbd> elsewhere): <kbd>Alt+R</kbd> run, <kbd>Alt+V</kbd> validate,
<kbd>Alt+L</kbd> lint, <kbd>Alt+G</kbd> graph — each combined with
<kbd>Cmd/Ctrl+Alt</kbd>.

The status bar shows the detected CLI version (or a warning when the binary
is missing); clicking it opens the command quick pick.

## Requirements

The [oxo-flow CLI](https://traitome.github.io/oxo-flow/) must be installed
for diagnostics, formatting, and the run commands. Install it with:

```bash
cargo install oxo-flow-cli
# or grab a release tarball from GitHub Releases
```

If the binary is not on your `PATH`, point **oxo-flow: Executable Path**
(`oxo-flow.executablePath`) at it. Without the CLI you still get syntax
highlighting, snippets, completion, and hover docs.

## Extension Settings

| Setting | Default | Description |
| --- | --- | --- |
| `oxo-flow.executablePath` | `oxo-flow` | Path to the CLI binary |
| `oxo-flow.diagnosticMode` | `save` | `off`, `save`, or `type` (debounced) |
| `oxo-flow.enableLintDiagnostics` | `true` | Merge `lint` findings with `validate` |
| `oxo-flow.runArgs` | `[]` | Extra arguments for Run Pipeline |
| `oxo-flow.formatOnSave` | `false` | Format `.oxoflow` files on save |
| `oxo-flow.autoOpenGraph` | `false` | Open the DAG graph after a successful run |
| `oxo-flow.trace` | `false` | Log CLI invocations to the output channel |

## Tasks

Define an `oxo-flow` task in `tasks.json`:

```json
{
  "type": "oxo-flow",
  "workflow": "pipeline.oxoflow",
  "jobs": 8,
  "keepGoing": true,
  "target": "results/all"
}
```

## Source & license

Part of the [Traitome/oxo-flow](https://github.com/Traitome/oxo-flow)
repository (this extension lives in `editors/vscode/`). Licensed under
Apache-2.0, see [LICENSE](LICENSE).
