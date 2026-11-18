# Fixture Contract Inventory — Audit Wave 4 (issue #1168)

**Status:** executed 2026-11-18 against `main` @ `10d91e34c` (worktree
`/tmp/fix1168`, branch `fix/1168-fixture-contract-inventory`).

**Question this artifact answers:** for every fixture family in the
repository, *what contract is the fixture meant to pin, and can its expected
value be traced to that contract* — or does it only encode what the engine
happened to do when the fixture was authored?

The distinction is load-bearing. A regression suite that encodes stale
semantics is worse than no suite: it actively resists the correct fix
(AGENTS.md Engineering Invariants: "Don't mirror semantics — share them").
No expectation in this inventory is accepted on the grounds that the current
binary produces it.

## 1. Verdict vocabulary

| Verdict | Meaning |
| :--- | :--- |
| **traceable** | Every expectation was traced to a named contract (docs, schema, manifest, or the fixture's own declared generator) and verified in this audit. A row marked traceable may still carry a documented boundary in the trace column. |
| **annotated** | The fixture family is valid within a stated limit (measurement harness with no independent oracle, inline test corpus that duplicates rather than references its source); there is no open traceability failure. |
| **suspect** | At least one expected value does **not** trace to a contract — it traces only to current engine output or is under-specified such that it passes vacuously. Either corrected in this audit (then counted as traceable) or left open with the derivation and remediation recorded. |

Corrected rows are counted as traceable (post-audit state); the remediation
column records that they were suspect at audit time.

## 2. Summary counts

| Verdict | Families |
| :--- | :---: |
| traceable | 17 |
| annotated | 2 |
| suspect (open) | 1 |
| **total** | **20** |

Suspect at audit time: 4 families. Three were corrected (evaluation intents
README counts, interpretation seed `s04`, comparative Snakefile line count);
one is left open and justified (`eval/gold/workflow.csv` `wf-004`, because
correcting it requires changing the checker's graph surface *and* a human
re-attestation of the evidence ledger — see §5.1).

## 3. Inventory

### 3.1 Evaluation fixtures (`eval/`)

| Family | Contract it pins | How the expectation traces | Verdict | Remediation / evidence |
| :--- | :--- | :--- | :---: | :--- |
| `eval/frontier/intents.json` (36 intents) | The frontier benchmark's intent set: unique ids, tiers (`easy`/`medium`/`hard`), domains, and per-intent `requires` tool list; `vague` intents are reported separately (`eval/frontier/README.md`, `run_eval.py`). | Fixture is authoritative and loaded wholesale (`run_eval.py` `--intents` default). Counted: 36 intents, 12 domains, 5 easy / 14 medium / 17 hard, 3 vague. README claimed 29 intents / 10 domains — untraceable to the fixture. | traceable | **Was suspect; corrected.** README now states 36 / 12 / 5-14-17 and is gated by `eval/frontier/test_intents_contract.py` (fails on the old text; see §4.2). `PILOT_FINDINGS.md`'s "29 intents" is a historical campaign record and was deliberately left unchanged. |
| `eval/frontier/fault_injection.py` `FAULT_CASES` (10 cases) | Seeded fault classes map to documented diagnostic contracts: BioTypes `TYPE-002/003/004/006`, scientific-preflight `SCI-VQSR-COHORT` / `SCI-FEATURECOUNTS-STRAND` / `SCI-MUTECT2-TUMOR-ONLY`, infrastructure runtime codes (137 OOM, SIGXCPU, ENOSPC) (`docs/guide/src/reference/diagnostics-engine.md`, `docs/guide/src/commands/dry-run.md`). | Every code the harness expects exists in the shipped diagnostics/checker code and docs; the executable-repair arm is gated on each case's reference repair (dry mode never fabricates stages). Ran the harness in dry mode (`--skip-repair`): static detection 7/10 exactly as documented (4 BioTypes + 3 SCI), the 3 infrastructure cases correctly report static=false with runtime attribution. | traceable | Boundary noted: the harness is a manual campaign tool, not part of `make eval-tests`; the run above is the evidence for this audit, not a CI claim. |
| `eval/frontier/statistical_report.py` | Matched three-arm ablation data required; synthetic mode is non-inferential; missing arms fail closed (EV-02, `eval/frontier/README.md`). | `eval/frontier/test_statistical_report.py` (part of `make eval-tests`) asserts the fail-closed and label behavior; tests pass. | traceable | — |
| `eval/gold/tool.csv` (149 rows) | `eval/schema.md` §1 columns; versions and negative-existence claims derive from the embedded knowledge base; every reviewed row is bound to `eval/review/evidence.jsonl` by canonical field hash. | `check_gold.py` (the `eval-lint` gate) verifies 10 negative names against the KB, 133 expected tools, 52 version pins against the KB, and 133 provenance rows — 0 hard findings, 0 warnings. Probed the generator: `gen_tool_csv.py` emits the **pre-review draft** (151 rows, empty review columns, versions filled), so byte-equality with the committed CSV is *not* the contract for reviewed rows; the contract is the reviewed field set + hash binding, which the gate checks. | traceable | Boundary: knowledge refresh regenerates the draft; reviewed rows are snapshot-scoped and must be re-synced in the same change (the `version-drift` finding already enforces this). |
| `eval/gold/rule.csv` (70 rows) | `eval/schema.md` §2: expected tool / key params / I/O / resource range derive from the `reference_workflow` + `reference_rule`. | All 40 in-repo reference rules exist and every expected tool/param appears in the referenced rule text (7 apparent misses are `transform` `map` strings, which the checker must read through — gallery 10). I/O differences: `rule-012` input is declared via `expand_inputs`, `rule-026` output is the documented transform chunk path `.oxo-flow/chunks/{by}/{value}.{ext}` (`docs/guide/src/gallery/transform-operator.md`). | traceable | Boundary: `check_gold.py` validates workflow DAG edges but does not re-derive rule-level params/I/O from the reference; the evidence ledger binds the reviewed fields, not the derivation. An automated rule↔reference check would close this (not added: it would relitigate human-approved rows). |
| `eval/gold/workflow.csv` (38 rows) | `eval/schema.md` §3: expected steps/tools/DAG edges/outputs derive from `reference_file`; declared edges are checked against the engine's dependency graph. | 16 in-repo rows: all expected steps exist, all expected outputs appear in the reference files, 25/38 prompts name an expected tool (matches `eval/README.md`). 16 local rows pass the `dag-edge` check. **`wf-004` declares `expected_dag_edges: []`** while `04_scatter_gather.oxoflow` declares `scatter = { … gather = "gather_gvcf" }`; the default `graph -f dot` surface omits that edge (`--expanded` shows 5 edges into `gather_gvcf`), so the empty list passes vacuously and cannot be traced to the reference semantics. | **suspect** | **Open, annotated** (§5.1). Not modified: the fix requires switching `check_gold.reference_graph` to `--expanded` plus a new evidence-ledger record, i.e. human re-attestation of a gold field (issue #172 remains a standing marker). |
| `eval/review/evidence.jsonl` (257 records) | Every reviewed gold row is cryptographically bound to a verdict, reviewer, and canonical `fields_hash` over its gold columns (`eval/schema.md`, `eval/review/METHODOLOGY.md`). | `verify_gold.audit_gold_set` (run inside `eval-lint` and `make eval-verify`) reports 257 rows audited, 0 findings, and automated reviewer placeholders are rejected. | traceable | — |

### 3.2 Interpretation seeds (`eval/interpretation/`)

| Family | Contract it pins | How the expectation traces | Verdict | Remediation / evidence |
| :--- | :--- | :--- | :---: | :--- |
| `eval/interpretation/seeds/*` (12 seeds) | Each `manifest.json`'s `run_expect_exit` is the exit status the *planted fault* must produce under the documented run contracts: fail-fast on a missing required source input (`docs/guide/src/commands/run.md`), wildcard safe-default charset (`docs/guide/src/reference/wildcards.md`), declared resource-group capacity, and output validation. `surface` selects `run` vs `dry-run`. | Ran every manifest's documented command with `OXO_FLOW_AI_PROVIDER=disabled` and a pinned `PATH`: 11/12 matched; **`s04-silent-failure` expected 0 and got 1** — its declared input was absent, so the fail-closed readiness gate aborted before the silent pipeline could run. That is the demonstrated stale-semantics fixture (§4.1). `s02-missing-binary` depends on a host precondition (tool absent); it now declares `requires_absent: ["bwa-mem2"]` and the gate enforces it. | traceable | **Was suspect; corrected** (`s04` workflow + manifest re-baselined; `s02` precondition declared). New provider-free gate: `crates/oxo-flow-cli/tests/interpretation_seeds.rs` runs in `cargo test --workspace` and fails on the pre-fix seed (proven, §4.1). |

### 3.3 Benchmark fixtures

| Family | Contract it pins | How the expectation traces | Verdict | Remediation / evidence |
| :--- | :--- | :--- | :---: | :--- |
| `eval/benchmarks/cohort_driver.py` | Renders a static three-rule workflow fanned out over `[[values]]`; plan length comes from the real `dry-run --json` plan, never inferred (EV-04). | Pipeline is generated from its own declared cohort; `eval/benchmarks/test_cohort_driver.py` asserts the measured-plan contract and rejects inferred counts. Tests pass. | traceable | — |
| `eval/benchmarks/hardware_scaling.py` | Projections must be labeled `MODEL PROJECTION` / `NOT MEASUREMENT` and only generated with `--allow-simulation`; measured data requires per-run telemetry (EV-03). | `eval/benchmarks/test_hardware_scaling.py` asserts the labels and the fail-closed measured path; tests pass. | traceable | — |
| `eval/benchmarks/sus_survey_analysis.py` + `ux_eval_protocol.md` | Synthetic SUS values are test vectors for arithmetic/reporting code, never pilot measurements; the protocol is a pre-registration draft (EV-06). | `eval/benchmarks/test_sus_survey_analysis.py` asserts `SIMULATION ONLY`, no p-values/effect claims, and a renderer that rejects inferential reports; the protocol states no participant data exists. Tests pass. | traceable | — |
| `benches/macro/suite.py` | Generates the N-rule/parallel/scatter workflows the lifecycle/scaling/reliability benches time; the only pass expectation is the engine exit code, plus duplicate-rule detection. | Generated workflows are constructed from declared rule counts (self-describing); no independent oracle exists for wall times by design. Duplicate-rule detection asserts `validate` rejects a duplicated rule name — the documented duplicate-name contract. | annotated | Measurement harness, not an expectation-bearing fixture; no test is claimed to validate timing results. |
| `benches/comparative/` (`hello.nf`, `echo_step.nf`, `Snakefile`, `run_comparison.sh`) | All three definitions produce the same sequential chain `step_0 -> … -> step_{N-1}` with `echo {i} > step_{i}_output.txt`; Nextflow is pre-expanded to 1000 aliases; Snakemake depth ceiling ~484 (README). | `hello.nf` is 2041 lines with `include step_0..step_999` and `if (count > k)` guards; `echo_step.nf` writes `step_${idx}_output.txt`; the Snakefile wildcard chain matches `generate_hello`. README claimed "34 lines" for the Snakefile — actual 41. | traceable | **Was suspect; corrected** (41). The timeout/ceiling numbers are labeled historical measurements and were not re-run. |
| `benches/j-suggestion/` (`gen_dags.py`, `*.sbatch`) | Four wave-structured DAGs with the widths/threads and recorded old/new `-j` values in `README.md`; the probe recomputes engine suggestions. | Ran `gen_dags.py`: d1 60 rules (20/20/20×t1), d2 42 (20×t1/2×t16/20×t1), d3 42 (2×t16/20×t1/20×t1), d4 47 (20×t1/5×t4/2×t16/20×t1) — exactly the README table. | traceable | Recorded makespans are declared single-run historical measurements; not re-measured. |

### 3.4 Example fixtures (`examples/`)

| Family | Contract it pins | How the expectation traces | Verdict | Remediation / evidence |
| :--- | :--- | :--- | :---: | :--- |
| `examples/gallery/*.oxoflow` (16 files) | Valid, DAG-correct workflows against the workflow-format reference and JSON schema; the docs pages embed these exact files (`--8<--`); the CLI embeds the same files as `template` sources. | All 16 pass `oxo-flow validate`; `tests/cli_integration.rs::cli_validate_all_gallery_workflows` validates every file; `tests/integration_test.rs` parses/builds DAGs for 01–09 plus 10/11/12/13/14/15 coverage; `crates/oxo-flow-cli/src/commands/project.rs` drift-guards the embedded template copies against `examples/gallery/`. Environment backends: every `envs/*.yaml` reference resolves (see next row). | traceable | — |
| `examples/envs/*.yaml` (18 YAML + 1 requirements.txt) | One environment file per environment name referenced by the workflows; copyable next to a workflow (`examples/README.md`). | Enumerated every `conda`/`mamba`/`pixi`/`venv` reference in the 16 gallery files (64 references, 19 distinct targets): every named YAML exists under `examples/envs/`. Container images and `envs/demo-venv` are runtime artifacts, not files. | traceable | — |

### 3.5 Integration-test corpora

| Family | Contract it pins | How the expectation traces | Verdict | Remediation / evidence |
| :--- | :--- | :--- | :---: | :--- |
| `tests/fixtures/import/` (18 files: 14 `.err`, 4 `.ok`) | `.ok` fixtures must import and survive `validate`/`lint`/`dry-run`; `.err` fixtures must fail naming the marker on their `# EXPECT-ERROR:` line and must not write output; golden substrings pin translation fidelity (`crates/oxo-flow-cli/tests/import_fixtures.rs`). | Markers name the documented unsupported constructs/properties; the gate ran green: 2 tests pass (all supported fixtures + all unsupported fixtures). Golden substrings reject parser junk (`["A"`, `"])`). | traceable | — |
| `tests/fixtures/mock-scheduler/` (4 scripts) | The mock reproduces the *contract* of the real scheduler CLI: `sacct` emits allocation + `.batch` rows and populates MaxRSS/TotalCPU only on step rows; `squeue` lists live jobs only (`MOCK_SQUEUE_HIDE_TERMINAL`); `scancel` is authoritative over a late process exit; `sbatch --parsable` returns immediately. | Each claim is stated in the fixture and exercised by `crates/oxo-flow-core/src/backend/cluster.rs` parser tests (`sacct_max_rss_comes_from_the_step_rows`, header-driven layout, state vocabulary) and `tests/cluster_backend.rs` (`exit_code` "0:0", cancelled jobs settle CANCELLED). | traceable | The mock is a mock: results are CI integration evidence, never cluster empirical claims. |
| Crate-local inline corpora (Rust `#[cfg(test)]` strings, e.g. inline gallery-comment expectations in `crates/oxo-flow-cli/src/commands/info.rs`, SCATTER fixtures in `tests/*.rs`) | Feature contracts at the unit level. | Inline strings are inputs, not expected fixture files; where they duplicate fixture content (gallery-16 config comments) they are deliberate locks — a fixture edit fails the test rather than drifting silently. | annotated | Mirror risk acknowledged: these must be updated in the same change as their source fixture; no automated cross-check exists. |

### 3.6 Schema and generated data

| Family | Contract it pins | How the expectation traces | Verdict | Remediation / evidence |
| :--- | :--- | :--- | :---: | :--- |
| `docs/schema/oxoflow-v1.schema.json` + CLI copy + `editors/vscode/src/generated/*` | Canonical workflow schema; the CLI-served copy must match exactly (`make schema-drift`); the extension's completion/hover data is generated from it (`editors/vscode/scripts/generate-completions.mjs`) and its unit test reads the canonical schema. | `schema-drift` is part of `make ci`; `crates/oxo-flow-core/src/config/known_keys.rs` compiles the schema and asserts known keys exist in it; `editors/vscode/src/test/unit/completionData.test.ts` reads `docs/schema/oxoflow-v1.schema.json`. | traceable | — |

## 4. Demonstrated stale-semantics fixtures

### 4.1 `s04-silent-failure` (issue #1168 exit evidence)

**Original fixture:** one rule declaring `input = ["data/reads/sample_R1.fastq.gz"]`
(the file is intentionally absent) with
`shell = "zcat {input[0]} | wc -l > {output[0]}"`, `run_expect_exit: 0`,
surface `report`. The benchmark's premise was that the pipeline exits 0 with
a poisoned output and the interpretation must flag the misleading success.

**Current contract (documented):** `docs/guide/src/commands/run.md` §
"missing required source input" and `docs/guide/src/how-to/troubleshooting.md`
— a missing source input that no rule produces fails the run at start. The
engine now (correctly) refuses the workflow, so `run` exits 1 and the planted
silent failure is unreachable. Worse, the benchmark runner marks such a seed
`broken` and **excludes it from the denominator**, so the suite stayed green
while a documented seed class stopped measuring anything.

**Fail-first proof** (new gate `crates/oxo-flow-cli/tests/interpretation_seeds.rs`,
run against the unmodified seed):

```text
seed s04-silent-failure: the planted fault must exit 0, got Some(1)
...
Error: workflow is missing 1 required source input — no rule produces it:
  rule 'summarize': input 'data/reads/sample_R1.fastq.gz' does not exist and no rule produces it
```

**Correction:** the seed now stages the declared R1 read with an upstream
rule (so readiness passes) and plants the fault on an **undeclared** second
file: `gzip -cd {input[0]} data/reads/sample_R2.fastq.gz | wc -l`. The
pipeline exit code still comes from `wc` (exit 0) while stderr records
`gzip: … sample_R2.fastq.gz: No such file or directory`, preserving the
silent-failure class without violating the fail-closed contract. Verified on
macOS: exit 0, poisoned/incomplete summary, `stderr_tail` names only
`sample_R2.fastq.gz`; the manifest's `expect_any` now names that file and
the ground truth explains why the fail-fast gate does not apply. The same
gate now passes for all 12 seeds.

Assumption recorded: `gzip -cd` (unlike macOS `zcat`, which refuses `.gz`)
reads a real gzip and fails only on the missing member, so the seed is
portable across macOS and Linux. This is a fixture-design requirement, not
an empirical claim.

### 4.2 Frontier README counts

`eval/frontier/README.md` claimed "29 intents", "10 domains", and "Intent set
is 29" while `intents.json` contains 36 intents across 12 domains. The new
`eval/frontier/test_intents_contract.py` derives all counts from the fixture
and fails on the old README; run against `origin/main`'s README it fails with

```text
AssertionError: 'all 36 intents x naive-prompt,cli' not found in ...
```

and passes after correction. This is a documentation-side stale expectation,
not an engine claim, and is reported as such.

## 5. Suspect / annotated details

### 5.1 `eval/gold/workflow.csv` `wf-004` (open suspect)

`wf-004` (`04_scatter_gather.oxoflow`) declares:

```toml
scatter = { variable = "chr", values_from = "config.chromosomes", gather = "gather_gvcf" }
```

The reference therefore has five instance edges into `gather_gvcf` (verified:
`oxo-flow graph 04_scatter_gather.oxoflow -f dot --expanded` prints
`0..4 -> 5`). The gold row declares `expected_dag_edges: []`. The `dag-edge`
checker (`check_gold.reference_graph`) calls the **unexpanded**
`graph -f dot`, which prints only nodes and no edge, so an empty list passes
vacuously. `eval/schema.md` simultaneously claims the graph "also connects …
scatter-aware links" — true only for `--expanded`.

The correct expectation, derived independently of the current CSV, is
`[["haplotype_caller", "gather_gvcf"]]`: the gather target consumes the
scattered rule's per-chromosome outputs, which the engine's own runtime
plan (`dry-run`) and expanded graph both encode. A probe confirmed that
switching the checker to `--expanded` keeps every other local row green
and makes that edge checkable.

Why it is left open: correcting the expectation means (a) changing the
checker's graph surface to `--expanded` (verified to keep all 16 local rows
green in a probe), (b) editing a reviewed gold field, and (c) minting a new
`fields_hash` evidence record — a human re-attestation under issue #172,
which is a standing marker and out of this audit's scope. The derivation is
recorded here so the next reviewer can correct it with the ledger owner.

