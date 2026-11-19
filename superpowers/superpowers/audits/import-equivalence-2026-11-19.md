# Corpus-wide import declared-equivalence — first measured corpus (#1165)

> **Status**: first measured corpus for README bet 3 ("Universal ecosystem
> transpiler", outstanding: *corpus-wide semantic-equivalence testing*) and
> ROADMAP Phase 12.3. This measures **declared-artifact equivalence** of the
> Nextflow/Snakemake importer over the repo's whole `tests/fixtures/import/**`
> corpus — the *quiet* failure mode where `oxo-flow import` succeeds but
> changes declared semantics. It is **not** execution equivalence and makes no
> execution claim (see §7). PR references issue #1165 with `Refs`, never
> `Closes`.

## 1. What was measured

The importer `oxo-flow import <source>` (`crates/oxo-flow-core/src/transpile/`)
translates a deterministic subset of Nextflow DSL2 and Snakemake into
`.oxoflow` TOML; out-of-subset constructs are hard errors that name the
construct. This audit puts the **whole fixture corpus** through the *real CLI
binary* and compares each accepted workflow against declared-artifact truth
**derived by hand from the source file**, not copied from the importer's output.

Concretely, per accepted workflow the comparison covers:

- **rule identities** (the set/order of rule names);
- **DAG edge structure** — who feeds whom, from declared outputs matched to
  declared inputs plus explicit `depends_on`;
- **declared resources** — `threads`, `memory`, `time_limit`;
- **declared environment** — conda / docker / modules;
- **declared inputs and outputs** — the wildcard-pattern path lists;
- **declared shell** — the command verb and the placeholder tokens it carries.

Corpus = **18 fixtures** (8 Nextflow `.nf`, 10 Snakemake `.smk`) under
`tests/fixtures/import/`. Every fixture was read to learn its declared
contract before the importer was run; the truth table is
`scripts/import_equivalence_expectations.py` (with `cited` source line ranges).

## 2. Per-arm identity (ledger §8)

| Field | Value |
| :--- | :--- |
| hostname | `shixiangwangdeiMac.local` |
| `uname -a` | `Darwin 25.6.0 … RELEASE_ARM64_T8122 arm64` |
| arch | `arm64` |
| CPUs | 8 |
| OS | macOS (Darwin 25.6.0) |
| corpus baseline commit | `b7be10ba844df04b51299264184c207cca95286c` (contains all 18 fixtures; importer unchanged to this PR's head) |
| harness + truth table | introduced in this PR's head `5e90bd05` (`scripts/import-equivalence.sh`, `scripts/import_equivalence_{expectations,run}.py`) |
| branch | `research/1165-import-equivalence` |
| oxo-flow | `0.32.0` (`target/debug/oxo-flow`) |
| rustc / cargo | `1.98.0` / `1.98.0` |
| python3 | `3.12.4` (stdlib `tomllib`) |
| shell | `/bin/zsh` |
| date (UTC) | 2026-11-19 |

## 3. Exact command

```bash
# from the worktree root, after `cargo build -p oxo-flow-cli`
scripts/import-equivalence.sh --bin ./target/debug/oxo-flow
```

The harness runs the real binary over all 18 fixtures, parses each emitted
TOML with `python3 -m tomllib`, compares against the truth table, prints the
per-case table and the confusion matrix. It is **report-only by default
(always exits 0, drift included)**; `--strict` makes it exit non-zero if any
supported fixture drifts or any rejected fixture unexpectedly imports. (The
harness captures the runner's status as `rc=0; python3 ... || rc=$?` so
`set -euo pipefail` cannot abort before the `--strict` check.) It cannot run
vacuously: it
resolves a real `oxo-flow` binary (via `OXO_FLOW_BIN`, `--bin`, else
`target/debug`, else `PATH`) and fails hard if none is found.

## 4. Pre-registered acceptance criterion

**Registered 2026-11-19T01:40Z, BEFORE the corpus run** (the expectations file
`scripts/import_equivalence_expectations.py` was authored from the source
fixtures first; the harness was then run against it):

> A **supported** fixture is **MATCHED** iff all of the following hold for the
> emitted workflow: (a) the rule-name list equals the source's declared
> process/rule names; (b) the DAG edge set (explicit `depends_on` plus
> output→input path matches) equals the source's declared data flow; (c) every
> declared `threads`/`memory`/`time_limit` equals the source directive after
> the engine's **documented** unit normalization (`'8 GB'`→`8G`,
> `mem_mb 8000`→`8000M`; `transpile/mod.rs::normalize_memory`,
> `scheduler.rs::parse_memory_mb`); (d) every declared environment matches the
> source's conda/docker/module declaration (container wins over a conda
> fallback, as Nextflow runs it); (e) the declared input/output path lists
> equal the source's patterns; (f) the shell carries the source's command verb
> and its parameterized tokens as `{...}` placeholders.
>
> A **rejected** fixture is **IMPORTER-REJECTED** iff the real CLI exits
> non-zero and names a construct that is in the documented out-of-subset list
> (`docs/guide/src/commands/import.md`). **Drifted** = a supported fixture that
> fails (a)–(f). **Not-attempted** = a fixture the harness could not exercise
> with the real binary.

