# Execution-equivalence — first measured triplet (#1304)

> **Status**: first measured **execution**-equivalence triplet for README bet 3
> ("Universal ecosystem transpiler", outstanding: *per-workflow execution
> equivalence on identical hosts/containers/resources*) and ROADMAP Phase 12.3.
> Where [`import-equivalence-2026-11-19.md`](import-equivalence-2026-11-19.md)
> measured the *declared* contract of the importer, this audit measures the
> *runtime* sibling: the SAME intent written natively in three engines
> (oxo-flow, Nextflow DSL2, Snakemake), run on one host over the same inputs
> and the same pinned container digest, must produce byte-identical **declared
> outputs**. PR references issue #1304 with `Refs`, never `Closes`.

## 1. What was measured

For each of three fixtures, the same intent is expressed three ways and each
expression is executed on the **same host, same inputs, same container
digest**:

- **oxo-flow** — `workflow.oxoflow` (TOML), run by the real CLI
  (`oxo-flow run workflow.oxoflow -j 2`), per-rule `[rules.environment]
  docker = "..."`.
- **Nextflow** — `main.nf` (DSL2), `container '...'`, `publishDir ...,
  mode: 'copy'`, `path` outputs, `Channel.fromList | PROC | collect | PROC`.
- **Snakemake** — `Snakefile`, per-rule `container:`, run with
  `--use-singularity --config container=<SIF|docker://...>`.

The comparison endpoint is a **SHA-256 of the file CONTENT** of every declared
output path, computed after the run in each arm's own staged directory,
compared cell by cell against the oxo-flow arm. It is deliberately **not** a
file-presence check: a file that exists with different bytes is DRIFTED.

Every arm also gets a **restart-into-cache** probe: the run is repeated and the
declared outputs are re-hashed, so a cache path that silently drops an output
or rewrites it would be caught.

The three fixtures are the execution counterparts of three fixtures that
already carry declared-artifact coverage; the selector list is **read from**
`scripts/import_equivalence_expectations.py` (`status == "supported"`), not
hand-copied, so a rename there fails the harness:

| fixture | intent | import-corpus reference |
| :--- | :--- | :--- |
| `rnaseq_chain` | trim → per-sample read count → fan-in summary (3-sample chain) | `nextflow/rnaseq.ok.nf`, `snakemake/rnaseq.ok.smk` |
| `qc_stats` | per-sample QC statistics, one artifact per sample (single stage) | `nextflow/nfcore_module.ok.nf` |
| `expand_filter` | expand over samples → per-sample score → fan-in threshold filter | `snakemake/expand.ok.smk` |

## 2. Pre-run manifest and sealed hashes (PR-03 §0)

Filled before the confirmatory run, in the PR-03 §0 field format. The manifest
is this section verbatim; its SHA-256 is the hash of the §1–§2 block as
committed at this PR's head.

| Field | Value |
| :--- | :--- |
| `campaign` | #1304 execution-equivalence (bet 3, Gate 3 direct measurement) |
| `engine_commit` | `e855fa6a71631237d5973041caf95664af23590d` (branch base = `origin/main` at branch time) |
| `engine_version` | `oxo-flow 0.32.0` (`target/debug/oxo-flow`, md5-equivalent build on `tx-ubuntu`) |
| `binary_sha256` | recomputed on `tx-ubuntu` for `target/debug/oxo-flow`; recorded in the raw records file |
| `schema_sha256` | `c9130eb49a97caff6b1482b2ee797938912aefecb4de57732ad13ab2d7e93f4a` (`docs/schema/oxoflow-v1.schema.json`) |
| `case_manifest_sha256` | `83a1b7e6cea37cef064ae1803c01ecf02cdb689b927efed23e688304a8e02a15` (18 files under `tests/fixtures/execution-equivalence/**`, concatenated path\0content in sorted order) |
| `analysis_script_sha256` | runner `2670c7f04071228ece8dfde7fe6d08c327169f4e5c4c8a35af3ffc0dfed1500a`; truth table `1362c93b66b601a140f16c87e9c7b93898815a6e2606fd8d9c3b83227c58e4f1`; wrapper `b4440c72191e60435822a2ab355e9bff66e463a21a3a0a3de727a28d1b5d43a5` |
| `model` | None — execution is deterministic, no provider call |
| `prompt_sha256` | None — no prompt |
| `toolchain` | oxo-flow 0.32.0; Nextflow **26.10.6 build 15032**; Snakemake **9.27.2** (see §6 O1 for the pin deviation); OpenJDK 21.0.11; container `python@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b` (docker id `3617028275c0`), materialized to `/tmp/py312.sif` for the Snakemake arm |
| `seed` | none — deterministic; each fixture's declared outputs re-hashed after a restart to confirm stability (all `restart_same=True`) |
| `hardware` | `tx-ubuntu`; Linux 6.8.0-137-generic, x86_64; Ubuntu 24.04.4 LTS; 4 CPUs; docker + singularity/apptainer |
| `dataset` | the 3 fixture inputs `data/s{1,2,3}.fastq`; per-file SHA-256 recorded in §8 |
| `budget` | per-arm wall-clock cap 1800 s (runner timeout); observed max 66.15 s (Nextflow, `rnaseq_chain`) |
| `exclusions` | no rule of any arm touches the network beyond container image materialization; no reference genome or read aligner is in scope (see §6 O2) |

