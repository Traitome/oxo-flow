# `oxo-flow touch`

Mark workflow outputs as up-to-date without re-executing rules.

---

## Usage

```
oxo-flow touch [OPTIONS] <WORKFLOW>
```

---

## Arguments

| Argument | Description |
|---|---|
| `<WORKFLOW>` | Path to the `.oxoflow` workflow file |

---

## Options

| Option | Short | Description |
|---|---|---|
| `--rule` | `-r` | Specific rule(s) whose outputs to touch |

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

---

## Output

```
oxo-flow 0.4.2 — Bioinformatics Pipeline Engine
  ✓ sample1.bam
  ✓ sample1.bam.bai
  ✓ sample2.bam
  ✓ sample2.bam.bai

Done: 4 file(s) touched, 0 wildcard patterns skipped
```

---

## Notes

- Updates the modification time of output files to the current time
- If an output file does not exist, an empty file will be created
- Useful for forcing the engine to skip rules when outputs were generated externally
- Wildcard patterns are skipped as they cannot be touched without specific values
