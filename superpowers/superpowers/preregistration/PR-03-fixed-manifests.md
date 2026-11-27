# PR-03 — Fixed Model, Prompt, Tool, Seed, Hardware, Dataset, and Budget Manifests

| Freeze record | Value |
| :--- | :--- |
| Artifact | PR-03 (Task 4.0 required artifact 3 of 6) |
| Version | 1.0.4 |
| Freeze status | FROZEN |
| Freeze date | 2026-11-18 (original freeze; amendments 1–5 on main; amendment 6 on 2026-11-28 for #1166) |
| Amendment | 1 (2026-11-18): review fixes D1–D5 and clarifications; 2–5 (2026-11-27/28): #1163 governed campaign (`τ_FA = 0.05`, held-out corpus, status/erratum/PR-04 caveat); 6 (2026-11-28): #1166 campaign parameters (`τ_ARCH = 0.80`, `n_min = 3`) — see [`AMENDMENTS.md`](AMENDMENTS.md) entries 1–10 |
| Content binding | SHA-256 recorded in [`MANIFEST.sha256`](MANIFEST.sha256) |
| Campaigns covered | #1163, #1164, #1165, #1166 |
| Change control | Post-freeze edits only through [`AMENDMENTS.md`](AMENDMENTS.md) |

**Frozen-before-data statement.** Every value marked FIXED below is
binding from this freeze. A TBD carries an owner and the deadline "before
data collection"; it must be filled by a pre-data amendment that records
the value and its evidence, and it may not be filled after any confirmatory
outcome is seen.

## 0. Pre-run manifest requirement

Before the first confirmatory run, the campaign owner commits a
machine-readable pre-run manifest (JSON) that repeats the values below and
adds the per-case records. The manifest's SHA-256 is sealed in PR-04's
custody record. The manifest must contain at least:

| Field | Meaning |
| :--- | :--- |
| `campaign` | Issue number and campaign name |
| `engine_commit` | `git rev-parse HEAD` of the oxo-flow tree used |
| `engine_version` | `oxo-flow --version` output |
| `binary_sha256` | SHA-256 of the executed CLI binary |
| `schema_sha256` | SHA-256 of `docs/schema/oxoflow-v1.schema.json` |
| `case_manifest_sha256` | SHA-256 of the sealed case list and expected outcomes |
| `analysis_script_sha256` | SHA-256 of every script that computes a primary endpoint |
| `model` | Provider, endpoint, exact model identifier, temperature, ceilings |
| `prompt_sha256` | SHA-256 of each prompt template used, including the recovery prompt |
| `toolchain` | Reference engine versions, container digests, stub-tool identities |
| `seed` | Every seed (run-order, bootstrap, repeats), or `none` with reason |
| `hardware` | Host identity, architecture, OS kernel, CPU, RAM, storage |
| `dataset` | Inputs/references by content hash, source, and custody reference |
| `budget` | Token, cost, wall-clock, and attempt ceilings |
| `exclusions` | Pre-run exclusions and sealed replacements |

A missing field blocks the run. A value that differs from this document
without an amendment makes the campaign non-compliant.

## C1 — #1163 truth-set manifest

| Dimension | Fixed value | Status / owner |
| :--- | :--- | :--- |
| Model | None — the checker is deterministic | FIXED |
| Prompt | None — no generation call | FIXED |
| Tool | `oxo-flow` CLI built from the frozen engine commit; `validate --json` is the verdict surface, with `lint --json` / `dry-run --json` recorded for context only and never substituted for `validate` | FIXED |
| Seed | None — deterministic; a repeated run must be byte-identical, and any difference is an infrastructure failure that voids the case | FIXED |
| Hardware | Single arm: the campaign lead's macOS arm64 workstation, host identity (`hostname`, `uname -srm`, CPU model, RAM), rustc version, and binary SHA-256 recorded in the pre-run manifest and the report. No cross-platform contrast is claimed | FIXED 2026-11-27 (amendment 2) |
| Dataset | Truth set: 83 expected-reject cases + 24 acceptance controls authored post-freeze, plus 9 pre-existing gallery controls disclosed as exposure (expected-accept, outside the FA denominator); at least 73 expected-reject cases; coverage spans `TYPE-001`–`TYPE-010`, every non-`generic` format, a `transform.*` generated edge, and an `[[include]]`-contributed edge; the 54-case pilot set is excluded as exposure. Expected verdicts are sealed by the pre-run manifest's canonical `cases` array hash; case bodies are pinned by the stamped commit and checked byte-for-byte by `make truthset-stamp`. | FIXED 2026-11-27 (amendment 2); custody wording corrected 2026-11-28 (amendment 4) |
| Budget | Static-only: no execution, no provider calls, no network. Per-case subprocess timeout 60 s (enforced by the sealed runner); total declared ceiling 7200 s (the primary campaign observed ~20 s; the sealed runner did not enforce the total as a hard timer — disclosed in the campaign erratum; the post-hoc supplement runner enforces both and records elapsed time) | FIXED 2026-11-27 (amendment 2); enforcement wording corrected 2026-11-28 (amendment 4) |

The pre-run manifest is
`docs/superpowers/preregistration/prerun/C1-governed-2026-11-27.json`
(plus its `.sha256` companion), committed before the first confirmatory
run. It carries the §0 fields and the full per-case expected-verdict
records; `case_manifest_sha256` is the canonical hash of its `cases`
array, and the campaign runner reads the `cases` array as its EXPECT
source so the sealed list and the executed list cannot drift.

## C2 — #1164 repair-campaign manifest

| Dimension | Fixed value | Status / owner |
| :--- | :--- | :--- |
| Model | Provider family: GLM served through the school proxy (OpenAI-compatible route, as in the 2026-11-18 pilot). Exact model identifier was **not recorded** in the pilot and must be recorded for the confirmatory run | Exact identifier TBD — owner: campaign lead; deadline: before data collection |
| Prompt | The engine's in-repo recovery prompt, identified by source path and SHA-256, plus the `--ai-recover` orchestration policy at the frozen engine commit. No hand-edited prompt is permitted mid-campaign | Source path FIXED by the engine commit; hash recorded in the pre-run manifest |
| Tool | `oxo-flow` binary at the frozen engine commit; `eval/frontier/fault_injection.py` at its frozen commit; the harness `stubbin/` tools (nine: `samtools`, `gatk`, `bcftools`, `bwa`, `bwa-mem2`, `vep`, `tabix`, `featureCounts`, `simtool`) are deterministic stub scripts created by the harness | FIXED for the harness; binary commit recorded pre-run |
| Seed | No provider seed: the Anthropic Messages API (and compatible endpoints) has no seed field, as documented in `eval/frontier/run_eval.py`. Set `[ai] temperature = 0` where the endpoint honors it; measure within-case agreement over R repeats | Temperature FIXED at 0 requested; R and the repeat policy TBD — owner: campaign lead; deadline: before data collection |
| Hardware | A single named measurement host per confirmatory campaign. The pilot ran on macOS and `tx-ubuntu`; the confirmatory host(s) and whether both are used must be declared. Host identity, kernel, CPU, RAM recorded per case | TBD — owner: campaign lead + infrastructure owner; deadline: before data collection |
| Dataset | Seeded fault cases with declared runtime class, expected diagnostic code, and executable reference repair. The confirmatory set must be held out from the 2026-11-18 pilot (PR-01 §C2). Stub-tool file-creation behavior is part of the fixed tool manifest | Case set TBD by hash — owner: campaign lead; deadline: before data collection |
| Budget | `--ai-max-retries 1`; maximum provider calls = 2 × number of cases (one diagnosis, at most one corrective); `OXO_FLOW_AI_TIMEOUT_SECS=300`; `OXO_FLOW_AI_MAX_TOKENS=16384` | Ceilings FIXED; total dollar cap and wall-clock cap TBD — owner: campaign lead; deadline: before data collection |

Provider credentials are read from the environment and are never recorded
in a manifest, log, or report. Only the endpoint identity and model
identifier are recorded.

## C3 — #1165 equivalence-corpus manifest

| Dimension | Fixed value | Status / owner |
| :--- | :--- | :--- |
| Model | None — import and execution are deterministic | FIXED |
| Prompt | None | FIXED |
| Tool | Three engines: `oxo-flow` at the frozen engine commit, Nextflow, and Snakemake. The repository reference numbers in `benches/comparative/README.md` record **Nextflow 25.10.4** and **Snakemake 9.19.0**; those are the reference pins. The campaign must additionally record the exact build and container digest used per case | Reference pins 25.10.4 / 9.19.0 FIXED; per-case build/digest pin TBD — owner: campaign lead; deadline: before data collection |
| Seed | Run-order randomization seed recorded before the first run. Each case's arm order is derived deterministically from that seed | Seed value TBD — owner: campaign lead; deadline: before data collection |
| Hardware | One host for all arms of a case; same OS, CPU, RAM, storage, scheduler state, and filesystem. Host identity and versions recorded per case | TBD — owner: campaign lead + infrastructure owner; deadline: before data collection |
| Dataset | Frozen corpus of Nextflow-origin and Snakemake-origin workflows inside the documented supported subset, each with an independently reviewed `.oxoflow` counterpart, inputs, references, container digests or environment locks, and expected declared outputs. Corpus bodies and expected artifact hashes sealed before the run | Corpus composition and size TBD — owner: campaign lead; deadline: before data collection |
| Budget | Per-arm wall-clock and total campaign CPU-hour cap; repeats per arm R | TBD — owner: campaign lead; deadline: before data collection |

The equivalence normalizer (PR-01 §C3 secondary endpoint) is part of the
tool manifest: its source and hash are sealed with the analysis script
before data collection. Container digests are per case and identical across
arms; a differing digest is a protocol deviation, not an equivalence
observation.

## C4 — #1166 reproduction manifest

| Dimension | Fixed value | Status / owner |
| :--- | :--- | :--- |
| Model | None | FIXED |
| Prompt | None | FIXED |
| Tool | `oxo-flow` at the frozen engine commit, run with `--provenance`; comparison via `oxo-flow provenance verify --json` on each arm's checkpoint | FIXED |
| Seed | None at the engine level; any tool-level non-determinism is measured, not seeded away | FIXED (no seed) |
| Hardware | Two architectures: x86_64 and aarch64. The Linux x86_64 reference host is `tx-ubuntu` (designated Linux validation host, native x86_64 docker). The aarch64 host is the local Apple Silicon macOS host running Docker Desktop's **native linux/arm64 VM** (no qemu). Both container legs execute the same multi-arch manifest-list digest; host OS/kernel/CPU class are recorded per arm | FROZEN 2026-11-28 (amendment 6) |
| Dataset | Two frozen workflows (`scripts/cross-arch-provenance-governed-a.oxoflow`, `-b.oxoflow`) with 24 primary declared outputs (12 each) in 8 classes of 3, plus one excluded negative control. Corpus manifest: `scripts/cross-arch-provenance-governed-corpus.json`; image pinned by multi-arch manifest-list digest `sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b` | FROZEN 2026-11-28 (amendment 6) |
| Budget | Two hosts × two workflows; each workflow is minutes of CPU and well under 1 GiB of storage; no cloud or paid calls | FROZEN 2026-11-28 (amendment 6) |

The container digest (or the conda-family environment-lock identity) is a
first-class manifest value, not metadata: if it differs between arms, the
run measures tooling differences and cannot support the architecture claim
(PR-02 §C4). A macOS-arm64 versus Linux-x86_64 comparison is recorded as a
declared confounded fallback only.

## 5. Manifest change rules

1. A FIXED value changes only by amendment, with the reason and the
   evidence that no confirmatory outcome was examined.
2. A TBD value is filled by a pre-data amendment. The amendment must name
   the value, the owner, the date, and the evidence; it is not enough to
   edit this table.
3. Any manifest change after a confirmatory run starts makes the affected
   campaign **exploratory** for the changed dimension. It cannot be
   presented as pre-registered.
4. The pre-run manifest snapshot is archived with the raw results and
   referenced from the campaign report, so a later reanalysis can compare
   the executed configuration to the frozen configuration.
