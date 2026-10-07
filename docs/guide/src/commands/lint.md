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
| `--quiet` | — | — | Global log-level flag: suppresses `info`-level log lines and the banner. Lint diagnostics and the summary are printed regardless |
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
oxo-flow v0.23.0 — Rust-native bioinformatics pipeline engine
  warning [W003]: rule has no description (rule: bwa_align)
    hint: add description = "Brief one-line description of what bwa_align does" to this rule
  info [W004]: rule has a shell command but no log file specified (rule: bwa_align)
    hint: add log = "logs/bwa_align.log"
  info [W007]: leaf rule (no dependents) could be marked as target = true (rule: fastqc)
    hint: add target = true to this rule
  info [W025]: rule uses deprecated rule-level threads/memory (rule: bwa_align)
    hint: replace `threads = N` / `memory = "8G"` with `resources.threads = N` / `resources.memory = "8G"` under this rule

Summary: 0 error(s), 1 warning(s), 3 info
```

Each diagnostic prints a `hint:` line (when a suggestion exists) showing
the fix, matching the style of `validate` and `run` output. W007
suggests `target = true`: a rule marked as a target is built by default
when `oxo-flow run` is invoked without an explicit `-t`, so marking the
final leaf rules (like `fastqc` above) makes them part of the default
run — see [Workflow Format: Priority and Targeting](../reference/workflow-format.md#priority-and-targeting).
W025 flags the deprecated rule-level `threads = N` / `memory = "8G"`
fields (superseded by `[rules.resources]` in v0.4) so old workflows
surface the migration instead of silently keeping their old settings —
see [Workflow Format: Rule resources](../reference/workflow-format.md#resources-extended).
W031 flags a consumer rule that reads the output of a `when`-gated
producer without a `when` gate of its own: when the producer's gate is
off, its files never appear and the consumer's inputs cannot be resolved
at plan time (dry-run `input ✗`) or the run fails outright. Wildcard
templates match after canonicalizing every `{...}` placeholder, and
fully literal paths match literally — a literal input of a gated
producer is the same hazard. The repair is either a matching `when` gate
on the consumer or splitting it into `when`-gated variants (the
`multiqc`/`multiqc_pseudo` idiom). Consumers that declare their
tolerance for a missing producer are not flagged: `optional = true` /
`"any"` rules, `input_groups` disk-discovery fallbacks, and consumers
that already carry any `when` gate.
W033 flags two rules that write the same output path — including
differently-named wildcards over the same template (`variants/{smp}.vcf`
vs `variants/{sample}.vcf`) and identical literal paths. The engine does
not refuse to run: both rules execute and the second to finish silently
overwrites the first (verified live in v0.23.0 with a run reporting
success while both colliding rules executed), so downstream rules consume
the wrong file. The repair is distinct output directories per writer —
or a `when` gate when the two rules are mutually exclusive alternatives
(the `wgs_coverage`/`wes_coverage` idiom). Multi-producer outputs remain
a supported feature, so this is a warning, not an error. `{config.*}`
placeholders are resolved against the parsed config before the templates
are compared, so `{config.out_dir}/{sample}.vcf` collides with its literal
counterpart `results/{sample}.vcf` when `out_dir = "results"` — and does
not collide with `{config.alt_dir}/{sample}.vcf` when the values differ.
See
[Troubleshooting → Output collisions](../how-to/troubleshooting.md#output-collisions-silent-overwrite).
W034 flags an `{input[N]}` / `{output[N]}` placeholder whose index is out
of range, or a named `{input.key}` / `{output.key}` whose key the rule
does not declare. The substitution loop only renders indices `0..len`
(list-form: declaration order; map-form: keys in sorted order; dir-form:
the single entry), and only renders named references for declared keys —
anything else survives rendering as literal brace text, so the tool is
invoked with a path like `{input[5]}` and fails with an unrelated
"No such file" error that hides the real mistake. The repair depends on
the input shape: use an in-range index (map-form indices follow the
rule's sorted keys, not the text order), switch to the bare `{input}` /
`{output}` form to interpolate all entries, or — preferred for
role-bearing IO — declare a map and use named access (`tumor = "..."` +
`{input.tumor}`), which also survives reordering. Lines that are comments
in the rendered script are exempt, and `script` paths are checked the
same way as inline `shell`.
W032 flags a `[config]` key whose *name* looks like a secret
(`token`, `secret`, `passwd`, `password`, `credential`, `api_key`,
`access_key`, `private_key`, `ssh_key`, `key` — matched at word/segment
boundaries, so names that merely contain these as a substring, like
`tokenize` or `password_length_hint`, are not flagged) but is not
declared `sensitive`: its value then lands in plaintext in command
records, stderr tails, and the checkpoint — credential leakage on shared
clusters and in CI artifacts. The repair is the declaration itself:

```toml
[config]
api_token = { default = "sk-...", sensitive = true }
```

Declared-sensitive keys are env-routed and masked on disk, so W032
never flags them — see
[`[config]` — Configuration Variables](../reference/workflow-format.md#config-configuration-variables).

---

## Notes

- `lint` is a strict superset of [`validate`](validate.md): it runs every
  validate check (parsing, DAG, input existence) plus style linting and
  secret scanning — running `validate` separately after `lint` adds nothing
- Linting checks for common mistakes, missing metadata, and potential performance issues
- Rules are checked for valid input/output patterns and environment declarations
- Use `--strict` to ensure high-quality workflow definitions in production environments

Every finding carries a stable code (`E0xx` = validation error, `W0xx` =
lint warning). The full code index with one-line meanings lives in the
[Glossary → Diagnostic Codes](../reference/glossary.md#diagnostic-codes).
