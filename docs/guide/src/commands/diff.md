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
oxo-flow v0.15.0 — Rust-native bioinformatics pipeline engine
Diff: 2 difference(s) between v1.oxoflow and v2.oxoflow:
  • [rules] rule "bwa_align": shell command changed
  • [config] config variable changed: "threads"
```

---

## Notes

- Performs a semantic comparison of workflow structures, not just a line-by-line diff
- Detects changes in rules, configuration variables, and metadata
- Useful for tracking changes during pipeline development
- All output goes to **stderr** and the exit code is always 0 — CI jobs
  must capture stderr (not the exit code) to detect differences
