# VS Code Extension

oxo-flow ships a first-party VS Code extension (publisher
[`traitome`](https://marketplace.visualstudio.com/publishers/traitome),
extension id `traitome.oxo-flow`) that turns VS Code into a full editor for
`.oxoflow` pipelines: smart syntax, schema-driven completion, background
diagnostics, canonical formatting, and one-click access to the CLI lifecycle.

The extension lives in [`editors/vscode/`](https://github.com/Traitome/oxo-flow/tree/main/editors/vscode)
and is released in lockstep with the engine: every oxo-flow release attaches
the same version as a VSIX (with a `.sha256` checksum) to the GitHub release
and publishes it to [Open VSX](https://open-vsx.org/).

!!! note "About the VS Code Marketplace"

    Publishing to the VS Code Marketplace is **not currently supported** —
    it requires a publisher account tied to Azure DevOps that the project
    does not maintain yet. Two good alternatives cover every VS Code fork:

    1. **Open VSX** — VSCodium, Cursor, Windsurf and most forks search it by
       default; the same `traitome.oxo-flow` id is there.
    2. **Offline VSIX** — download the release artifact and install it with
       one command (below); this works in stock VS Code and any fork.

## Install

=== "Open VSX (recommended for forks)"

    VSCodium / Cursor / Windsurf and most VS Code forks search
    [Open VSX](https://open-vsx.org/extension/traitome/oxo-flow) by default —
    install the extension from the Extensions view like any other, or:

    ```bash
    codium --install-extension traitome.oxo-flow
    ```

=== "Offline VSIX (works in any VS Code)"

    Download `oxo-flow-vscode-v<version>.vsix` (with a `.sha256` checksum)
    from the release page and install it directly:

    ```bash
    code --install-extension oxo-flow-vscode-v0.22.0.vsix
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

## Run CodeLens

Every `[[rules]]` header shows two CodeLens buttons when you open a
`.oxoflow` file:

- **▶ Run this rule** — runs `oxo-flow run -t <name>`, which the CLI resolves
  to that rule plus its upstream closure.
- **▶ Run to here** — repeats `-t` for every rule at or before this one in
  file order, so the pipeline runs up to (and including) this rule.

Both honor `oxo-flow.runArgs` and reuse the cached run task. Editors who
prefer the keyboard can pass the same targets by hand:
`Run Pipeline` accepts `target` in its task definition, or list several
rules via repeated `-t` in `extraArgs`.

## Onboarding walkthrough

First launch surfaces a **Get Started with oxo-flow** walkthrough
(`Help → Welcome` → *Walkthroughs*, or "Walkthroughs…" from the
command palette): four checklist steps that light up as you go —

1. **Welcome to oxo-flow** — what a `.oxoflow` pipeline is and what the
   editor adds.
2. **Connect the extension to the CLI** — install the binary or set
   `oxo-flow.executablePath`; completes when the setting changes or
   settings open.
3. **Open or create a pipeline** — open a `.oxoflow`, scaffold with
   `oxo-flow init`, or generate with AI.
4. **Validate, then run** — the static-gates-first habit; completes on
   the first Validate or Run.

## CLI lifecycle commands

All commands are prefixed `oxo-flow:` in the command palette; the status bar
item shows the detected CLI version and opens a quick pick of all of them.
`.oxoflow` editors also get **Run** and **Validate** buttons in the editor
title bar.

| Command | CLI equivalent |
| --- | --- |
| Run Pipeline (<kbd>Ctrl+Alt+R</kbd>) | `oxo-flow run <file>` + `oxo-flow.runArgs` |
| Run Rule Targets… (CodeLens) | `oxo-flow run <file> -t <name>…` (▶ Run this rule / ▶ Run to here above each `[[rules]]`) |
| Dry Run (plan only) | `oxo-flow dry-run <file>` |
| Validate Pipeline (<kbd>Ctrl+Alt+V</kbd>) | `oxo-flow validate <file> --json` |
| Lint Pipeline | `oxo-flow lint <file> --json` |
| Format Document | `oxo-flow format <file>` |
| Show DAG Graph | `oxo-flow graph <file>` |
| Resume from Checkpoint | `oxo-flow resume <checkpoint>` (quick pick over `**/.oxo-flow/checkpoint.json`) |
| Show Run Status | `oxo-flow status <checkpoint> --timing` (quick pick over checkpoint files) |
| Clean Outputs… | `oxo-flow clean <file> -n` preview → confirmation → `--force` (optionally `--orphans`) |
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
