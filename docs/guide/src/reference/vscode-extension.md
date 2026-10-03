# VS Code Extension

oxo-flow ships a first-party VS Code extension (publisher
[`traitome`](https://marketplace.visualstudio.com/publishers/traitome),
extension id `traitome.oxo-flow`) that turns VS Code into a full editor for
`.oxoflow` pipelines: smart syntax, schema-driven completion, background
diagnostics, canonical formatting, and one-click access to the CLI lifecycle.

The extension lives in [`editors/vscode/`](https://github.com/Traitome/oxo-flow/tree/main/editors/vscode)
and is released in lockstep with the engine: every oxo-flow release publishes
the same version as a VSIX to the GitHub release, [Open VSX](https://open-vsx.org/)
and the VS Code Marketplace.

## Install

=== "Marketplace / Open VSX"

    Search for **oxo-flow Pipeline** in the VS Code Extensions view, or:

    ```bash
    code --install-extension traitome.oxo-flow
    ```

    On VSCodium / Cursor / Windsurf, install from
    [Open VSX](https://open-vsx.org/extension/traitome/oxo-flow) — most
    VS Code forks search it by default.

=== "Offline VSIX"

    Download `oxo-flow-vscode-v<version>.vsix` (with a `.sha256` checksum)
    from the release page and install it directly:

    ```bash
    code --install-extension oxo-flow-vscode-v0.21.2.vsix
    ```

## Language support

`.oxoflow` files get a dedicated language (`oxoflow`), not a TOML alias:

- **Syntax highlighting** — full TOML grammar plus oxo-flow specifics:
  `[[rules]]` headers, `{sample}`-style wildcards, and `{input[0]}` /
  `{output}` template placeholders inside shell commands; invalid string
  escapes are flagged.
- **Folding** — each `[table]` / `[[rules]]` section folds independently.
- **Snippets** — type `pipeline`, `rule`, `wildcard-rule`, `conda-rule`,
  `when-rule`, `config`, `cluster-slurm`, … (see the snippet suggestions).

## Smart completion and hover docs

Completion data is **generated from the canonical `oxoflow-v1` JSON Schema**
(`docs/schema/oxoflow-v1.schema.json`, the same artifact `oxo-flow schema`
exports) at build time, so the editor can never drift from the engine. It
covers:

- top-level tables and their properties (`[workflow]`, `[ai]`,
  `[wildcard_constraints]`, `[cluster]`, …)
- all 51 `[[rules]]` properties, with hover documentation for each
- enum values where the schema constrains them (rule `checksum`, cluster
  `backend`, config `type`, …)
- `depends_on` / `extends` complete **rule names defined in the current
  file**
- `env_group` completes `[env_groups.*]` groups from the current file
- environment backend keys (`conda`, `docker`, `singularity`, `modules`,
  `conda_prefix`, …) inside `environment = { … }`

## Diagnostics

When the `oxo-flow` CLI is available, the extension runs `validate --json`
(and `lint --json`, optional) in the background and surfaces the findings as
Problems-panel diagnostics:

- findings are **anchored to the failing rule's `name = "..."` line** when
  the CLI reports a rule
- validate errors, lint warnings/best-practice hints, and missing-input
  warnings are deduplicated across the two commands
- trigger on save by default (`oxo-flow.diagnosticMode: save`); switch to
  `type` for debounced live diagnostics or `off` to disable

## Formatting

`oxo-flow: Format Document` (and the standard *Format Document* action) runs
the engine's canonical TOML formatter (`oxo-flow format`) on the current
buffer — unsaved changes included.

## CLI lifecycle commands

All commands are prefixed `oxo-flow:` in the command palette; the status bar
item shows the detected CLI version and opens a quick pick of all of them.

| Command | CLI equivalent |
| --- | --- |
| Run Pipeline (<kbd>Ctrl+Alt+R</kbd>) | `oxo-flow run <file>` + `oxo-flow.runArgs` |
| Dry Run (plan only) | `oxo-flow dry-run <file>` |
| Validate Pipeline (<kbd>Ctrl+Alt+V</kbd>) | `oxo-flow validate <file> --json` |
| Lint Pipeline | `oxo-flow lint <file> --json` |
| Format Document | `oxo-flow format <file>` |
| Show DAG Graph | `oxo-flow graph <file>` |
| Resume from Checkpoint | `oxo-flow resume <checkpoint>` (quick pick over `**/.oxo-flow/checkpoint.json`) |
| Generate Pipeline with AI… | `oxo-flow template "<description>" --ai -o <file>` |
| Show AI Provider Status | `oxo-flow ai` |
| Export JSON Schema | `oxo-flow schema > oxo-flow-schema.json` |

Run / Dry Run / Graph execute as **tasks** (`type: "oxo-flow"`), so they get
a dedicated terminal panel, re-run support, and `tasks.json` customization:

```json
{
  "type": "oxo-flow",
  "workflow": "pipeline.oxoflow",
  "kind": "run",
  "jobs": 8,
  "keepGoing": true,
  "target": "results/all",
  "extraArgs": ["--profile", "slurm"]
}
```

`kind` selects the lifecycle: `run` (default), `dry-run`, `graph`,
`resume`, or `generate`. For `resume`, `workflow` holds the checkpoint path
(e.g. `.oxo-flow/checkpoint.json`); for `generate`, the task runs
`oxo-flow template "<description>" --ai -o <file>` with `extraArgs[0]` as
the description.

## Settings

| Setting | Default | Description |
| --- | --- | --- |
| `oxo-flow.executablePath` | `oxo-flow` | Path to the CLI binary (machine-overridable, e.g. for SSH remotes) |
| `oxo-flow.diagnosticMode` | `save` | `off`, `save`, or `type` |
| `oxo-flow.enableLintDiagnostics` | `true` | Merge `lint` findings into the Problems panel |
| `oxo-flow.runArgs` | `[]` | Extra arguments for Run Pipeline |
| `oxo-flow.trace` | `false` | Log CLI invocations to the *oxo-flow* output channel |

## Remote and untrusted workspaces

The extension declares `extensionKind: workspace`, so in SSH / dev-container
sessions it runs **on the remote** where the pipelines and the CLI live. It
refuses to activate in untrusted workspaces because it shells out to the
local binary.

## See also

- [Editor Setup](../how-to/editor-setup.md) — other editors (Zed, Helix,
  Neovim, JetBrains, …) and Even Better TOML schema association
- [Workflow Format](workflow-format.md) — the full `.oxoflow` reference
