# Changelog

All notable changes to the oxo-flow VS Code extension are documented here.
The extension version is kept in lockstep with the oxo-flow project version
(see the project-level `CHANGELOG.md` for engine changes).

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
