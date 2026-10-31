# Eval Gold-Set Schema

The AI evaluation benchmark is organized as three CSVs — one per evaluation
layer — under `eval/gold/`. Each row is one question ("item") with a gold
answer. The gold answer is a **draft by construction** (`gold_draft_by =
claude`): every row carries a `provenance_url` pointing at the primary
source the answer was derived from, so a human reviewer can verify each row against the source without trusting the draft.

Reviewers edit only the review columns; `eval/scripts/runner.py` scores rows with `review_status = approved` **or** `corrected` (a corrected row has had its gold columns repaired during review, so it is judged against its fixed answer) unless run with `--include-unreviewed`. When no reviewable rows exist, the harness exits with an explicit error instead of silently emitting an empty report.

## Common columns (all three CSVs)

| Column | Meaning |
|---|---|
| `id` | Unique item id, prefixed `tool-`, `rule-`, `wf-` |
| `layer` | `tool`, `rule`, or `workflow` |
| `difficulty` | `easy`, `medium`, or `hard` |
| `gold_draft_by` | Who drafted the gold answer (`claude`) |
| `review_status` | `draft` \| `approved` \| `corrected` \| `rejected` |
| `reviewer` | Reviewer handle (the repository owner, `wangshx`, for AI-assisted reviewed passes; empty until reviewed) |
| `review_comment` | Reviewer's note, esp. what was corrected and why |
| `review_date` | ISO date of the review |

CSV quoting: fields containing commas, quotes, or newlines must be quoted
per RFC 4180. JSON-valued columns are always single-line JSON arrays or
objects, so they stay inside their quoted cell.

## 1. `tool.csv` — single-tool grounding

One item = one query the AI must answer with a tool name (and where the
query asks for one, a version).

| Column | Meaning |
|---|---|
| `query` | The natural-language query asked of the AI |
| `query_type` | `exact_name` \| `purpose` \| `alias` \| `version_pin` \| `commercial` \| `negative` |
| `expected_tool` | Tool name the answer must contain (empty for `negative`) |
| `expected_version` | Version the answer must contain (empty if the query does not ask for one) |
| `expected_source` | `bioconda` \| `nfcore` \| `commercial` \| `biotools` \| `none` |
| `negative_sample` | `1` if the correct answer is "tool not found", else `0` |
| `provenance_url` | Primary source (bioconda recipe / nf-core module / vendor page) |
| `provenance_date` | ISO date the source value was read (from `knowledge_meta.json`) |

Judging (runner): tool-name token match, version match (when asked), and — for `negative` items — that the answer explicitly rejects the fake tool without suggesting a known fallback tool.

## 2. `rule.csv` — single-rule generation

One item = one task description the AI must turn into a single-rule
oxo-flow workflow. Gold answers derive from gallery or community workflows.

| Column | Meaning |
|---|---|
| `task_description` | The natural-language task given to the AI |
| `context_note` | Extra constraints (e.g. paired-end, reference available) |
| `expected_tool` | The tool the rule's shell must invoke |
| `expected_version` | Version that should be pinned (`bioconda::tool=X.Y.Z` or docker tag); may be empty for `easy` items |
| `expected_key_params` | JSON array of key flags the shell should contain, e.g. `["-q", "--outSAMtype"]` |
| `expected_inputs` | JSON array of input path patterns, e.g. `["raw/{sample}_R1.fastq.gz"]` |
| `expected_outputs` | JSON array of output path patterns |
| `resource_range` | JSON object `{"threads_min":1,"threads_max":32,"memory_max_mb":131072}` |
| `validate_must_pass` | `1` (the generated workflow must pass `oxo-flow validate`) |
| `provenance_url` | Source of the gold rule (repo file URL) |
| `reference_workflow` | `examples/gallery/06_rnaseq_quantification.oxoflow` or a community repo |
| `reference_rule` | Rule name inside the reference workflow |

Judging (runner): tool present in shell, version pinned and existing in the embedded knowledge base, key params matched by regex, inputs/outputs declared by normalized path-suffix matching, resources explicitly declared, resources inside the allowed range, and `oxo-flow validate` exit code.

## 3. `workflow.csv` — end-to-end workflow generation

One item = one natural-language requirement the AI must turn into a full
multi-rule workflow. Gold answers derive from the gallery and the community
workflow repositories.

| Column | Meaning |
|---|---|
| `requirement_text` | The natural-language requirement given to the AI |
| `expected_steps` | JSON array of step names the workflow should contain (order-insensitive), e.g. `["fastp_trim","star_align","featurecounts"]` |
| `expected_tools` | JSON array of tool names the workflow must use |
| `expected_dag_edges` | JSON array of `["from_step","to_step"]` pairs that must exist (by expected step name) |
| `expected_outputs` | JSON array of final output path patterns |
| `must_validate` | `1` |
| `must_lint` | `1` (generated workflow must pass `oxo-flow lint`) |
| `reference_repo` | `examples/gallery` or a community repo URL |
| `reference_file` | Path to the reference workflow |
| `provenance_url` | Source of the gold steps (repo file URL) |

Judging (runner): `validate` + `lint` exit codes, step-name coverage (including namespaced rules such as `qc::fastqc`), tool coverage, DAG-edge coverage against the engine's own dependency graph (`oxo-flow graph -f dot`, which also connects directory inputs, `depends_on` and scatter-aware links), and output-pattern coverage. See `eval/README.md` for the exact scoring formulas.

## Review workflow (publication-track)

Reviewer identity: `reviewer` records the repository owner's handle (`wangshx`);
`*-automated` placeholders are strictly forbidden and guarded by CI. The
attribution reflects an AI-assisted verification pass performed under the
owner's supervision, recorded verdict-by-verdict in `eval/review/evidence.jsonl`
(see `eval/review/METHODOLOGY.md`). A `fields_hash` binds each verdict to the
exact gold fields it judged.

1. Do **not** open the gold CSVs in a spreadsheet editor: rows carry long
   single-line JSON cells and ISO dates, and Excel/Numbers rewrite quoting
   and dates on save. Edit through a CSV-aware editor or script instead.
2. Every row is bound in `eval/review/evidence.jsonl` with a canonical
   SHA-256 hash over its gold fields:
   `python3 eval/scripts/verify_gold.py` verifies 100% hash consistency. Any
   un-audited edit to a gold column alters the hash and fails CI. To change a
   gold field legitimately: re-verify the row against its `provenance_url`,
   update the gold columns, then mint a fresh evidence record (new
   `fields_hash`, verdict, `verified_at`, justification) for the item.
3. Every approved or corrected item points to an external primary artifact
   (Bioconda recipe URL, nf-core module registry, bio.tools entry, or git-tracked
   reference workflow). Negative controls cite verified non-existence in the
   embedded Bioconda (6,590 recipes) and nf-core (2,108 modules) registries.
4. Non-bioinformatics packages and retired internal components are marked
   `review_status = rejected` with a specific rationale in the review comment
   and the ledger justification, and isolated from active benchmark
   denominators.
5. Run `python3 eval/scripts/verify_gold.py` and `python3 eval/scripts/check_gold.py`
   (`make eval-lint`) before and after any benchmark modifications.
