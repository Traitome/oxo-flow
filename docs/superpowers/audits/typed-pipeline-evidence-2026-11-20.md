# Real typed pipeline evidence — first `[types.*]` usage in the repo (#1272)

> **Status**: evidence addendum to the BioType truth-set reports. It closes one
> specific documented gap — §9 of
> [`biotype-truth-set-2026-11-20.md`](biotype-truth-set-2026-11-20.md) said
> verbatim *"**Still no `[types.*]` usage in the repo.** The typed-correctness
> evidence rests entirely on hand-authored cases, not on real typed pipelines."*
> That clause is now **falsified**: shipped gallery example
> `examples/gallery/17_typed_variant_calling.oxoflow` declares a
> `[types.*]` / `[rules.types]` contract on **every crossing edge** of a real,
> complete, runnable pipeline. It is **not** the governed campaign; the
> pre-registration threshold `τ_FA` remains UNDECIDED and no governed verdict is
> claimed here.

## 1. What was added

`examples/gallery/17_typed_variant_calling.oxoflow` — the gallery-13 per-sample
germline chain (FastQC/fastp → BWA-MEM2 → MarkDuplicates → BQSR →
HaplotypeCaller), reproduced **verbatim** for its shells, environments, and
sample group, with a biological type-contract layer added on top:

- Six **named, reusable** `[types.<name>]` tables: `trimmed_reads`,
  `sorted_bam`, `dedup_bam`, `recal_table`, `recal_bam`, `germline_gvcf`.
- Per-rule `[rules.types]` `inputs` / `outputs` channel maps binding each
  crossing edge to a named type on **both** sides.

It is a new file. No existing gallery example was mutated, so the
`gallery-03` / schema-drift gates on existing files are untouched.

## 2. Per-arm identity (ledger §8)

| Field | Value |
| :--- | :--- |
| hostname | `shixiangwangdeiMac.local` |
| `uname -srm` | `Darwin 25.6.0 arm64` |
| arch | `arm64` (Apple silicon) |
| rustc | `1.98.0 (88d9e12ae 2026-08-18)` |
| oxo-flow | `0.32.0`, built from the branch worktree |
| commit (branch HEAD at run) | `e855fa6a71631237d5973041caf95664af23590d` |
| branch | `research/1272-typed-pipeline` |
| binary | `target/debug/oxo-flow` (debug build) |

Single arm; the checker is platform-independent pure Rust, so a single arm is
the correct design for a *logic* result.

## 3. Exact commands and REAL output

```bash
cargo build -p oxo-flow-cli
oxo-flow validate examples/gallery/17_typed_variant_calling.oxoflow --json
```

```json
{
  "command": "validate",
  "dependencies": 6,
  "errors": [],
  "missing_inputs": [
    "raw/NA12878_R1.fastq.gz", "raw/NA12878_R2.fastq.gz",
    "raw/NA12891_R1.fastq.gz", "raw/NA12891_R2.fastq.gz",
    "raw/NA12892_R1.fastq.gz", "raw/NA12892_R2.fastq.gz"
  ],
  "rules": 7,
  "valid": true,
  "warnings": [],
  "workflow": "examples/gallery/17_typed_variant_calling.oxoflow"
}
```

- **`errors: []`** — no fatal TYPE code. Every crossing edge forms exactly one
  matched producer/consumer channel pair (so no `TYPE-010`), and every producer
  on a typed edge is typed (so no `TYPE-008`).
- **`missing_inputs`** — the six raw FASTQ placeholders, the same expected
  missing-input notices as gallery 13 (not a type finding).

`oxo-flow lint examples/gallery/17_typed_variant_calling.oxoflow --json`: 0
errors, 1 warning (`W023`, a command-substitution notice in `bwa_align` — the
same in gallery 13), 12 info, and **no `TYPE` diagnostics** on the lint surface
either.

`oxo-flow dry-run examples/gallery/17_typed_variant_calling.oxoflow`:
`Plan: would run: 21 | skip: 0 | completed: 0 (DAG size: 21)` — identical to
gallery 13. Types are a static validation layer; they change no execution
semantics.

