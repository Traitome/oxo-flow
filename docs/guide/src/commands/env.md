# `oxo-flow env`

Manage software environments. Provides subcommands for listing available backends and checking workflow environment requirements.

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
oxo-flow 0.8.0 — Bioinformatics Pipeline Engine
Available environment backends:
  ✓ conda
  ✓ docker
  ✓ singularity
  ✓ venv
```

### `env check`

Check that all environments declared in a workflow file are valid and their backends are available.

```bash
oxo-flow env check <WORKFLOW>
```

| Argument | Description |
|---|---|
| `[WORKFLOW]` | Optional path to the `.oxoflow` workflow file. If omitted, checks system-wide backend availability instead. |

**Output (all valid):**

```
  ✓ align (conda)
  ✓ call_variants (docker)
  ✓ annotate (singularity)

### `env create`

Create a new environment from a specification file.

```bash
oxo-flow env create <SPEC> [-n <NAME>]
```

| Argument | Description |
|---|---|
| `<SPEC>` | Path to the environment specification file (`.yaml`, `.toml`, `.lock`) |

| Option | Short | Description |
|---|---|---|
| `--name` | `-n` | Custom name for the created environment (default: derived from the spec filename) |
```

**Output (missing backend):**

```
  ✓ align (conda)
  ✗ call_variants — Docker is not available on this system
```

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
