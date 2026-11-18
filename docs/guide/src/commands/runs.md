# `oxo-flow runs`

Manage the per-run workspace trees that web create-run materializes.

---

## Usage

```
oxo-flow runs <SUBCOMMAND>
```

---

## Subcommands

### `runs gc`

Reclaim **stale per-run workspace trees** left behind by finished web runs.

Every web run (created through `POST /api/runs`) gets its own immutable tree
under the web workspace:

```
<OXO_FLOW_WORKSPACE>/users/<user_id>/runs/<run_id>/
```

The tree holds the workflow snapshot (`workflow.oxoflow`), the staged
inputs/includes, the checkpoint, logs, and outputs. Nothing reclaims that
tree when its run finishes, so a long-lived server accumulates one directory
per run forever. `runs gc` sweeps the stale ones.

```bash
oxo-flow runs gc [--older-than DAYS] [--user NAME] [--dry-run] [--json]
```

| Option | Default | Description |
|---|---|---|
| `--older-than` | `90` | Only reclaim trees whose run finished at least this many days ago |
| `--user` | (all users) | Restrict the sweep to one user, by username |
| `--dry-run` | off | Report what would be reclaimed without deleting anything |
| `--json` | off | Emit the full report as a JSON document on stdout |

**Safety guards.** A tree is only ever deleted when ALL of the following hold;
each guard is independently tested:

- **Terminal run only** — the run must be `completed`, `failed`, or `cancelled`. A live run (`queued`, `running`, `paused`) is never touched.
- **Not a lineage target** — retry and resume create a *new* run row that copies the predecessor's `workdir`. A `workdir` shared by two or more rows still identifies the tree a later run depends on, so any such tree is spared.
- **Age gate** — the run must have finished (or, absent a finish time, been created) at least `--older-than` days ago. An unparseable timestamp is treated as too young and the tree is kept.
- **Protected output / checkpoint reference** — if the tree carries a declared protected output (or its workflow snapshot cannot be parsed, which is treated fail-closed as protected), the tree is kept.
- **Runs-root confinement** — deletion only ever targets a path whose parent directory is literally named `runs`, with the run id and user id traversal-validated first. Anything else is skipped.

**What `runs gc` deletes:**

- Only whole `<...>/runs/<run_id>/` trees that pass every guard above
- A single tree that fails to delete increments the failure count and, outside `--dry-run`, makes the command exit non-zero after reporting the rest

```bash
# Preview
oxo-flow runs gc --dry-run

Run trees: 12 run(s) scanned
  · /srv/oxo-flow/workspace/users/9f1e.../runs/f0c2... would be deleted

Summary: 1 run tree(s) would be deleted, 42.3 MiB freed
  11 run tree(s) kept

# Sweep, restricted to one user
oxo-flow runs gc --user alice --older-than 30
```

**Notes:**

- The age gate is measured from the run's `finished_at` (falling back to `created_at`), so a run that finished long ago is eligible even if the tree's files were touched recently
- The lineage guard keys on a shared `workdir` string across run rows — the mechanism retry/resume use to point a new run at an existing tree — so a run that is itself old is still spared while a newer run depends on its tree
- `--json` emits `{dry_run, older_than_days, user, scanned, reclaimed_count, skipped_count, failed_count, bytes_freed, reclaimed, skipped}`, with per-run details under `reclaimed` and `skipped`
- `runs gc` [engine] configuration is intentionally absent in this first version: the settings are CLI flags rather than workflow keys, so the command can run against any workspace without first opening a workflow file

---

## Options

| Option | Short | Description |
|---|---|---|
| `--verbose` | `-v` | Enable debug-level logging |

---

## Examples

```bash
# Preview everything eligible under the default 90-day gate
oxo-flow runs gc --dry-run

# Reclaim one user's stale trees older than 30 days
oxo-flow runs gc --user alice --older-than 30

# Machine-readable report
oxo-flow runs gc --json
```

---

## Notes

- The command reads `DATABASE_URL` (default `sqlite://oxo-flow.db`) to enumerate runs, and `OXO_FLOW_WORKSPACE` (default `workspace`) to locate the trees — the same sources the web server uses, so point both at the server's values
- Nothing reclaims run trees automatically yet: `runs gc` is the only sweeper, and it is a manual, auditable operation
