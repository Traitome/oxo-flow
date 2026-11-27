# Real-world import execution-equivalence — first measured corpus (#1398)

> **Status**: the first measured SOURCE-vs-IMPORTED **execution**-equivalence
> result over real-world workflows. Two workflows derived from published
> upstream sources were executed in their native engine and as imported
> `.oxoflow` workflows on one host, over identical inputs and one pinned
> container image digest; both produced byte-identical declared outputs.
> This is the execution half of the transpiler ask that
> [`import-equivalence-2026-11-19.md`](import-equivalence-2026-11-19.md) §9
> recorded as `executed = 0`. PR references issue #1398 with `Fixes` only if
> the acceptance criterion (at least one real-world workflow executes on both
> arms with byte-identical declared outputs) is genuinely met, which it is:
> 2 fixtures matched, 0 drifted.

## 1. What was measured

For each fixture the same source workflow is expressed twice and each
expression is executed on the **same host, same planted inputs, same pinned
container image**:

- **SOURCE** — the native engine (`nextflow run ... -with-docker`, or
  `snakemake -s ... --use-singularity`), running the fixture exactly as
  committed.
- **IMPORTED** — `oxo-flow import <source> -o W.oxoflow` followed by
  `oxo-flow run W.oxoflow -d <staged dir>`, running the importer's output.

