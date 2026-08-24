# Concurrent Runs

This guide explains how oxo-flow behaves when the same workflow is run more
than once — at the same time, or one after another — and the pattern for
sharing one workflow across many analyses.

---

## The guarantee

A single workflow file is safe to reuse in three ways at once:

- **Many workdirs, concurrently** — any number of `oxo-flow run` invocations
  can execute the same workflow file at the same time, each in its own
  workdir (`-d`). Every run gets its own checkpoint, logs, chunks, and
  outputs; the workflow file itself is only ever read.
- **Many users, concurrently** — runs under different `HOME`s are fully
  isolated (config, per-user state), and the one shared piece of state
  outside the workdir — the module cache used by git-pinned
  `[[include]]`s — is clone-locked, so two processes fetching the same
  module at the same time are serialized instead of racing (one clones,
  the other waits and reuses).
- **One workdir, exclusively** — two runs pointed at the *same* workdir
  are refused: the second fails fast with
  `Error: workdir is locked by another oxo-flow process`, and the lock
  releases automatically when the first run exits (even if it crashes).

Re-running the same workflow later from a new workdir starts from a fresh
checkpoint; nothing written by an earlier run leaks into the new one.

---

## What is shared vs per-workdir

| | Shared across concurrent runs | Per workdir (`-d` / workdir) |
|---|---|---|
| Workflow file (`.oxoflow`) | Read-only — never written | — |
| Git-pinned `[[include]]` module cache (`~/.cache/oxo-flow/modules`, or `$OXO_FLOW_MODULE_CACHE`) | Clone-locked, safe to share | — |
| Profiles (`profiles/*.toml` next to the workflow) | Read-only, shared | — |
| Checkpoint (`.oxo-flow/checkpoint.json`) | — | Own checkpoint per workdir |
| Logs (`.oxo-flow/logs/`) | — | Own logs per workdir |
| Transform chunks (`.oxo-flow/chunks/`) | — | Own chunks per workdir |
| Outputs / intermediate files | — | Written in the workdir |
| Environment setups (`envs_dir`) | — | Resolved per run |

The workdir lock lives at `.oxo-flow/lock` inside each workdir — that is
why different workdirs never contend, and why the same workdir refuses a
second concurrent run before any rule starts.

---

## The recommended pattern

Keep the workflow in one read-only location and run each analysis in its
own workdir:

```bash
# One shared workflow file, e.g. in a central repo directory
ls /share/pipelines/rnaseq.oxoflow

# Each analysis gets its own workdir — these run concurrently and safely
oxo-flow run /share/pipelines/rnaseq.oxoflow -d analyses/sample-A
oxo-flow run /share/pipelines/rnaseq.oxoflow -d analyses/sample-B
```

The workflow directory only needs to be readable — a `chmod 555` central
directory (or a read-only mount) works fine; the engine never writes next
to the workflow file. Per-analysis `-d` directories hold everything that
varies: checkpoints, logs, chunks, and outputs.

---

## Troubleshooting

**`Error: workdir is locked by another oxo-flow process: <path>`**

Another run is active in that workdir (or a previous one crashed mid-run —
the lock releases automatically, so this is transient). Wait for it to
finish, or point the new run at a fresh workdir with `-d`.