### 5.2 `eval/gold/tool.csv` generator is not a regeneration oracle

Re-running `gen_tool_csv.py` produces 151 draft rows; the committed CSV has
149 reviewed rows with `review_status`/`reviewer`/`review_comment` and
corrected versions. Byte-equality is therefore not a valid contract for this
fixture; the valid contract is the reviewed field set bound by
`eval/review/evidence.jsonl` plus the KB version-drift check, both of which
pass. Anyone adding a "regeneration equals committed" gate would encode a
false invariant.

### 5.3 Export/replay boundaries

- Custom benchmark numbers (`PILOT_FINDINGS.md`, comparative "reference
  numbers", j-suggestion makespans) are **historical measurements**, kept as
  records. They were not re-measured and are not evidence for current
  performance.
- The AI hit-rates in `eval/interpretation/README.md` were produced with a
  live provider and are not reproducible in CI; this audit called no
  provider. Only the deterministic exit-code contracts were verified.
- `eval/frontier/results/` and `benches/*/results/` are gitignored outputs,
  not fixtures. The tracked `eval/interpretation/results/*` directories are
  historical campaign records (run with the then-shipped seed text), also not
  fixtures; the `s04` redesign makes those rows non-comparable and the README
  says so. They are not counted in the inventory verdicts.

## 6. Verification commands (run 2026-11-18, this worktree)

| Command | Result |
| :--- | :--- |
| `cargo build -p oxo-flow-cli` | ok |
| `python3 eval/scripts/check_gold.py --no-fetch-community` | 0 hard findings, 0 warnings; 257 gold rows audited; 52 version pins; 10 negative names; 16 workflow references |
| `python3 -m unittest discover -s eval/scripts` | 102 tests ok |
| `python3 -m unittest discover -s eval/frontier -p 'test_*.py'` | 54 tests ok (52 existing + 2 new contract tests) |
| `python3 -m unittest discover -s eval/benchmarks -p 'test_*.py'` | 12 tests ok |
| `python3 eval/frontier/fault_injection.py --skip-repair --binary …` | static detection 7/10 as documented; infrastructure cases runtime-attributed; repair stages unmeasured by design |
| `cargo test -p oxo-flow-cli --test interpretation_seeds` | **fails on the original seed** (`s04` exit 1 vs 0), passes after correction (12/12 seeds) |
| `cargo test -p oxo-flow-cli --test import_fixtures` | 2 tests ok (18 fixtures) |
| `oxo-flow validate examples/gallery/*.oxoflow` | 16/16 pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test -p oxo-flow-cli -- --test-threads=1` | exit 0 (19 test binaries, 0 failures; includes the new seed gate) |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` | clean |

## 7. What this audit does NOT establish

- No claim about biological correctness, engine performance, AI quality, or
  cluster behavior. Every benchmark number cited is a fixture expectation or
  a historical record, never a new empirical result.
- No fixture expectation was accepted because the current binary emits it.
  The one place where the shipped expectation *does* trace to current engine
  output (`wf-004`) is marked suspect and left for a human-attested fix.
- The gold set's scientific content (`eval/gold/*` values) is bound by human
  review and the evidence ledger; this audit checked traceability and
  consistency of the *bindings*, not the underlying bioinformatics claims.