The comparison endpoint is the **complete produced-artifact path→SHA-256
map** after each arm runs (planted inputs, the seeded workflow files, and
engine metadata are excluded). When the imported workflow declares literal
outputs (no wildcard syntax), each is additionally required to be present in
both arms; a fixture whose `execution` block pins a container must import to
that same container (the `docker://` prefix is normalized), else it is
SKIPPED. The harness enumerates supported fixtures from
`scripts/import_equivalence_expectations.py` (`status == "supported"`), so the
execution corpus cannot drift from the declared-artifact truth table. A
fixture is **MATCHED** only when BOTH arms genuinely executed (exit 0) and
each produced at least one artifact; a non-running arm, missing tool, missing
backend, or non-zero exit is **SKIPPED** with the reason; two executed arms
with empty artifact sets are **NO_ARTIFACTS**, never MATCHED (the #1349 rule).
These rules pre-date this campaign (the harness was authored for #1300); this
campaign did not relax them. The container and literal-declared-output
validations were added by the #1398 review and are covered by the hermetic
harness tests. The campaign evidence below was produced by the same runner
logic without those two assertions; re-checking the two real fixtures against
the review harness confirms both satisfy them: their imported environment
matches the pinned container after `docker://` normalization, and every
literal declared output is present in both recorded arm artifact maps.

The two new real-world fixtures:

| fixture | upstream source (cited in the fixture header) | tool | declared output |
| :--- | :--- | :--- | :--- |
| `nextflow/nfcore_faidx.ok.nf` | nf-core/modules `modules/nf-core/samtools/faidx/main.nf`, commit `49d0ca7a887eeb1b85cfb68ab18c8039753bd1de` (retrieved 2026-11-27) | `samtools faidx` | `GENOME_PATH/genome.fna.fai` |
| `snakemake/training_faidx.ok.smk` | troycomi/snakemake-training `2_containers/Snakefile`, rule `index_reference` lines 49–59, commit `1b2861ccc52bda56c0838cdd5b6f55c23fc16e2e` (retrieved 2026-11-27) | `samtools faidx` | `GENOME_PATH/GCF_000001215.4_Release_6_plus_ISO1_MT_genomic.fna.fai` |

Both fixtures are runnable as committed (the harness plants the FASTA inputs)
and both are inside the importer's documented subset. Every wrapper change is
listed in the fixture header and in §6.

## 2. Pre-run manifest (PR-03 §0 format)

| Field | Value |
| :--- | :--- |
| `campaign` | #1398 real-world import execution-equivalence |
| `engine_commit` | host checkout at campaign time `94f3c8ec2b4ae05564a102c83caa95255396f8a9` (recorded in `data/import-execution-realworld-2026-11-27-identity.txt`). The binary was built from `8be843433dfeab31312e22969d1c98adee822649`; commits between are fixtures/scripts/docs only. The review head later changed `nextflow.rs` by a comment/attribute move and `nextflow_export.rs` by the export-escaping fix — neither is on the measured import/run path |
| `engine_version` | `oxo-flow 0.32.0` (`target/debug/oxo-flow`) |
| `binary_sha256` | `2a9e7e89a49a04b0c882bb88e533c979380519c5e6d51bc46207ca35339b19c7` |
| `schema_sha256` | `7f68d09a481f28a1568fb9a37eb14a75bc6ef57d2d504891ef8ce5aeebca47e0` (`docs/schema/oxoflow-v1.schema.json`) |
| `analysis_script_sha256` | campaign-time runner `3c6f18ea098787973dab8624540f65ac8e539bab473bb4c606159e01bca27e06`; wrapper `4bf51520e78340c51207d3f8fab54be0af995cb79ce65eb1e06dd03f5f36ceaa`; truth table `e0133af31aaf93db9d4d98a53bde66022dfcdfcc370cc271abf4a77955ebe7ba`. Review-head runner (container/declared-output validation added, hermetic tests) `5df2afd33cde81522cacb40ebc6418cc7b5369753e7df3a5d54d4c5f30660fc2`; no verdict change for these fixtures (re-checked against their imported documents and the recorded arm artifact maps) |
| `fixture_sha256` | campaign-time `nextflow/nfcore_faidx.ok.nf` `4357594f3a36d9280ce9ad40cd932e91f75cdc0dabdb92b57b1d315132299afc`; `snakemake/training_faidx.ok.smk` `217d1261c2a767e8ad21aaf90c64d2fe9cb7517f439894e8b1455befba85265a`. Review-head nf-core fixture `6ae0a2a4c8b92218fe0507f75e6b385a5d424a1132c3b4168487a210e8bc0d5f` differs by header comments only (execution-inert) |
| `importer_source_sha256` | measured code (checkout `94f3c8ec`): `nextflow.rs` `0c6d74882cebca70eb6d83a3c6454fb556d8762578848c14f4a67df77ba1ee80`, `snakemake.rs` `6d094b57253009b7c172b12154bd9ede5bc8842003e3c835028e80b06a27e48c`. Review head: `nextflow.rs` `0be1fd8b865f6cc194be2200ed1aaabcb6cbca1c014c00e9f541e12bb2aff1f6` (comment/attribute move only), `snakemake.rs` `d93465aec0ba2773fe6b6c551df3fc521d660a837713cdbbc83215c79d15664b` (test comment only), and `nextflow_export.rs` `dff9775a5c9c5ffd22f7f658b9e45d3016dc98ab6bd5d4fe4e7528ceef47aef5` (the export-escaping fix; the exporter is not on the measured import/run path) |
| `model` | None — execution is deterministic, no provider call |
| `prompt_sha256` | None — no prompt |
| `toolchain` | Nextflow **26.10.6 build 15032**; Snakemake **9.27.2**; OpenJDK 21.0.11; Python 3.12.3; Docker 29.7.2; Apptainer 1.3.6 |
| `container` | `community.wave.seqera.io/library/htslib_samtools:1.25--704fb80d75745bd4`; docker repo digest `sha256:58bb7138f8ce684713fe71c74066ffb17c79204bbbc73bc392827313fb91826a`; image id `sha256:7f9bddfc16f1c0a8f769164246fb6621aa9eb019a2b4d6187dca968263c3a03d`; Apptainer SIF used by the Snakemake arm `sha256:ee2c040e592631106826ae7b092414a3328837d63ec855aa09c7ba595cd7cb5a` (`/tmp/apptainer-cache/a16ad758c9b6ea309d329a8462906917.simg`) |
| `seed` | none — deterministic; both arms re-run in the campaign and in a separate `--strict` run with the same hashes |
| `hardware` | `tx-ubuntu` (`VM-4-4-ubuntu`); Linux 6.8.0-137-generic, x86_64; Ubuntu 24.04.4 LTS; 4 CPUs; 3 GiB RAM (~3.6 GB available) |
| `dataset` | one planted FASTA per fixture, identical bytes in both arms: `sha256:215e5e2c91bbc9dc682470da73b22a09ff39151a1606fa8d6827ccd2a3a1a822` (`>chr1` 36 bp, `>chr2` 16 bp) |
| `budget` | per-fixture arm timeout 1200 s for the two real-world fixtures (runner default 300 s otherwise); wall-clock is not recorded by this instrument (§7 O6). Observed: each arm completes in seconds once the image is materialized; the first Nextflow invocation adds JVM startup |
| `exclusions` | no rule reaches the network; images are materialized before the campaign; no cluster/cloud backend and no reference genome |

Baseline gap evidence (same harness, engine binaries deliberately absent from
`PATH`): `executed = 0`, `skipped = 8`, `matched = 0` — this is the exact gap
#1398 names, reproduced before the campaign
(`data/import-execution-realworld-2026-11-27-baseline.log`).

## 3. Exact commands

```bash
# On tx-ubuntu, checkout ~/oxo-1398 at c695e4d7, after
# CARGO_BUILD_JOBS=1 cargo build -p oxo-flow-cli

# baseline (no engine binaries on PATH) -> 0 executed / 8 skipped
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
  scripts/import-execution-equivalence.sh --bin target/debug/oxo-flow

# campaign
PATH=$HOME/bin:$HOME/smk-venv/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
APPTAINER_CACHEDIR=/tmp/apptainer-cache NXF_OPTS=-Xmx1g \
EQ_RECORDS_PATH=$HOME/1398-work/campaign-records.json \
  scripts/import-execution-equivalence.sh --bin target/debug/oxo-flow

# falsifiable strict run (exit 0 on matched/no-drift, non-zero only on drift)
PATH=$HOME/bin:$HOME/smk-venv/bin:... APPTAINER_CACHEDIR=/tmp/apptainer-cache \
NXF_OPTS=-Xmx1g scripts/import-execution-equivalence.sh --strict \
  --bin target/debug/oxo-flow
```

Per-fixture SOURCE arm commands are declared in the truth table's `execution`
block (the runner does not hardcode one engine's flags for these fixtures):

```text
nextflow/nfcore_faidx.ok.nf
  SOURCE:   nextflow run nfcore_faidx.ok.nf -with-docker -ansi-log false -work-dir work
  IMPORTED: oxo-flow run W.oxoflow -d <staged run dir>

snakemake/training_faidx.ok.smk
  SOURCE:   snakemake -s training_faidx.ok.smk --cores 2 --use-singularity
  IMPORTED: oxo-flow run W.oxoflow -d <staged run dir>
```

## 4. Pre-registered acceptance criterion

The harness contract (authored for #1300, hardened by #1349, unchanged here):

> A fixture is **MATCHED** iff both arms exit 0 and their produced
> path→SHA-256 artifact maps are non-empty and equal. A fixture where either
> arm did not run (missing runtime, missing backend, non-zero exit) is
> **SKIPPED** with the reason, never MATCHED. Two executed arms with empty
> artifact sets are **NO_ARTIFACTS**. **DRIFTED** names the differing paths.
> `--strict` exits non-zero iff an executed fixture drifted. The review
> hardening adds two fail-closed SKIP conditions: a pinned
> `execution.container` that the imported environment does not run, and a
> literal declared output missing from either arm.

The campaign was run twice (report-only and `--strict`); the strict invocation
returned `strict_rc=0`.

## 5. Results

Headline on `tx-ubuntu` (supported fixtures = 8, of which the two real-world
fixtures reached execution):

**executed 2 / matched 2 / drifted 0 / skipped 6 / no_artifacts 0**, in both
the report-only and the `--strict` invocation.

| fixture | arm | exit | produced artifacts | SHA-256 | verdict |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `nextflow/nfcore_faidx.ok.nf` | Nextflow (`-with-docker`) | 0 | 1/1 | `696683592f087bb9aa2bf8cb2b9bd85df1b482c809c3770d86c9450d629d6cfe` | MATCHED |
| `nextflow/nfcore_faidx.ok.nf` | imported (oxo-flow) | 0 | 1/1 | `696683592f087bb9aa2bf8cb2b9bd85df1b482c809c3770d86c9450d629d6cfe` | MATCHED |
| `snakemake/training_faidx.ok.smk` | Snakemake (`--use-singularity`) | 0 | 1/1 | `696683592f087bb9aa2bf8cb2b9bd85df1b482c809c3770d86c9450d629d6cfe` | MATCHED |
| `snakemake/training_faidx.ok.smk` | imported (oxo-flow) | 0 | 1/1 | `696683592f087bb9aa2bf8cb2b9bd85df1b482c809c3770d86c9450d629d6cfe` | MATCHED |

All four arms produced `.fai` index bytes with the identical SHA-256. This is
not a coincidence folded into the harness: the same four hashes were observed
in a manual pre-campaign reproduction of each arm, and the same tally/hashes
recur in the independent `--strict` run.

The six SKIPPED fixtures and their exact reasons (from the campaign log):

| fixture | reason |
| :--- | :--- |
| `nextflow/nfcore_module.ok.nf` | SOURCE `nextflow run ... -with-docker` exit 1, `ERROR ~ Script compilation failed` (the fixture's `fastqc: $( fastqc --version )` line is not valid DSL2 for Nextflow 26.10.6); imported arm also exited 1 (`workflow execution failed`). No arm produced a declared output. |
| `nextflow/rnaseq.ok.nf` | pre-flight: `no execution environment: missing conda` (imported environment declares conda; the host has no conda/mamba). |
| `nextflow/samtools_sort.ok.nf` | SOURCE exited 0 with no artifacts (no `data/*.bam` input is planted for this fixture); IMPORTED exited 1, `Error: failed to expand wildcard rules`. Neither arm produced the declared output. |
| `snakemake/expand.ok.smk` | pre-flight: `no execution environment: missing modules (HPC modules)`. |
| `snakemake/rnaseq.ok.smk` | pre-flight: `no execution environment: missing conda`. |
| `snakemake/rseqc_stat.ok.smk` | pre-flight: `no execution environment: missing conda`. |

No cell was DRIFTED; no executed fixture lacked artifacts; no arm was counted
as run when it had exited non-zero.

## 6. Wrapper changes (documented, not hidden)

`nextflow/nfcore_faidx.ok.nf` (all listed in the fixture header):

1. `tuple val(meta), path(fasta), path(fai)` + `val get_sizes` collapsed to
   `path fasta` — the importer has no tuple/val channel syntax, and a
   standalone module invocation supplies one FASTA.
2. Outputs reduced to `path "${fasta}.fai"` (optional fa/sizes/gzi are not
   produced without extra args; the `versions` topic is not a file artifact).
3. Dynamic container ternary materialized to its docker branch; the
   `moduleDir` conda reference materialized to its literal spec.
4. `process_single` resources materialized (time 4h); cpus 2 and memory 2 GB
   so the task fits the 3 GiB host (Nextflow refuses to schedule 6 GB against
   ~3.6 GB available — observed on the first attempt). cpus 2 also keeps the
   thread count explicit through the export round trip (the exporter omits
   `threads = 1`).
5. `get_sizes_command` dropped (no `get_sizes` argument in the wrapper).
6. A `workflow { }` call added (the upstream file is a module only).
7. `publishDir "GENOME_PATH", mode: 'copy'` added — nf-core publishes at the
   pipeline level; without it the declared output stays inside `work/` and a
   standalone run produces no artifact (observed: exit 0, no file).

`snakemake/training_faidx.ok.smk`:

1. `rule all` added as the concrete target (the upstream file's default target
   downloads data over the network); only the index rule is extracted.
2. The container value is written on its own line (valid Snakemake; the
   multi-line directive shape real workflows use, e.g. the NBIS
   workshop-reproducible-research final workflow). This layout does **not**
   by itself exercise the trailing-comma parser defect fixed in §8: this
   fixture has no trailing comma, and it imported on pre-fix main. It only
   places the value in the multi-line continuation form that the parser must
   handle.
3. The upstream `biocontainers/samtools:v1.3_cv3` image (1.26 GB) is replaced
   with the pinned `htslib_samtools:1.25` image (202 MB) for the ~15 GB-free
   qualification host. Both arms use this same image, so the comparison stays
   same-tool/same-input; the shell is unchanged.

## 7. Enumerated off-criterion observations

| # | observation | class | where |
| :--- | :--- | :--- | :--- |
| O1 | One tool family (`samtools faidx`) across both fixtures. Real tool, real container, but not a multi-tool corpus. | scope limit | fixtures |
| O2 | The Snakemake fixture does not run the upstream v1.3 image (§6); the substituted image is recorded by repo digest and SIF hash. | protocol deviation (documented) | §2, §6 |
| O3 | The Nextflow fixture is a derived module wrapper (§6), not the unmodified nf-core module — the module alone has no workflow/publishDir and its tuple channel is outside the importer subset. | wrapper | §6 |
| O4 | Both arms of both fixtures produce the same bytes partly because they run the **same image** (samtools 1.25). This does not measure cross-version tolerance of `samtools faidx`. | systematic | §5 |
| O5 | Nextflow/Snakemake/AI image materialization reaches the network during setup; the rules themselves do not. | below-threshold | harness setup |
| O6 | Wall time, peak RSS and disk I/O are not recorded by this instrument; it records per-arm command, exit state and artifact hashes. The #1304 triplet instrument records those separately. | instrument limit | runner |
| O7 | Engine builds are the host's **26.10.6 / 9.27.2**, not the PR-03 reference pins; they are recorded in §2. | protocol note | §2 |
| O8 | The Apptainer default cache (`~/.apptainer/cache`) fails on this host with an input/output error; the campaign sets `APPTAINER_CACHEDIR=/tmp/apptainer-cache`. This is a host workaround, not a product change. | host environment | §3 |
| O9 | A source comment containing the words `process <word>` is scanned as a phantom process by the Nextflow importer (observed while authoring the fixture; the fixture header was worded to avoid it). Not fixed in this campaign. | residual importer robustness gap | §8 |

## 8. Importer defects found while making real workflows work (fixed, with regression tests)

Two importer defects and one exporter defect were found in this work and
fixed in this PR. Their provenance differs, and the report states it exactly:

- **Snakemake multi-line directive values kept the trailing list separator.**
  Found while generalizing multi-line directive parsing for the real-world
  fixture: a value on the line after the directive and ending with a list
  comma (`container:` / `"docker://…",`) kept the comma and closing quote
  inside the spec, and the importer refused the workflow with E016
  (shell-unsafe `"`). `unquote` now strips a trailing comma before the
  quotes. **The committed `training_faidx.ok.smk` fixture does not contain a
  trailing comma and imported on pre-fix main**, so the fixture is not the
  proof; the defect is pinned by the synthetic regression
  `multiline_directive_values_drop_the_list_separator`. The same multi-line
  list shape (with its trailing separator) appears in real Snakefiles — the
  pre-existing `rseqc_stat.ok.smk` corpus entry documents the analogous
  multi-line `log:` artifact as O13 in the 2026-11-19 audit.
- **Nextflow script strings were copied verbatim.** The committed
  `nfcore_faidx.ok.nf` carries nf-core's `samtools \\` line continuations;
  before the fix the importer emitted `\\` into the shell (observed in the
  imported workflow while authoring the fixture), where bash treats it as a
  literal backslash argument instead of a continuation — the imported rule
  could not run. Added Groovy escape decoding for script bodies (`\\`,
  `\$`, `\n`, `\t`, `\r`, quotes). Regressions:
  `nextflow_string_escapes_decode_to_the_runtime_shell_text`,
  `line_continuations_are_decoded_in_the_emitted_shell`.
- **Exporter escaping (found by the #1398 review, fixed here).** The new
  decoder made the existing exporter lossy: it wrote the shell raw into a
  Groovy `"""` string, so a literal `\t` or `$HOME` in an `.oxoflow` shell
  would change on export→import (a real tab, or a Nextflow interpolation).
  The exporter now emits the inverse escapes for literal segments while
  leaving its own `${meta.id}`/`$task.*`/`$inN` interpolations raw.
  Regression: `shell_escapes_round_trip_through_the_exported_groovy_string`
  (proved failing before the fix).

Harness findings fixed in the same PR (not importer defects): a relative
`--bin` was resolved against each arm's staged cwd and failed ENOENT; the
wrapper now anchors it. Fixture-only corrections: the 6 GB task requirement
(host has ~3.6 GB) and the missing `publishDir` (output stayed in `work/`).

## 9. Honesty boundary

**What this is.** The first measured real-world execution-equivalence result
for the importer: two published-source workflows (one nf-core module, one
community Snakemake tutorial rule), each executed natively and as an imported
`.oxoflow` workflow on one host, over identical inputs and one pinned image
digest, with the produced artifact sets (including the declared outputs)
compared by SHA-256. Both matched; none drifted; six other supported fixtures
were honestly SKIPPED with reasons.

**What this is NOT, stated loudly.**

- **Not Gate 3 / not corpus-wide.** N=2 real workflows; 6 of the 8 supported
  fixtures did not execute on this host: four for a missing conda/modules
  backend, one because the installed Nextflow does not compile that source
  fixture, and one because the imported arm's wildcard expansion found no
  planted BAM. This is a floor, not coverage.
- **Not a multi-tool or multi-host result.** One tool (`samtools faidx`), one
  host, one image. No aligner, variant caller, or reference genome.
- **Not a cross-version claim.** Both arms run the same samtools image; the
  result says the imported workflow reproduces the source's declared bytes
  with the fixture's declared environment, not that tool versions agree.
- **Not a wall-time/RSS/IO measurement.** See O6.
- **"MATCHED" is not "equivalent in general".** It means these two workflows'
  produced artifact sets (which include the declared outputs) were
  byte-identical on this host with this image under the documented wrappers.
- **Single host, single author.** `tx-ubuntu` (x86_64 Linux); sources and truth
  were authored as part of this PR.

**What would move it forward**: conda/mamba or module environments on the
qualification host (unlocking the four pre-flight-skipped fixtures), a
second tool family, a second host (aarch64), and source fixtures that the
installed Nextflow compiles unmodified.

## 10. Raw-data lineage

| Artifact | Path |
| :--- | :--- |
| Harness (bash) | `scripts/import-execution-equivalence.sh` |
| Execution runner (python) | `scripts/import_execution_equivalence_run.py` |
| Truth table | `scripts/import_equivalence_expectations.py` (`status == "supported"`, `execution` blocks) |
| Fixtures | `tests/fixtures/import/nextflow/nfcore_faidx.ok.nf`, `tests/fixtures/import/snakemake/training_faidx.ok.smk` |
| Hermetic tests | `eval/scripts/test_import_execution_equivalence.py` (38 passed) |
| Baseline log (0 executed / 8 skipped) | `docs/superpowers/audits/data/import-execution-realworld-2026-11-27-baseline.log` |
| Campaign log | `docs/superpowers/audits/data/import-execution-realworld-2026-11-27-campaign.log` |
| Campaign raw records (per-arm command, exit, artifact SHA-256) | `docs/superpowers/audits/data/import-execution-realworld-2026-11-27-records.json` |
| Strict-run log (ends with `strict_rc=0`) | `docs/superpowers/audits/data/import-execution-realworld-2026-11-27-strict.log` |
| Strict-run raw records | `docs/superpowers/audits/data/import-execution-realworld-2026-11-27-strict-records.json` |
| Campaign identity (head, binary hash, image/SIF digests) | `docs/superpowers/audits/data/import-execution-realworld-2026-11-27-identity.txt` |

Reproduce: build the CLI, plant the two FASTA files, set
`PATH=$HOME/bin:$HOME/smk-venv/bin:$PATH`,
`APPTAINER_CACHEDIR=<writable cache>`, `NXF_OPTS=-Xmx1g`, then run §3. The
runner records the exact per-arm commands it executed.
