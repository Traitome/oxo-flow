# `oxo-flow agent-docs`

Scaffold agent-facing docs into a project so an AI agent can operate it
safely: an editable `AGENTS.md` entry file, a one-line `CLAUDE.md` import,
and the full canonical `SKILL.md` operating manual.

---

## Usage

```
oxo-flow agent-docs [OPTIONS]
```

Works in any project directory — new or existing. `oxo-flow init --agent`
runs the same scaffold while creating a project.

---

## Options

| Option | Short | Default | Description |
|---|---|---|---|
| `--dir` | `-d` | `.` | Target directory |
| `--force` | — | off | Overwrite existing agent docs (user edits are lost) |
| `--verbose` | `-v` | — | Enable verbose (debug-level) logging |
| `--quiet` | — | — | Suppress non-essential output (errors only) |

---

## Generated files

| File | Purpose |
|---|---|
| `AGENTS.md` | **The entry point an agent reads first.** Contains the non-negotiable data-safety summary, a relative link to `SKILL.md`, and an editable "Project notes" section. This file belongs to the project owner — it is never overwritten without `--force`. |
| `CLAUDE.md` | One line: `@AGENTS.md`. Claude Code imports the entry file, so user edits to `AGENTS.md` flow through automatically. |
| `SKILL.md` | The full [AI Agent Skill](../SKILL.md) operating manual, embedded in the CLI binary and written verbatim — seven-stage analysis loop, static gates, execution, monitoring, QC, delivery, and the binding data-safety contract. |

`AGENTS.md` is the open convention read by most agent tools (Codex, Cursor,
Jules, Claude Code); other tools can be pointed at it or at `SKILL.md`
directly.

---

## Examples

### Add agent docs to an existing project

```bash
cd my-pipeline
oxo-flow agent-docs
```

### Refresh after an oxo-flow upgrade

The generated files carry the version that wrote them; after upgrading
oxo-flow, refresh the skeleton (this **overwrites** your `AGENTS.md` edits —
copy them out first, or keep project specifics in the "Project notes"
section and merge them back):

```bash
oxo-flow agent-docs --force
```

### Create a new project with agent docs

```bash
oxo-flow init my-pipeline --agent
```

---

## How agents use the files

1. `AGENTS.md` gives the entry context: safety rules inline, project notes,
   and the pointer to `SKILL.md`.
2. `SKILL.md` is the canonical manual: read it in full once per task, follow
   its links on demand.
3. The data-safety contract (inputs read-only, snapshot-before-mutate,
   scratch confined to `.oxo-flow/agent/`, operation journal) binds every
   command the agent runs — see
   [Agent Safety & Workspace Discipline](../how-to/agent-safety.md).

---

## Notes

- `SKILL.md` is embedded at build time from the docs canonical
  (`docs/guide/src/SKILL.md`); `make skill-drift` (a `make ci` gate) fails
  when the two diverge. `make sync-skill` refreshes the embedded copy.
- Existing files are kept unless `--force` is passed; `oxo-flow init --agent`
  never overwrites agent docs, even with `--force`.