**One pinned container for every arm of every fixture** (PR-03 C3: identical
across arms). `python:3.12-slim` carries `/bin/bash` (Nextflow's container shim
needs it) and the POSIX tools the fixtures use. A differing digest would be a
protocol deviation, not an equivalence observation.

## 3. Exact commands

```bash
# oxo-flow arm (per fixture)
target/debug/oxo-flow run workflow.oxoflow -j 2

# Nextflow arm (per fixture)
nextflow run main.nf -work-dir <work> -ansi-log false -with-docker

# Snakemake arm (per fixture)
snakemake --use-singularity --config container=/tmp/py312.sif \
  -j 2 --singularity-args '-B /tmp'
```

Harness (report-only by default, always exits 0; `--strict` exits non-zero only
when a fixture **DRIFTED** — a SKIPPED arm alone never fails `--strict`):

```bash
# locally (engines absent -> every arm SKIPPED, exit 0)
scripts/execution-equivalence.sh --bin ./target/debug/oxo-flow
scripts/execution-equivalence.sh --strict --bin ./target/debug/oxo-flow

# on tx-ubuntu (engines present), env:
#   EQ_NEXTFLOW_BIN, EQ_SNAKEMAKE_BIN, EQ_SNAKEMAKE_CONTAINER=/tmp/py312.sif
#   EQ_SNAKEMAKE_EXTRA="--singularity-args '-B /tmp'"
scripts/execution-equivalence.sh         --bin target/debug/oxo-flow
scripts/execution-equivalence.sh --strict --bin target/debug/oxo-flow
```

The wrapper captures the runner's status as `rc=0; python3 ... || rc=$?` so
`set -euo pipefail` cannot abort before the `--strict` check.

## 4. Pre-registered acceptance criterion

**Registered before the tx-ubuntu run** (the truth table and the three
fixtures were authored first; the harness was then run against them):

> A fixture is **MATCHED** iff, on one host with one pinned container digest:
> (a) all three arms exit 0; (b) every declared output path exists in every
> arm; and (c) the SHA-256 of the content of each declared output is identical
> across all three arms. A fixture where any arm did not run (missing binary,
> non-zero exit, absent output) is **SKIPPED**, never MATCHED. A fixture where
> all arms ran but a declared output's content or set differs is **DRIFTED**,
> with the differing path named. Anything outside (a)–(c) — e.g. the container
> **pin deviation**, wall-clock spread, or engine-version spread — is an
> off-criterion observation in §6, not folded into the verdict.

The harness cannot run vacuously: it resolves a real `oxo-flow` binary and
fails hard if none is found, and it refuses to call a fixture MATCHED unless
all three arms actually ran.

## 5. Truth table and per-fixture results

Headline on `tx-ubuntu`: **MATCHED 3 / DRIFTED 0 / SKIPPED 0** over 3
fixtures, in both the report-only and the `--strict` invocation
(`strict_rc=0`).

| fixture | arm | exit | wall s | outs | restart_exit | restart_same | verdict |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| `rnaseq_chain` | oxo-flow | 0 | 3.16 | 7/7 | 0 | True | MATCHED |
| `rnaseq_chain` | nextflow | 0 | 66.15 | 7/7 | 0 | True | MATCHED |
| `rnaseq_chain` | snakemake | 0 | 3.72 | 7/7 | 0 | True | MATCHED |
| `qc_stats` | oxo-flow | 0 | 1.93 | 3/3 | 0 | True | MATCHED |
| `qc_stats` | nextflow | 0 | 26.10 | 3/3 | 0 | True | MATCHED |
| `qc_stats` | snakemake | 0 | 3.02 | 3/3 | 0 | True | MATCHED |
| `expand_filter` | oxo-flow | 0 | 2.51 | 4/4 | 0 | True | MATCHED |
| `expand_filter` | nextflow | 0 | 28.57 | 4/4 | 0 | True | MATCHED |
| `expand_filter` | snakemake | 0 | 3.51 | 4/4 | 0 | True | MATCHED |

