# `oxo-flow env`

Manage software environments. Provides subcommands for listing available backends, checking workflow environment requirements, and maintaining the shared environment store.

---

## Usage

```
oxo-flow env <SUBCOMMAND>
```

---

## Subcommands

### `env list`

List available environment backends, or list environments defined in a workflow.

```bash
oxo-flow env list [WORKFLOW]
```

When called without arguments, lists all backends detected on the system.
When given a workflow file, lists the environments used by each rule.

**Output:**

```
oxo-flow v0.24.1 — Rust-native bioinformatics pipeline engine
https://github.com/Traitome/oxo-flow
Available environment backends:
  ✓ conda
  ✓ docker
  ✓ singularity
  ✓ venv
```

The two banner lines print only at an interactive terminal. All of this output goes to **stderr** — capture it with `2>` or `2>&1`; stdout stays empty.

### `env check`

Check that all environments declared in a workflow file are valid and their backends are available.

```bash
oxo-flow env check [WORKFLOW]
```

| Argument | Description |
|---|---|
| `[WORKFLOW]` | Optional path to the `.oxoflow` workflow file. If omitted, checks system-wide backend availability instead. |

**Output (all valid):**

```
  ✓ align (conda)
  ✓ call_variants (docker)
  ✓ annotate (singularity)
```

Without a workflow, each available backend also names the binary that a run
would resolve — the engine prefers `mamba` → `micromamba` → `conda`, so on a
machine with several conda-family installs the resolved binary is what
actually gets invoked:

```
Environment backend availability:
  ✓ mamba (resolved: micromamba)
  ✓ conda
  ✗ docker (not found)
```

### `env create`

Create a new environment from a specification file.

```bash
oxo-flow env create [OPTIONS] <SPEC>
```

| Argument | Description |
|---|---|
| `<SPEC>` | Path to the environment specification file (`.yaml`/`.yml`, `.toml`, `.lock`) |

| Option | Short | Description |
|---|---|---|
| `--name` | `-n` | Custom name for the created environment (default: derived from the spec filename) |
| `--ai` | — | Generate the environment spec from a natural-language description (SPEC is the description) |
| `--backend` | — | Environment backend to generate: conda (YAML) or pixi (TOML). Default: inferred from the spec's extension, or conda with `--ai` |

**Output (missing backend):**

```
  ✓ align (conda)
  ✗ call_variants — environment error (docker): docker is not installed or not in PATH
```

### `env gc`

Inspect and clean up the shared environment store (issue #904).

Pixi and venv environments are materialized **once per content key** under a
machine-wide store directory instead of inside each workflow tree; workflow
trees hold symlinks into the store. Repeated runs across many pipeline
checkouts reuse the same environment instead of re-downloading and
re-building it per tree. A broken link (e.g. the store dir was deleted) is
detected automatically on the next run: the cached entry is invalidated and
the environment is rebuilt, no manual intervention needed.

```bash
oxo-flow env gc [--min-age-days N] [--dry-run]
```

| Option | Default | Description |
|---|---|---|
| `--min-age-days` | `14` | Minimum age (mtime) before an unreferenced store dir may be deleted |
| `--dry-run` | off | Report what would be deleted without touching anything |

**Where environments live:**

- Store root: `$OXO_FLOW_ENV_STORE`, default `~/.cache/oxo-flow/envs/`
- Each environment: `<root>/<backend>-<sha256-16hex of its content key>` (e.g. `pixi-3f2a…`)
- Workflow trees: a symlink (`.pixi` for pixi rules, the venv path for venv rules) pointing into the store

**What `env gc` deletes:**

- Only store dirs **not referenced** by any discovered run checkpoint (`env_store_refs` in `.oxo-flow/checkpoint.json`), AND older than `--min-age-days`
- Referenced dirs are always kept — a running or resumable run keeps its environments (the #893 lesson)
- Young dirs are kept even when torn/incomplete: a fresh torn dir may be a concurrent run mid-setup, so the age gate protects those too
- Deleting a still-needed env is recoverable: the next run detects the broken link and rebuilds

```bash
# Preview
oxo-flow env gc --dry-run

Shared environment store: /home/user/.cache/oxo-flow/envs (3 env dirs)
  pixi-3f2a91b0c4d5e6f7 [pixi] referenced by a checkpoint — kept
      1.2 GiB /home/user/.cache/oxo-flow/envs/pixi-3f2a91b0c4d5e6f7
  venv-8b7c6d5e4f3a2b1c [venv] unreferenced and old enough — eligible
      310.4 MiB /home/user/.cache/oxo-flow/envs/venv-8b7c6d5e4f3a2b1c

Summary: 1 store dir(s) would be deleted, freeing ~310.4 MiB (dry run — nothing touched)
```

**Notes:**

- GC discovery covers the checkpoint of the workdir you run it from (`.oxo-flow/checkpoint.json`); runs in other trees are protected by the age gate. Check `--dry-run` output before a real run when multiple worktrees share one store
- `oxo-flow env gc` only ever deletes inside the store root — workflow-tree symlinks are never followed or removed, and store contents are never modified, only whole dirs deleted

---

## Options

| Option | Short | Description |
|---|---|---|
| `--verbose` | `-v` | Enable debug-level logging |

---

## Examples

```bash
# List available backends
oxo-flow env list

# Check a specific workflow
oxo-flow env check pipeline.oxoflow
```

---

## Notes

- `env check` exits with code `1` if any environment validation fails
- The check verifies backend availability, not that specific conda environments or Docker images exist — it confirms that the required *type* of environment manager is installed
- Run `env check` before submitting to a cluster to catch environment issues early
- To check that environment **definition files** exist (conda YAML, venv
  directories, `pixi.toml`), use
  [`oxo-flow test --deep`](test.md#deep-checks-deep) (finding `D002`)
- Only **pixi** and **venv** environments participate in the shared store today; conda/docker/singularity keep their native cache layouts. See [`env gc`](#env-gc) for store maintenance