Anything not covered by (a)–(f) — e.g. the dropped `versions.yml` output, the
`{meta.id}`→`{sample}` key renaming, positional `{input[0]}` mapping — is
recorded as an **off-criterion observation** in §6, not silently folded into a
MATCHED verdict.

## 5. Confusion matrix and per-case table

Headline: **MATCHED 4 / DRIFTED 0 / IMPORTER-REJECTED 14 / NOT-ATTEMPTED 0**
over a corpus of 18.

| case | source | imported? | drift? | what (declared) | rationale / cited source |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `nextflow/rnaseq.ok.nf` | Nextflow | yes | no | 3 rules trim→align→count, resources, conda, in/out | processes TRIM/ALIGN/COUNT L2–42, wiring L58–60 |
| `nextflow/nfcore_module.ok.nf` | Nextflow | yes | no\* | 1 rule fastqc, container, in/out | process FASTQC L3–36; \*versions.yml output dropped, §6 |
| `snakemake/expand.ok.smk` | Snakemake | yes | no | 1 rule quantify, modules, temp, localrule | rule quantify L3–18 |
| `snakemake/rnaseq.ok.smk` | Snakemake | yes | no | 3 rules trim/align/count, conda/docker | rules L6–48; `all` dropped L50 |
| `nextflow/bare_glob_output.err.nf` | Nextflow | no | — | — | hard error: bare glob `*.bam` |
| `nextflow/channel_operator.err.nf` | Nextflow | no | — | — | hard error: `.map { it }` |
| `nextflow/dynamic_container.err.nf` | Nextflow | no | — | — | hard error: dynamic `container` |
| `nextflow/include.err.nf` | Nextflow | no | — | — | hard error: cross-file `include` |
| `nextflow/mixed_outputs.err.nf` | Nextflow | no | — | — | hard error: mixed wildcard/constant outputs |
| `nextflow/standalone_module.err.nf` | Nextflow | no | — | — | hard error: uninvoked module |
| `snakemake/directory.err.smk` | Snakemake | no | — | — | hard error: `directory(...)` |
| `snakemake/dynamic_input.err.smk` | Snakemake | no | — | — | hard error: `lambda wildcards:` |
| `snakemake/expand_fan_in.err.smk` | Snakemake | no | — | — | hard error: mixed wildcard/constant outputs |
| `snakemake/include.err.smk` | Snakemake | no | — | — | hard error: global `include:` |
| `snakemake/mixed_outputs.err.smk` | Snakemake | no | — | — | hard error: mixed wildcard/constant outputs |
| `snakemake/params.err.smk` | Snakemake | no | — | — | hard error: `params:` |
| `snakemake/rules_output_fan_in.err.smk` | Snakemake | no | — | — | hard error: mixed wildcard/constant outputs |
| `snakemake/run_body.err.smk` | Snakemake | no | — | — | hard error: `run:` |

**Drift cells: none.** No supported fixture failed criterion (a)–(f). Every
rejected fixture names a construct that is present in the documented
out-of-subset list — the measured rejection boundary matches the asserted one.

Source-of-rejection concentration: 4 of the 14 rejections (3 Snakemake + 1
Nextflow) share a single cause — *mixed wildcard/constant outputs* (fan-in
collision), the most common out-of-subset shape in the corpus.

## 6. Enumerated off-criterion observations

These are real declared-artifact differences that the (a)-(f) criterion does
not score. They are recorded so the "MATCHED" verdict is not read as
"byte-identical". Each is either **warned** by the importer (on-subset side) or
**below-threshold/systematic** (a documented normalization or a key-shape
choice a single-instance fixture cannot distinguish).

