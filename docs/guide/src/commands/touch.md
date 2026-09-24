# `oxo-flow touch`

Mark workflow outputs as up-to-date without re-executing rules.

---

## Usage

```
oxo-flow touch [OPTIONS] <WORKFLOW> [KEY=VALUE]...
```

---

## Arguments

| Argument | Description |
|---|---|
| `<WORKFLOW>` | Path to the `.oxoflow` workflow file |
| `[KEY=VALUE]...` | Direct config overrides as trailing positionals — the same forms `run` accepts (`KEY=VALUE`, `--KEY=VALUE`, and the declared-key-only `--KEY VALUE`; see [run](run.md)). Needed when outputs or wildcard expansion branch on `{config.*}` (issue #432). Command flags must come **before** the overrides. |

---

## Options

| Option | Short | Description |
|---|---|---|
| `--rule` | `-r` | Specific rule(s) whose outputs to touch |
| `--workdir <DIR>` | `-d` | Working directory the outputs live in (default: the workflow file's directory) |

---

## Examples

### Touch all outputs

```bash
oxo-flow touch pipeline.oxoflow
```

### Touch outputs of a specific rule

```bash
oxo-flow touch pipeline.oxoflow -r bwa_align
```

### Touch outputs whose paths depend on config

```bash
# Output pattern "{config.out_dir}/x.bam" — resolve against the override,
# exactly as `run` would with the same value
oxo-flow touch pipeline.oxoflow out_dir=results/cohort-A
```

---

## Output

```
oxo-flow v0.20.0 — Rust-native bioinformatics pipeline engine
  ✓ sample1.bam
  ✓ sample1.bam.bai
  ✓ sample2.bam
  ✓ sample2.bam.bai

Done: 4 file(s) touched, 0 wildcard pattern(s) skipped
```

---

## Notes

- Updates the modification time of output files to the current time
- If an output file does not exist, an empty file will be created
- Useful for forcing the engine to skip rules when outputs were generated externally
- Wildcard patterns are skipped as they cannot be touched without specific values
- Config overrides reuse `run`'s override machinery, so `touch` and `run`
  agree on what an override means and on the resolved output locations
  (issue #432)
