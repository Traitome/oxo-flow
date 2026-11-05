# Changelog

All notable changes to the oxo-flow VS Code extension are documented here.
The extension version is kept in lockstep with the oxo-flow project version
(see the project-level `CHANGELOG.md` for engine changes).

## 0.28.0

- Refactor wave: shared `pipelineSlug` / `parseCliVersion` helpers, one
  output channel (IDE generation no longer forks its own), folder
  resolution delegated to `resolveFolder`, task definitions keyed by the
  `TASK_TYPE` constant, and dead-code removal in the schema generator
  and completion data.
- Snippet fixes: `sample_groups` emitted as a sequence, env groups carry
  the environment keys directly, config knobs use `help`.
- Lint/validate parse failures surface a diagnostic instead of silence;
  AI repair rounds that yield no TOML block warn.
- Task and status args use folder-relative paths.

## 0.27.0

- Structural refactors merged with the engine release; no
  extension-specific user-visible features.

## 0.23.0 – 0.26.0

- **AI generation can use the editor language model** — new
  `oxo-flow.ai.backend` setting (`auto` | `cli` | `ide`); `ide` runs
  generation through VS Code's language-model API instead of the CLI
  binary (#820).
- Publishing reliability, first-run UX, and Report Issue fixes (#817);
  the Report Issue URL survives `openExternal` verbatim (#822).

## 0.22.0

- **Get Started walkthrough** — four-step onboarding (welcome → connect
  the CLI → open/create a pipeline → validate, then run) with checklist
  steps that complete as you use the corresponding commands.
- **Run CodeLens** on `[[rules]]` headers — ▶ Run this rule (`-t <name>`,
  rule + upstream) and ▶ Run to here (repeated `-t` over file-order rules),
  plus a `Run Rule Targets…` palette command.
- **Format on save** — new `oxo-flow.formatOnSave` setting (default off);
  when enabled, saves of `.oxoflow` documents run `oxo-flow format` first.
- **Graph format quick pick** — *Show DAG Graph* now offers all
  `oxo-flow graph -f` formats (ascii, mermaid, dot, dot-clustered, tree,
  metro) and remembers the last choice; new `oxo-flow.autoOpenGraph`
  setting (default off) opens the graph after a successful run.
- Editor title-bar **Run** and **Validate** buttons for `.oxoflow`
  editors.
- Editor context-menu group (Run / Dry Run / Validate / Lint / Format).
- Schema-driven completion and hover docs regenerated from
  `oxoflow-v1.schema.json` (17 completion contexts).

## 0.21.2

Initial release.

- `.oxoflow` language: syntax highlighting (TOML-based with oxo-flow
  wildcards and `{input}`/`{output}` template placeholders), bracket
  matching, section folding, and snippets.
- Smart completion driven by the canonical `oxoflow-v1` JSON Schema:
  table/section keys, rule properties, enum values, `depends_on`/`extends`
  rule references, and environment backend keys.
- Hover documentation for workflow and rule properties.
- Background diagnostics via `oxo-flow validate` / `oxo-flow lint --json`.
- Document formatting via `oxo-flow format`.
- One-click CLI lifecycle: run, dry-run, validate, lint, DAG graph,
  resume-from-checkpoint, AI pipeline generation, AI status.
- `oxo-flow` task type (`tasks.json`) with jobs/keep-going/target options.
- Status bar indicator with CLI version and command quick pick.