| # | observation | class | where |
| :--- | :--- | :--- | :--- |
| O1 | `versions.yml` output (emit `versions`) dropped; source declares 3 outputs, imported has 2 | below-threshold | `nextflow.rs:1639` |
| O2 | memory unit respelled, scale preserved: `8 GB`->`8G`, `32 GB`->`32G`; `mem_mb 8000`->`8000M` | systematic (documented) | `transpile/mod.rs::normalize_memory`, `scheduler.rs::parse_memory_mb` |
| O3 | `${meta.id}`->`{sample}` fan-out key renamed to canonical wildcard | systematic | source n=1, single {sample} |
| O4 | named I/O -> positional `{input[0]}/{input[1]}` (rnaseq.smk) | systematic (order-preserving) | deterministic from declared order |
| O5 | `{log}` left untranslated in the rnaseq.smk shell | below-threshold | compiles; runtime needs engine to populate `{log}` |
| O6 | `label 'process_single'` not imported | warned | importer WARNING |
| O7 | `when: task.ext.when` gate not imported (rule runs unconditionally) | warned | importer WARNING |
| O8 | `def args = task.ext.args` -> `{config.ext_args}` (requires `[config] ext_args`) | warned | importer WARNING |
| O9 | `expand("raw/{sample}.fastq", sample=["A","B"])` value list not materialized | warned | importer WARNING |
| O10 | Snakemake `rule all` (target/sink) dropped | warned | importer WARNING |
| O11 | `benchmark:` becomes a declared path but is not auto-populated | warned | importer WARNING |

O1 is the only observation that removes a **declared artifact** rather than
respelling or gating it. It is arguably on-table (the Nextflow hard-error list
in `docs/guide/src/commands/import.md` says *"versions.yml output | Skipped"*),
but it does not appear in the positive translation table, so it is flagged here
rather than buried. It is the single candidate if a future revision tightens
the criterion to count dropped outputs as drift. No importer change is made in
this PR.

## 7. Honesty boundary

**What this is.** A declared-artifact equivalence measurement over the repo's
own import corpus: 18 fixtures, real CLI, hand-derived truth. It answers
"does the importer preserve the *declared* contract (rules, edges, resources,
environments, I/O, shell shape) of every workflow it claims to support?"

**What this is NOT, stated loudly.**

- **Not execution equivalence.** No rule was run. No container was pulled. No
  bioinformatics tool (fastqc/STAR/trimmomatic/featureCounts/salmon/subread)
  was invoked. The full #1165 ask — per-workflow execution equivalence on
  identical hosts/containers/resources — is **not** achievable in this
  environment (no container runtime, no toolchain, no real data) and is **not
  claimed**.
- **"MATCHED" is not "equivalent".** It means the declared facts in §1 agree
  after documented normalization. It says nothing about whether the generated
  shell produces the same files, or whether the workflow runs at all.
- **N is small and self-selected.** 4 supported fixtures is the *entire*
  current corpus, not a sample of real-world Nextflow/Snakefile code. This is a
  floor, not a coverage estimate: no nf-core pipeline, no large community
  workflow is in it. The confusion matrix bounds the *fixtures*, not the
  ecosystems.
- **The truth table is single-author.** The expected facts were derived by hand
  from the sources and are reviewed as part of this PR; they are not
  independently reproduced.
- **No importer change.** This PR adds measurement only. It does not rewrite
  the importer, does not fix any observation in §6, and does not close #1165.

**What would move it forward**: a corpus drawn from real-world workflows (so
the 4/14 split stops being the repo's own fixtures), execution equivalence on a
container host, and a second independent derivation of the truth table.

## 8. Raw-data lineage

| Artifact | Path |
| :--- | :--- |
| Harness (bash) | `scripts/import-equivalence.sh` |
| Corpus runner (python) | `scripts/import_equivalence_run.py` |
| Truth table (author-written) | `scripts/import_equivalence_expectations.py` |
| Corpus | `tests/fixtures/import/nextflow/*.nf`, `tests/fixtures/import/snakemake/*.smk` |
| Importer | `crates/oxo-flow-core/src/transpile/{nextflow,snakemake,mod}.rs` |
| Claimed boundary | `docs/guide/src/commands/import.md` |
| Corpus baseline commit | `b7be10ba844df04b51299264184c207cca95286c` — the fixtures under `tests/fixtures/import/**` and the importer are unchanged from this commit to the PR's head (verified with `git diff --stat b7be10ba 5e90bd05 -- tests/fixtures/import crates/oxo-flow-core/src/transpile`, empty) |
| Harness + truth table commit | `5e90bd05` — this PR's head; `scripts/import-equivalence.sh` and the truth table were introduced here, not in the corpus baseline commit |