## 4. Falsifiability — the contract is load-bearing

A clean accept is weak evidence on its own; the contract must be shown to *bind*.
Editing one **named type definition** in place keeps both sides consistent (both
producer and consumer reference the same name), so that is not a falsifiable
edit. A **one-sided** edit is:

```bash
# temp copy: change ONLY the consumer requirement on one edge
sed 's|inputs = { aligned = "sorted_bam" }|inputs = { aligned = "SAM" }|' \
  examples/gallery/17_typed_variant_calling.oxoflow > /tmp/broken.oxoflow
oxo-flow validate /tmp/broken.oxoflow --json
```

Result — `valid: false` with **exactly one** error:

```json
{
  "code": "TYPE-001",
  "message": "upstream rule 'bwa_align' (aligned -> aligned): biological format mismatch: producer emits bam, but consumer requires sam",
  "rule": "mark_duplicates",
  "suggestion": "connect an adapter rule to convert bam into sam"
}
```

The contract on the `bwa_align → mark_duplicates` edge is real: a one-sided
format mismatch is rejected in preflight by name, before any command runs.

## 5. Truth-set integration

The harness `scripts/biotype-truth-set.sh` now carries this file as an eighth
real-repository row (expected **accept**, `-` codes, since the contracts are
satisfied):

```
examples/gallery/17_typed_variant_calling.oxoflow  accept  -  real repo example, TYPED [types.*]/[rules.types] contracts (#1272)
```

Re-run at HEAD `e855fa6a`: **26 true-accept / 0 false-accept / 17 true-reject /
0 false-reject**, codes cross-check **43/43**. The only change vs the 42-case
run is `true-accept 25 → 26` and `cases 42 → 43` — the new typed example, which
passes on the real admission surface. Machine-readable results regenerated at
`docs/superpowers/audits/data/biotype-truth-set-results.json`.

Because the harness is a *measurement* (exit 0 always), this does not gate CI;
the example is independently covered by the auto-discovering gallery gates
`cli_validate_all_gallery_workflows` and `cli_lint_all_gallery_workflows`
(`tests/cli_integration.rs`), which validate+lint every `examples/gallery/*.oxoflow`.

## 6. Honesty boundary — what this does and does NOT prove

**Does prove:**

- A real, complete, runnable pipeline exists in the repo with a genuine
  `[types.*]` / `[rules.types]` contract on **every** crossing edge — the
  "no `[types.*]` usage in the repo" clause is closed.
- On that real pipeline, the checker accepts when contracts match and rejects,
  by name, a one-sided mismatch — the contract is load-bearing.

**Does NOT prove / not claimed:**

- **Validated, not executed.** The pipeline was parsed, DAG-built, validated,
  linted, and dry-run — no real data was staged and no bioinformatics tool was
  run.
- **Not the governed campaign.** `τ_FA` remains UNDECIDED (PR-06 §C1), so no
  POSITIVE/NEGATIVE/INCONCLUSIVE decision is claimed. This is one real pipeline,
  not a breadth corpus; the truth-set breadth boundary in the 42-case report §9
  still applies (no `transform.*`-generated rules, no `[[include]]`-contributed
  types on this example; formats limited to FASTQ/BAM/TSV/VCF).
- **gVCF attribute path.** The `germline_gvcf` type uses the expanded form of
  the built-in `gvcf` alias (`format = "vcf"`, `compression = "bgzip"`,
  `attributes = { variant_type = "germline" }`); the alias equivalence itself is
  covered by the hand-authored truth set, not re-proven here.
- **Single arm, single toolchain** (macOS arm64, rustc 1.98.0).

## 7. References

- `examples/gallery/17_typed_variant_calling.oxoflow` — the typed pipeline.
- `docs/guide/src/gallery/typed-variant-calling.md` — its gallery page.
- `docs/superpowers/audits/biotype-truth-set-2026-11-20.md` — the 42-case
  breadth run this addendum extends; its §9 clause is updated by this change.
- `docs/superpowers/preregistration/PR-06-publication-decision-rule.md`
  (v1.0.1, FROZEN 2026-11-18) §C1 — `τ_FA` UNDECIDED.