**Per-cell WHY on every mismatch: none — no cell DRIFTED.**

The declared-output SHA-256 sets (identical in all three arms of each fixture;
the oxo-flow arm's values are shown as the reference):

`rnaseq_chain` (7 declared outputs):

| path | SHA-256 |
| :--- | :--- |
| `trimmed/s1.fastq` | `c986fd4bf0568796217ecb37f5ef0b649ef516edfa9678f3bbb3409a2f3bba5f` |
| `trimmed/s2.fastq` | `7a40af7202607744cad8d52e848b5bf77cc16092ac9ae8140722cd0c2fb9af7b` |
| `trimmed/s3.fastq` | `22766b9eb42e34d7865bfbc6b061c6a882711534f5031371c94442b1d0e7033b` |
| `counts/s1.tsv` | `f0e6554702e0a1aeb6a18c4669e6bc68f7b0bb4f3193de580c3b2473ae2a77f9` |
| `counts/s2.tsv` | `7c68e499d144920a57f8b90dc00924494f25e5029c320a2f17ac7a96dbd76609` |
| `counts/s3.tsv` | `8f7b7f3fe3324c6fb50584fd48d8471998f76cd174528f6321c0041ee1f9bd9a` |
| `report/summary.tsv` | `f1fb3344bf0470e5e7a06a9b72e57dcc54bd84edf23638bf4c3db3593d29796d` |

`qc_stats` (3): `qc/s1.stats.txt` `7600bc…a0edbf`, `qc/s2.stats.txt`
`bc3bba…0da0d605`, `qc/s3.stats.txt` `7b8fa6…3bb7109f`.

`expand_filter` (4): `quant/s1.sf` `a4ccc8…5b5b3692d`, `quant/s2.sf`
`f4b5f0…c3222504b`, `quant/s3.sf` `1dd2ae…8d2113554`, `results/kept.tsv`
`35f8aaedc4c11823406a86e16bab08ea7ce5595e871b6f712f58c206c17b53d2`.

**Executed vs skipped across the whole campaign:**

| cell | state |
| :--- | :--- |
| all 9 arm-runs on `tx-ubuntu` (3 fixtures × 3 engines) | **executed** |
| all 9 restart-into-cache probes | **executed** |

No cell was SKIPPED on the measurement host. (On a host without the Nextflow or
Snakemake binaries — e.g. this macOS worktree — all three fixtures report
`SKIPPED`, MATCHED=0, exit 0; that is the honesty path, not a pass.)

## 6. Enumerated off-criterion observations

Real, intentional differences that the (a)–(c) criterion does not score. They
are recorded so "MATCHED" is not read as "the three engines are the same tool".

| # | observation | class | where |
| :--- | :--- | :--- | :--- |
| O1 | **Engine-version pin deviation.** PR-03 C3 names Nextflow **25.10.4** / Snakemake **9.19.0** as the reference pins; the measurement host carries **26.10.6 / 9.27.2**. The digests and the container are pinned, the engine builds are not. | protocol deviation (must be closed before a confirmatory claim) | this §2 `toolchain`; PR-03 C3 |
| O2 | Container image materialization (`docker pull` / `singularity pull`) reaches the network; the rules themselves do not. | below-threshold | harness setup |
| O3 | Nextflow arm wall-clock is ~8–20× the other two (JVM start, image shim, `.nextflow` bookkeeping); wall-clock is **not** an equivalence endpoint. | systematic | §5 wall-s |
| O4 | The Snakemake arm needs `--singularity-args '-B /tmp'` because the SIF lives outside the workdir; encoded via `EQ_SNAKEMAKE_EXTRA`, not in the fixture. | env knob, not a fixture fact | `scripts/execution-equivalence_run.py` |
| O5 | Fixtures use POSIX `awk`/`grep`/`cat`, not bioinformatics tools; no aligner, no reference genome. The intent is identical, the *tools* are the shell. | **scope limit** (see §7) | fixtures |
| O6 | Nextflow DSL2 requires `$` escaping (`\$(...)`, `\$0`) and Snakemake requires doubled awk braces (`{{...}}`) — a notation difference between the *source languages*, erased before the shell runs. | below-threshold | `main.nf`, `Snakefile` |

## 7. Honesty boundary

**What this is.** A first *execution*-equivalence triplet: 3 intents, each
written natively in oxo-flow, Nextflow and Snakemake, each **actually run** on
one host (`tx-ubuntu`) over shared inputs and one pinned container digest, with
declared-output **content** compared by SHA-256 and a restart probe. It answers
"given the same intent/host/inputs/container, do the same declared outputs come
out byte-for-byte identical across the three engines?" — for these three.

**What this is NOT, stated loudly.**

- **Not a Gate-3 pass.** This is direct execution-equivalence measurement for
  **3** fixtures, not the corpus-wide, real-workflow, multi-host evidence Gate
  3 needs. It does not close #1304 and does not claim Gate 3 passed. N=3.
- **Not real bioinformatics.** No aligner, variant caller, QC tool, or reference
  genome is invoked (O5). The three intents are shaped so a POSIX-only container
  suffices *by design*, to isolate engine behaviour from tool behaviour. A
  real-tool triplet (STAR, samtools, …) with per-tool reference locks is not in
  this PR.
- **Not a pinned-engine claim.** The measured Nextflow/Snakemake builds differ
  from the PR-03 C3 reference pins (O1). A confirmatory claim must re-run on the
  frozen builds or file a pre-data amendment.
- **"MATCHED" is not "equivalent in general".** It means these three intents
  produced identical declared-output bytes on this host with this container. It
  says nothing about other constructs, larger fan-outs, cluster execution, or
  failure-path behaviour.
- **Single host, single author.** One host (x86_64 Linux); the truth table and
  the three-way sources are single-author, reviewed only as part of this PR.
- **No engine change.** This PR adds measurement only; it fixes no observation
  in §6 and changes neither oxo-flow nor the importer.

**What would move it forward**: the pinned engine builds (O1); real-tool
fixtures with reference locks (O5); a second host (aarch64, per PR-03 C4); a
corpus drawn from real-world non-fixture workflows; and a second independent
authoring of the three-way sources.

## 8. Raw-data lineage

| Artifact | Path |
| :--- | :--- |
| Harness (bash) | `scripts/execution-equivalence.sh` |
| Runner (python) | `scripts/execution_equivalence_run.py` |
| Truth table (author-written) | `scripts/execution_equivalence_expectations.py` |
| Hermetic tests | `eval/scripts/test_execution_equivalence.py` |
| Fixtures | `tests/fixtures/execution-equivalence/{rnaseq_chain,qc_stats,expand_filter}/{workflow.oxoflow,main.nf,Snakefile,data/s{1,2,3}.fastq}` |
| Raw records JSON | `execution-equivalence-records.json` written by the runner into its scratch dir (reproduce with §3) |

Fixture source-file SHA-256 (identity of what was executed):

| fixture | `workflow.oxoflow` | `main.nf` | `Snakefile` |
| :--- | :--- | :--- | :--- |
| `rnaseq_chain` | `307ef24ee1897c0626e6524c12c9313a4055e345b068a7cbee42be5b7a30a19c` | `f693210231ebc499ad078aca9353007b9ea3263850e6ff64e7238bf8110860a7` | `8485098e2f353570cc1f2f1cc2a94f086b724d179e4b6f151b5403a9484e28d0` |
| `qc_stats` | `bd1d07341d92be55289e5ee10a3ece278441a3c741107ae2c4fbc28039376db8` | `d3a283527f5ef40c5312c10f2befa45385568f3fc2fae08422ac4aa46e331bde` | `9b3f7d13381888e99b6fc2c63fd8c4afb059407a83a896ccf9cdb5ced4732e3c` |
| `expand_filter` | `2967ffc5ed65218f22757d825b93ef73f33694cb0412b5e8edff28b15ddcb687` | `54c32c71c9a39f102784202950641506df4f15ed6f1e4f4b2ea146b3ebe05d97` | `bdcf12ea22f0f200682a3953f519b2a5e5517a311f98e4551428603c15828b4d` |

Input `data/*.fastq` SHA-256 (identical across all three fixtures — the same
three files are copied into every arm):

| file | SHA-256 |
| :--- | :--- |
| `data/s1.fastq` | `c986fd4bf0568796217ecb37f5ef0b649ef516edfa9678f3bbb3409a2f3bba5f` |
| `data/s2.fastq` | `7a40af7202607744cad8d52e848b5bf77cc16092ac9ae8140722cd0c2fb9af7b` |
| `data/s3.fastq` | `22766b9eb42e34d7865bfbc6b061c6a882711534f5031371c94442b1d0e7033b` |

Measurement host facts (`tx-ubuntu`, captured immediately around the run):
Nextflow `26.10.6 build 15032`; Snakemake `9.27.2`; OpenJDK `21.0.11`;
`Linux 6.8.0-137-generic x86_64`; Ubuntu `24.04.4 LTS`; 4 CPUs; oxo-flow
`0.32.0`; container `python@sha256:5e2bdb…a4fa5b` (docker id `3617028275c0`).
