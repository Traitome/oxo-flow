# `oxo-flow diff`

Compare two `.oxoflow` workflow files and show differences.

---

## Usage

```
oxo-flow diff <WORKFLOW_A> <WORKFLOW_B>
```

---

## Arguments

| Argument | Description |
|---|---|
| `<WORKFLOW_A>` | First workflow file |
| `<WORKFLOW_B>` | Second workflow file |

---

## Examples

### Compare two workflows

```bash
oxo-flow diff v1.oxoflow v2.oxoflow
```

---

## Output

```
oxo-flow 0.5.1 — Bioinformatics Pipeline Engine
Diff: 2 difference(s) between v1.oxoflow and v2.oxoflow:
  • [rule] rule 'bwa_align' shell command changed
  • [config] variable 'threads' changed from 8 to 16
```

---

## Notes

- Performs a semantic comparison of workflow structures, not just a line-by-line diff
- Detects changes in rules, configuration variables, and metadata
- Useful for tracking changes during pipeline development
