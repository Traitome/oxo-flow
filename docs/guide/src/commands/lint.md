# `oxo-flow lint`

Run best-practice linting checks on a `.oxoflow` file.

---

## Usage

```
oxo-flow lint [OPTIONS] <WORKFLOW>
```

---

## Arguments

| Argument | Description |
|---|---|
| `<WORKFLOW>` | Path to the `.oxoflow` workflow file |

---

## Options

| Option | Short | Default | Description |
|---|---|---|---|
| `--strict` | — | — | Treat warnings as errors (non-zero exit) |
| `--ai` | — | — | Enable AI-powered semantic linting |
| `--verbose` | `-v` | — | Enable verbose (debug-level) logging |
| `--quiet` | — | — | Suppress non-essential output (errors only) |
| `--no-color` | — | — | Disable colored output |
| `--json` | — | — | Output machine-readable JSON to stdout |

---

## Examples

### Run standard linting

```bash
oxo-flow lint pipeline.oxoflow
```

### Run strict linting

```bash
oxo-flow lint pipeline.oxoflow --strict
```

---

## Output

```
oxo-flow v0.17.1 — Rust-native bioinformatics pipeline engine
  warning [W003]: rule has no description (rule: bwa_align)
    hint: add a `description` field to the rule
  warning [W004]: rule has a shell command but no log file specified (rule: bwa_align)
    hint: add `log = "logs/bwa_align.log"` to the rule
  info [W007]: leaf rule (no dependents) could be marked as target = true (rule: fastqc)
  info [W025]: rule uses deprecated rule-level threads/memory (rule: bwa_align)
    hint: move `threads`/`memory` under `[rules.resources]`

Summary: 0 error(s), 2 warning(s), 2 info
```

Each diagnostic prints a `hint:` line (when a suggestion exists) showing
the fix, matching the style of `validate` and `run` output. W007
suggests `target = true`: a rule marked as a target is built by default
when `oxo-flow run` is invoked without an explicit `-t`, so marking the
final leaf rules (like `fastqc` above) makes them part of the default
run — see [Workflow Format: Priority and Targeting](../reference/workflow-format.md#priority-and-targeting).
W025 flags the deprecated rule-level `threads = N` / `memory = "8G"`
fields (removed in v0.17.1 in favor of `[rules.resources]`) so old
workflows surface the migration instead of silently keeping their old
settings — see [Workflow Format: Rule resources](../reference/workflow-format.md#resources-extended).
W031 flags a consumer rule that expands the full wildcard output of a
`when`-gated producer without a `when` gate of its own: when the
producer's gate is off, its files never appear and the consumer's inputs
cannot be resolved at plan time (dry-run `input ✗`). The repair is
either a matching `when` gate on the consumer or splitting it into
`when`-gated variants (the `multiqc`/`multiqc_pseudo` idiom). Consumers
that declare their tolerance for a missing producer are not flagged:
`optional = true` / `"any"` rules, `input_groups` disk-discovery
fallbacks, and consumers that already carry any `when` gate.

---

## Notes

- `lint` is a strict superset of [`validate`](validate.md): it runs every
  validate check (parsing, DAG, input existence) plus style linting and
  secret scanning — running `validate` separately after `lint` adds nothing
- Linting checks for common mistakes, missing metadata, and potential performance issues
- Rules are checked for valid input/output patterns and environment declarations
- Use `--strict` to ensure high-quality workflow definitions in production environments
