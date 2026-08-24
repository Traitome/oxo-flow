# Eval Gold-Set Schema

The AI evaluation benchmark is organized as three CSVs — one per evaluation
layer — under `eval/gold/`. Each row is one question ("item") with a gold
answer. The gold answer is a **draft by construction** (`gold_draft_by =
claude`): every row carries a `provenance_url` pointing at the primary
source the answer was derived from, so a human reviewer (student, friend)
can verify each row against the source without trusting the draft.

Reviewers edit only the review columns; `eval/scripts/runner.py` scores
only rows with `review_status = approved` unless run with
`--include-unreviewed`.

## Common columns (all three CSVs)

| Column | Meaning |
|---|---|
| `id` | Unique item id, prefixed `tool-`, `rule-`, `wf-` |
| `layer` | `tool`, `rule`, or `workflow` |
| `difficulty` | `easy`, `medium`, or `hard` |
| `gold_draft_by` | Who drafted the gold answer (`claude`) |
| `review_status` | `draft` \| `approved` \| `corrected` \| `rejected` |
| `reviewer` | Reviewer name/handle (empty until reviewed) |
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

Judging (runner): tool-name token match, version match (when asked),
and — for `negative` items — that the answer does not hallucinate a tool.

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

Judging (runner): tool present in shell, version pinned and existing in the
embedded knowledge base, key params matched by regex, inputs/outputs
declared, resources inside the range, `oxo-flow validate` exit code.

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

Judging (runner): `validate` + `lint` exit codes, step-name coverage
(expected steps matched against generated rule names, loose token match),
tool coverage, DAG-edge coverage (edges inferred from declared inputs and
outputs), and output-pattern coverage. See `eval/README.md` for the exact
scoring formulas.

## Review workflow (for students/friends)

1. Open the CSV in a spreadsheet editor (Excel/Numbers/LibreOffice all
   handle RFC 4180; Google Sheets imports it directly).
2. For each row: click `provenance_url` and compare the gold answer with
   the source. If the draft is right, set `review_status = approved` and
   fill `reviewer`/`review_date`.
3. If the draft is wrong, fix the gold columns, set
   `review_status = corrected`, and describe the change in
   `review_comment` — one sentence is enough.
4. If the item itself is bad (ambiguous, unverifiable), set
   `review_status = rejected` and say why in `review_comment`.
5. Rows left at `draft` are skipped by the runner (see `--include-unreviewed`).
