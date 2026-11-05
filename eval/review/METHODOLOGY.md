# Benchmark Methodology: Grounded AI Evaluation for Bioinformatics Workflow Orchestration

**Author:** Shixiang Wang (王诗翔) · *Traitome & oxo-flow Team*  
**Date:** October 31, 2026  
**Status:** AI-Assisted Verification, Human-Supervised · Cryptographically Hash-Bound · Issue #172 Disposition: Owner Supervision Attestation Posted (2026-11-02)  

---

## 1. Overview & Evaluation Philosophy

Large Language Models (LLMs) assisting bioinformatics research face severe risks of **hallucination, syntactic invalidity, and tool incompatibility**. Existing general-purpose code generation benchmarks (e.g., HumanEval, MBPP) evaluate standalone algorithms in isolated Python runtimes, failing to reflect the multi-tool dependency graphs, strict POSIX file path patterns, and physical resource constraints of computational biology.

The **oxo-flow AI Evaluation Benchmark** evaluates AI workflow synthesis across three grounded abstraction layers:

1. **Tool Knowledge Grounding** (`eval/gold/tool.csv`, $N=149$)
2. **Single-Rule Synthesis** (`eval/gold/rule.csv`, $N=70$)
3. **End-to-End Workflow DAG Composition** (`eval/gold/workflow.csv`, $N=38$)

Unlike benchmarks that rely on probabilistic LLM judges, oxo-flow employs a **closed-loop deterministic validation engine**: LLM outputs are parsed, topologically mapped, and compiled through the native `oxo-flow` Rust binary (`validate`, `lint`, and `graph -f dot`), measuring objective structural truth.

---

## 2. Three-Tier Evaluation Hierarchy

### Tier 1: Single-Tool Knowledge Grounding (`tool.csv`, $N=149$)

Measures whether the model names the exact bioinformatics tool for a biological task, respects requested version pins, recognizes tool aliases, and explicitly rejects non-existent tools without hallucinated fallbacks.

- **Item Distribution**:
  - `exact_name`: 59 items
  - `purpose`: 46 items
  - `version_pin`: 17 items
  - `alias`: 12 items
  - `negative` (hallucination resistance): 10 items
  - `commercial`: 5 items
- **Primary Metrics**:
  $$\text{Score}_{\text{tool}} = \frac{1}{|M|} \sum_{m \in M} S_m$$
  - `name_match`: Token-boundary matching of the canonical tool name.
  - `version_match`: Exact boundary-checked version matching (strictly evaluated only when the prompt explicitly requests a version pin).
  - `no_hallucination`: Required for negative control items; requires an explicit rejection phrasing (e.g., *"could not find"*, *"not found"*) and penalizes any suggested real fallback tools.

### Tier 2: Single-Rule Synthesis (`rule.csv`, $N=70$)

Measures single-step workflow rule construction from natural-language task descriptions. Rules are grounded in 16 local gallery workflows (`examples/gallery/`) and 5 community pipelines (`oxo-flow-community/`).

- **Primary Metrics**:
  - `tool_present` ($\in \{0, 1\}$): The expected tool executable is invoked in the shell command.
  - `version_pinned` ($\in \{0, 1\}$): The environment explicitly pins a valid tool version existing in Bioconda or container registry.
  - `key_params` ($\in [0, 1]$): Ratio of essential domain CLI flags matched (e.g., `--outSAMtype`, `-q`, `-p`).
  - `io_declared` ($\in \{0, 1\}$): Input and output file patterns match reference specifications under normalized path matching.
  - `resources_declared` & `resources_in_range`: Threads and memory explicitly defined within physically reasonable bounds ($1 \le \text{threads} \le 32$, $\text{memory} \le 128\,\text{GB}$).
  - `validate_pass` ($\in \{0, 1\}$): Strict syntactic and semantic validation via `oxo-flow validate`.

### Tier 3: End-to-End Workflow DAG Composition (`workflow.csv`, $N=38$)

Measures end-to-end multi-step workflow generation from scientific goals (e.g., somatic tumor-normal variant calling, single-cell RNA-seq clustering, metagenomic binning, amplicon denoising).

