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

List all environment backends available on the current system.

```bash
oxo-flow env list
```

**Output:**

```
oxo-flow 0.5.1 — Bioinformatics Pipeline Engine
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
| `<WORKFLOW>` | Path to the `.oxoflow` workflow file |

**Output (all valid):**

```
  ✓ align (conda)
  ✓ call_variants (docker)
  ✓ annotate (singularity)
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
