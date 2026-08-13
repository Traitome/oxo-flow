# `oxo-flow profile`

Manage execution profiles (local, SLURM, PBS, SGE, LSF).

---

## Usage

```
oxo-flow profile [OPTIONS] <COMMAND>
```

---

## Actions

| Action | Description |
|---|---|
| `list` | List available execution profiles |
| `show` | Show details of a specific profile |
| `current` | Show the current active profile |

---

## Arguments

| Argument | Description |
|---|---|
| `<NAME>` | Profile name (e.g., `local`, `slurm`, `pbs`, `sge`, `lsf`) |

---

## Examples

### List all profiles

```bash
oxo-flow profile list
```

### Show SLURM profile details

```bash
oxo-flow profile show slurm
```

### Show current active profile

```bash
oxo-flow profile current
```

---

## Output

```
oxo-flow 0.10.2 — Bioinformatics Pipeline Engine
Available execution profiles:
  • local — Local execution (default)
  • slurm — SLURM cluster scheduler
  • pbs — PBS/Torque cluster scheduler
  • sge — Sun Grid Engine (SGE) scheduler
  • lsf — IBM LSF scheduler
```

---

## Options

| Option | Description |
|--------|-------------|
| `-v, --verbose` | Enable verbose (debug-level) logging |
| `--quiet` | Suppress non-essential output (errors only) |
| `--no-color` | Disable colored output |
| `--json` | Output machine-readable JSON to stdout |

## Notes

- Profiles define how jobs are submitted and monitored across different computing environments
- The `local` profile is used by default if no other profile is specified
- Profile configurations can be customized with `profiles/<name>.toml` (or `.oxoflow`) files in the workflow directory, used via `oxo-flow run --profile <name>`