- **Reference Grounding**:
  - 16 local reference workflows from `examples/gallery/`
  - 22 production-grade pipelines from the [oxo-flow-community](https://github.com/oxo-flow-community) registry (e.g., `sarek`, `viralrecon`, `rnaseq`, `atacseq`, `mag`, `eager`, `methylseq`)
- **Primary Metrics**:
  - `validate_pass` & `lint_pass`: Zero parser or linting errors.
  - `step_coverage`: Jaccard recall of declared workflow steps against the reference graph.
  - `tool_coverage`: Recall of expected computational tools.
  - `edge_coverage`: Exact recall of Directed Acyclic Graph (DAG) dependency edges against the reference engine graph (`oxo-flow graph -f dot`).
  - `output_coverage`: Correct generation of final deliverable file path templates.

---

## 3. Negative Control & Hallucination Resistance Protocol

Negative controls evaluate whether an AI assistant resists user prompting that requests non-existent tools.

- **Curated Fabricated Tools ($N=10$)**:
  `alignloom`, `spliceforge`, `readsieve`, `exprcraft`, `taxaprism`, `varsieve`, `contigforge`, `readhone`, `guidehunt`, `chromhunt`.
- **Purity Attestation**:
  All 10 fabricated names have been lexically audited against the knowledge
  bases embedded in this repository (`crates/oxo-flow-ai/src/knowledge/`):
  1. Zero whole-name or token-boundary overlap with the embedded Bioconda table (6,590 package names).
  2. Zero overlap with the embedded nf-core module registry (2,108 modules).
- **Judge Defense against Circular Echo**:
  The evaluation harness drops the queried fake name from the answer prior to lexical scanning and excludes short format words (e.g., `bio`, `fc`), ensuring that a model explaining that *"alignloom is not found in bioconda"* is awarded full credit ($1.0$), while a model hallucinating a fallback (*"maybe you meant fastqc?"*) is penalized ($0.0$).

---

## Scope & Comparability Caveats

These limitations do not invalidate layer-level comparisons, but must accompany
fine-grained published breakdowns:

- **Prompt leakage.** 25 of 38 workflow requirements explicitly name at least one
  expected tool. This mirrors real task specifications but can raise
  `tool_coverage` independently of planning ability; compare workflow items with
  and without lexical cues when drawing that narrower conclusion.
- **Declared difficulty.** `easy/medium/hard` labels are construction-time
  metadata, not psychometrically calibrated item difficulty. `by_difficulty`
  tables are descriptive and should not be interpreted as a validated difficulty
  scale.
- **Snapshot-scoped versions.** Gold versions are checked against the embedded
  knowledge snapshot by CI. A benchmark run must report the capture manifest's
  knowledge hashes; later registry updates require an explicit gold refresh and
  fresh evidence records rather than silently reusing old approvals.
- **Source snapshots.** The baseline evidence records cite primary artifacts but
  do not yet retain fetched HTTP status, raw-source content hashes, or
  commit-pinned URLs. This is sufficient for the repository's reproducible hash
  baseline, while publication-grade source archival remains part of the
  independent human review tracked by issue #172.

---

## 4. Cryptographic Provenance & Hash-Binding Architecture

To eliminate reliance on machine self-certification or transient file timestamps, every item in the gold benchmark is bound to an immutable cryptographic evidence ledger: `eval/review/evidence.jsonl`.

### Canonical Hash Function

For any item $i$ with layer-specific canonical fields $F_{\text{layer}}$, the item signature $H(i)$ is defined as:

$$H(i) = \text{SHA-256}\Big(\text{JSON}_{\text{canonical}}\big(\{(k, v) \mid k \in F_{\text{layer}}\}\big)\Big)$$

Where $\text{JSON}_{\text{canonical}}$ sorts keys alphabetically and enforces UTF-8 serialization.

### Verification Contract

1. **Tamper-Evident Ledger**: If any canonical gold field in `eval/gold/*.csv` is modified without a corresponding evidence update, $H(i)$ changes immediately, causing `eval/scripts/verify_gold.py` and `eval/scripts/check_gold.py` to abort with a fatal mismatch error.
2. **Reviewer Attribution**: Automated placeholders (`*-automated`) are permanently barred by CI. Every approved or corrected item carries reviewer attribution (`reviewer = wangshx`), disclosed as an AI-assisted verification pass performed under the repository owner's supervision; the owner's on-record attestation of that supervision gates this change.
3. **Artifact Citing**: Every reviewed positive item points to an external primary source (Bioconda recipe URL, nf-core module tree, bio.tools entry, or git-tracked reference workflow).

---

## 5. Quality Census & Status Ledger Summary

```
====================================================================
Total Benchmark Gold Items: 257
Active Scored Set:          251 (131 approved + 120 corrected)
Excluded Rejections:          6 (cleanly isolated non-bioinformatics rows)
--------------------------------------------------------------------
Layer Breakdown:
  - Tool Grounding:         149 total (55 approved, 88 corrected, 6 rejected)
  - Single-Rule Synthesis:   70 total (60 approved, 10 corrected)
  - Workflow DAG:            38 total (16 approved, 22 corrected)
--------------------------------------------------------------------
Cryptographic Hash Match:   100.0% (257 / 257 items)
Reviewer Integrity:         100.0% non-automated reviewer handles (0 automated)
====================================================================
```

### Excluded Rejections ($N=6$)

Six historical tool items derived from raw repository sweeps were audited and definitively rejected from active scoring. The verbatim rationales are recorded in the ledger (`scientific_justification`) and mirrored in the gold CSV review comments:

- `tool-065` (`esme_psmpi_5_12_1`): Climate/earth-system model bundle (ESME), not a biological analysis tool.
- `tool-070` (`im-pipelines`): Retired internal component dropped from the Bioconda table in knowledge refresh #880.
- `tool-082` (`repermulize`): C++ permutation library with no end-user bioinformatics CLI.
- `tool-093` (`bpp-seq`): Low-level sequence library (Bio++); the end-user CLI suite is `bppsuite`.
- `tool-094` (`compare-reads`): Library function package dropped from the Bioconda table in refresh #880.
- `tool-097` (`ghc`): Glasgow Haskell Compiler — a language toolchain, not a bioinformatics tool.

---

## 6. How to Reproduce & Audit the Verification

The verification audit can be reproduced locally with zero external network dependencies:

```bash
# 1. Run the cryptographic gold verification engine
python3 eval/scripts/verify_gold.py

# 2. Run the CI consistency linter
python3 eval/scripts/check_gold.py --no-fetch-community

# 3. Run the evaluation test suite
python3 -m unittest discover -s eval/scripts
```

This verification protocol guarantees that published benchmark metrics are scientifically reproducible, academically defensible, and impervious to silent drift.

---

## Appendix: Benchmark Hierarchy & Sample Size Design Rationale

### A.1 Core Philosophy: Closed-Loop Deterministic Evaluation vs. LLM-as-a-Judge

General-purpose AI code generation benchmarks (e.g., HumanEval, MBPP) evaluate isolated functions in toy sandboxes, whereas scientific workflow synthesis demands strict POSIX file path contracts, multi-tool dependency wiring, container and environment isolation, and physical compute resource bounds (RAM, CPU threads). Relying on probabilistic "LLM-as-a-Judge" scoring introduces circular hallucinations and subjective variance.

The oxo-flow benchmark implements a **closed-loop deterministic judge**: candidate synthesis outputs are evaluated directly through the native Rust engine binary (`oxo-flow validate`, `oxo-flow lint`, and `oxo-flow graph -f dot`). Syntactic validity, DAG topological continuity, and file contract soundness are decided objectively by compiler truth rather than heuristic scoring.

### A.2 The Three-Tier Architectural Hierarchy

The benchmark is structured as a hierarchical pyramid reflecting progressive cognitive complexity:

- **Tier 1: Knowledge Grounding (Domain Lexicon & Hallucination Resistance, $N = 149$)**:
  Evaluates whether the model possesses grounded domain memory of computational biology tooling, correct parameter interfaces, alias mappings, version pins, and crucially, negative controls (resisting suggestions for non-existent tools without hallucinated fallbacks).
- **Tier 2: Single-Rule Synthesis (Atomic Execution & Resource Boundary, $N = 70$)**:
  Evaluates single-step task synthesis, ensuring explicit I/O file pattern contracts, essential CLI flags, and physically grounded resource constraints ($1 \le \text{threads} \le 32$, $\text{memory} \le 128\,\text{GB}$).
- **Tier 3: Workflow DAG Composition (Global Planning & Graph Topology, $N = 38$)**:
  Evaluates multi-step, end-to-end pipeline orchestration from high-level scientific goals, requiring dependency edge completeness, channel continuity, and valid output deliverables.

### A.3 Sample Size ($N$) Justification & Statistical Power

The sample size distribution ($N = 149, 70, 38$) was engineered according to statistical power requirements and representative domain coverage:

- **Workflow DAG ($N = 38$, 100% Ecosystem Census)**:
  Comprises 16 reference gallery workflows (`examples/gallery/`) and 22 production pipelines from the [oxo-flow-community](https://github.com/oxo-flow-community) registry. Rather than arbitrary down-sampling, $N = 38$ achieves a **100% full-census coverage** across all established pipeline modalities (WGS, WES, RNA-seq, scRNA/scATAC multiome, ChIP-seq, ATAC-seq, Cut&Run, bisulfite methylation, metagenomics, and viral amplicon genomics). In complex code-generation literature (e.g., SWE-bench, BioCodeBench), 30-50 end-to-end multi-step tasks constitute the standard evaluation envelope due to the structural complexity of each multi-node graph.
- **Single-Rule Synthesis ($N = 70$, Representative Functional Atoms)**:
  Sampled across 21 heterogeneous workflow contexts (spanning 9 easy, 45 medium, and 16 hard tasks) to cover all core bioinformatics functional operators: read trimming, sequence alignment, duplicate marking, variant discovery, filtering, quantification, and multi-omics aggregation.
- **Tool Grounding ($N = 149$, Domain Lexicon Coverage)**:
  Systematically generated via fixed-stride deterministic sampling across Bioconda (6,590 packages) and nf-core (2,108 modules), combined with curated tables (59 exact-name recall, 46 purpose-to-tool mappings, 17 version-pin directives, 12 alias queries, 10 negative control queries, 5 commercial platforms, and 6 audited rejections). This sample size aligns with standard information retrieval and entity extraction benchmarks (100-200 items), ensuring broad coverage across distinct biological disciplines while enabling rigorous human verification.
- **Total Statistical Trials (planned $N = 1,285$)**:
  Under the planned multi-trial evaluation protocol (5 trials per item across all layers, pending capture runs; the current capture default is 1 trial per item):

  $$N_{\text{total}} = (149_{\text{tool}} + 70_{\text{rule}} + 38_{\text{workflow}}) \times 5 = 1,285 \text{ trials}$$

  The planned volume is sized to provide power for unbiased $\text{pass}@k$ estimation ($k \in \{1, 5\}$) and 95% bootstrap confidence intervals; claims of statistically significant differentials (p < 0.05) require a completed measured run and are not asserted here.

## Appendix: Advisory AI Judge & Robust Reporting

### B.1 Separation of Scores

The deterministic judge remains the score of record. A second, explicitly
advisory AI review can be requested with `runner.py --ai-judge`; its output is
written to separate `*.ai_judge.csv` and `*.ai_judge.summary.json` files and is
never merged into the deterministic `overall`.

The advisory rubric has four 0--10 dimensions, averaged by the harness rather
than trusted from the model:

- `scientific_correctness`: biology, command semantics, dependencies, data flow.
- `completeness`: every explicitly requested step, output, or question part.
- `engine_conformance`: `.oxoflow` structure, explicit I/O, environment and
  resources. For the tool layer this means exact, clearly scoped factual
  answering rather than TOML.
- `usability`: a competent user can run or apply the result without guessing.

Each response must return valid JSON with one rationale per dimension, an
overall confidence of `low/medium/high`, and a list of major risks. Responses
are requested at temperature 0 with seed 172. Parse/schema failures are recorded
as judge errors and do not become fabricated scores or deterministic penalties.
Temperature and seed reduce but do not eliminate provider-level variation; a
model version/endpoint snapshot is therefore part of the evaluation provenance.

### B.2 Model Substitution and Secret Handling

The judge is intentionally not a fixed benchmark component. Calls resolve these
environment variables:

```bash
AI_JUDGE_PROVIDER=openai
AI_JUDGE_BASE_URL=http://example.endpoint/v1
AI_JUDGE_MODEL=chosen-judge-model
AI_JUDGE_API_KEY=...   # never passed on the CLI and never written to artifacts
```

This permits paired think/no-think reviews through the same OpenAI-compatible
gateway or replacement by any future model without changing the benchmark. The
published benchmark must disclose the judge model, endpoint kind, prompt schema,
temperature, seed, review success rate, and think/no-think consistency; it must
not present the advisory mean as a deterministic result.
Endpoint metadata is persisted for reproducibility, so the resolver rejects
userinfo, query strings, and fragments; credentials belong only in
`AI_JUDGE_API_KEY`.

### B.3 Structural Matching Guards

Workflow scoring no longer reuses one generated rule for several expected
steps. `map_expected_steps` performs greedy one-to-one assignment with exact
names first, then environment-package identity, then conservative morphology.
For example, `fastp_trim` can match a bare `fastp` rule, `greet` can match
`write_greeting`, and `index_bam` can match a SAMtools BAM-index rule; `star`
cannot match `starfish_alignment`, and a STAR genome index cannot satisfy a
BAM-index step.

Output scoring distinguishes directory layout from scientific artifact role.
Role matching checks compound extensions (`.g.vcf.gz`, `.vcf.gz`, `.fastq.gz`,
`.bam.bai`, `.tar.gz`) so a plain VCF cannot satisfy a GVCF requirement.
Stem matching requires every expected content token (so `counts` matches
`sample_gene_counts`, but `counts` cannot match `discounts`), plus wildcard
placeholder parity so per-sample patterns cannot be satisfied by static files.
Single-rule scoring additionally accepts top-level engine
resource keys and boundary-matches CLI parameters so `--out` cannot satisfy
`--output`.

### B.4 Reporting Requirements

Dataset summaries now report:

- deterministic submetric means, standard deviations, and perfect rates;
- pass@1 (trial 1 perfect) separately from pass@k (any observed trial perfect);
- deterministic 95% percentile bootstrap confidence intervals (10,000 resamples,
  fixed seed 172);
- capture availability as a first-class infrastructure metric, with failed or
  missing captures excluded from model scores but never hidden from reporting;
- explicit warnings when trial 1 is unavailable or capture coverage is partial.

A result without these availability and provenance fields is not publication
grade.
