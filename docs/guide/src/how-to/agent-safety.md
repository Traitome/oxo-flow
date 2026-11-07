# Agent Safety & Workspace Discipline

AI agents increasingly operate oxo-flow directly: they probe inputs, author
workflows, run them, and repair failures. That makes the agent the operator of
someone else's data. This page is the operating contract — the same rules ship
in the [AI Agent Skill](../SKILL.md) so canonical agents load them
automatically.

**The engine's own gates protect rule shells, not your terminal.** `E011`
blocks recursive deletions outside the run workdir at run time;
`[engine] protected_data_trees` detects drift of declared data trees before
the next run. Neither one prevents an agent's own `rm`, `mv`, or truncating
redirect. These rules do.

## The seven rules

1. **Inputs are read-only.** Never delete, move, or overwrite an input without
   explicit human authorization (see [Authorization](#authorization-required)).
2. **Snapshot before mutating.** `cp -a` the target into your scratch tree and
   record checksums first. Write new files, rename on success.
3. **Never truncate in place.** Use `>>` to append; a `>` redirect into an
   existing path you did not create this task is a destructive write.
4. **Probe inside one scratch tree in the project.** No `$HOME` writes, no
   global `/tmp` scatter, nothing outside the working directory.
5. **Journal every mutating operation** to `.oxo-flow/agent/operations.log`.
6. **Protect input trees** with `[engine] protected_data_trees` in every
   workflow that consumes vendor data.
7. **Inspect without mutating.** Prefer `oxo-flow status`, `oxo-flow report`,
   `oxo-flow provenance verify`, and `dry-run` over ad-hoc shell.

## Workspace layout

Keep everything the agent creates inside the project. One scratch tree per
task, plus an append-only journal:

```
<project>/
├── raw/                    # inputs — READ-ONLY
├── reference/              # inputs — READ-ONLY
├── <project>.oxoflow
└── .oxo-flow/
    ├── checkpoint.json     # engine state
    ├── agent/
    │   ├── operations.log  # append-only JSONL journal
    │   └── 2026-11-08-align/   # one scratch tree per task
    │       ├── tmp/        # TMPDIR for probes and tools
    │       ├── snapshots/  # cp -a copies of anything you mutate
    │       └── probes/     # throwaway experiments
    └── ai_sessions/        # AI session archives (engine-managed)
```

Add `.oxo-flow/agent/` to the project's `.gitignore` (`.oxo-flow/` is often
already ignored). Delete only scratch trees you created, and only when the
task is done.

## Snapshot, mutate, verify

Before changing anything that is not your own scratch output:

```bash
SCRATCH=".oxo-flow/agent/$(date +%F)-<task>"
mkdir -p "$SCRATCH"/{tmp,snapshots,probes}

# 1. Snapshot + record the pre-state checksum
cp -a reference/panel.bed "$SCRATCH/snapshots/panel.bed.pre"
sha256sum reference/panel.bed >> "$SCRATCH/snapshots/checksums.pre"

# 2. Work on a COPY, never in place
cp -a reference/panel.bed "$SCRATCH/panel.bed.new"
sed -i 's/hg19/hg38/g' "$SCRATCH/panel.bed.new"

# 3. Rename only after the human-approved review, then verify
cmp -s "$SCRATCH/panel.bed.new" reference/panel.bed || true   # expected: differs
sha256sum "$SCRATCH/panel.bed.new"
```

The same discipline applies to commands: `mv` over an input, `dd of=<input>`,
`truncate -s 0 <input>`, `find <input-tree> -delete`, and
`sed -i <input>` are all destructive writes that require authorization.

## Probe isolation

Point every tool that writes scratch data at your task scratch tree. The
engine exports these variables into rule shells and hooks too — for probes
you run yourself, export them in your shell:

```bash
export OXO_FLOW_TMPDIR="$PWD/$SCRATCH/tmp"   # per-invocation override (#907)
export TMPDIR="$OXO_FLOW_TMPDIR" TMP="$OXO_FLOW_TMPDIR" TEMP="$OXO_FLOW_TMPDIR"
```

For workflows, prefer the config key so every future run inherits it:

```toml
[engine]
tmp_dir = ".oxo-flow/agent/tmp"
```

## The operation journal

Append one JSON line per **mutating** command — the audit trail that makes an
agent's session reviewable after the fact:

```bash
journal() {
  local exit_code=$?
  printf '{"ts":"%s","cwd":"%s","cmd":"%s","exit":%d,"note":"%s"}\n' \
    "$(date -u +%FT%TZ)" "$PWD" "$1" "$exit_code" "${2:-}" \
    >> .oxo-flow/agent/operations.log
}

journal_do() { local note=$2; "$@"; local rc=$?; journal "$*" "$note"; return $rc; }
```

Read-only inspections (`ls`, `cat`, `grep`, `oxo-flow status`) do not need
journaling. A journal with missing mutating operations is worse than none —
record first, then run.

## Protect input trees

Opt in per workflow; the engine records a baseline and refuses to run when
the tree drifts:

```toml
[engine]
protected_data_trees = ["raw/", "reference/"]
```

Details: [Workflow Format — `[engine]`](../reference/workflow-format.md#engine-runtime-disk-pressure-guard).
This is **detection, not prevention** — it catches out-of-band changes at the
next plan/dry-run/run, it does not stop a deletion as it happens. Treat it as
a backstop behind rules 1–3, never as permission to touch inputs.

## Authorization required

These actions cross the line. Ask the human first — quote the exact command,
name the target, give the reason, and wait for an explicit "yes":

| Action | Examples |
|---|---|
| Deleting data | `rm`, `rmtree`, `find -delete`, emptying a directory |
| Moving/renaming inputs | `mv raw/ raw_old/`, renaming a reference |
| Truncating or rewriting | `> input.fq`, `truncate`, `dd of=`, `sed -i` |
| Git history/data changes | `git checkout --`, `git clean -fdx`, `git reset --hard` |
| Relaxing an engine gate | `--allow-protected-drift`, `--rerun` on shared outputs |
| Writes outside the project | any path outside the working directory |

If no human is reachable: stop, write down the intended action and why it
seemed necessary, and leave it for review. Never guess.

## Cleanup

- Remove only scratch trees you created this task, after the delivery stage
  has been signed off.
- Never "clean up" inputs, outputs you did not create, or the `.oxo-flow/`
  engine state (`checkpoint.json`, `ai_sessions/`).
- Prefer `oxo-flow clean --dry-run` over manual deletion of run outputs.
