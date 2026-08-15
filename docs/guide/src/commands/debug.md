# `oxo-flow debug`

Debug a workflow by showing each rule with its fully resolved shell command,
outputs, and dependencies. Useful for verifying that template variables are
substituted correctly.

---

## Usage

```
oxo-flow debug <WORKFLOW> [OPTIONS]
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
| `--rule <NAME>` | `-r` | Show only a specific rule (by name) |
| `--ai` | — | Enable AI-powered command explanation |
| `--verbose` | `-v` | Enable debug-level logging |

---

## Examples

### Debug all rules in a workflow

```bash
oxo-flow debug pipeline.oxoflow
```

### Debug a specific rule

```bash
oxo-flow debug pipeline.oxoflow -r bwa_align
```

---

## Output

For each rule, the debug command shows:

- **Rule name** and description (when the rule declares one)
- **Outputs** (with wildcard patterns expanded)
- **Shell (expanded)** — the fully resolved shell command
- **Dependencies** — other rules that must run first

```
oxo-flow 0.12.0 — Bioinformatics Pipeline Engine
Debug: Debugging 3 rules
── Rule: transform ──
  Outputs: ["data/filtered.csv"]
  Shell (expanded): head -1 data/raw.csv > data/filtered.csv
awk -F',' 'NR>1 && $3 > 500' data/raw.csv >> data/filtered.csv

  Dependencies: ["generate_data"]
```

---

## Notes

- The debug command does not execute any shell commands
- Template variables like `{input}`, `{output}`, and `{threads}` are
  substituted in the expanded view
- Wildcard rules are expanded per sample before display: rule names get a
  `_<sample>` suffix (e.g. `bwa_mem2_align_cohort_NA12878`) and wildcard
  placeholders are replaced with concrete values
- With `--rule`, use the full expanded rule name as shown in the output
  (for wildcard workflows, the template name like `bwa_mem2_align` will not match)
- Use this command to verify variable substitution before running a workflow
