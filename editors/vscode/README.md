# oxo-flow Pipeline — VS Code extension

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
  (`oxo-flow format`), including unsaved buffers.

### CLI lifecycle

| Command | What it does |
| --- | --- |
| `oxo-flow: Run Pipeline` | Runs the active (or selected) pipeline as a task |
| `oxo-flow: Dry Run (plan only)` | Plans the DAG without executing |
| `oxo-flow: Validate Pipeline` | Validates and shows the JSON report |
| `oxo-flow: Lint Pipeline` | Best-practice lint report |
| `oxo-flow: Format Document` | Canonical TOML formatting |
| `oxo-flow: Show DAG Graph` | Renders the rule graph |
| `oxo-flow: Resume from Checkpoint` | Picks a `.oxo-flow/checkpoint.json` and resumes |
| `oxo-flow: Generate Pipeline with AI…` | `oxo-flow template "<desc>" --ai` |
| `oxo-flow: Show AI Provider Status` | `oxo-flow ai` status |
| `oxo-flow: Export JSON Schema` | Writes `oxo-flow-schema.json` for [Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml) |

Keybindings (inside `.oxoflow` files): <kbd>Ctrl+Alt+R</kbd> run,
<kbd>Ctrl+Alt+V</kbd> validate.

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
