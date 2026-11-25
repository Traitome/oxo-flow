# Metadata-contract truth set and performance evidence (issue #1368)

Date: 2026-11-25 (revised after independent review)
Scope: `[workflow.metadata_schema]` — validated cohort/metadata contracts,
`META-001` … `META-012` findings, the canonical metadata digest, and the
deterministic 10k/100k-row performance check.

## What was measured

- **Truth set**: 18 fixture workflows under `tests/fixtures/metadata/`,
  each run through the compiled `oxo-flow validate --json` binary by
  `tests/metadata_contract.rs` (not a unit-only helper).
- **Assertion strength** (stated precisely): 13 of the 18 cases assert the
  full `(code, severity, row, column, id)` tuple — the eight
  `row_level_codes_are_exact_and_sorted` cases, both coverage-severity
  cases (three tuples each), `pair_conflict` (META-008), `pair_coverage`
  (META-009, two tuples), and `freeform_duplicate` (a schema-less
  META-003 + META-010 warning pair). The three clean cases assert findings
  are absent and that TSV/CSV/JSON digests are equal; `custom_id` asserts
  absence; `empty_id` asserts the parse-error code and the row in the
  message; the run-side cases assert refusal/proceed behavior.
- **Format equivalence**: one logical table written as TSV, CSV, and JSON
  (with JSON numbers/booleans) under the same schema must produce one
  byte-identical `metadata_digest`. Unit tests additionally pin typed
  canonicalization (`007` ≡ `7`, `1.50` ≡ `1.5`, `TRUE` ≡ `true`) and
  duplicate-id row-order independence.
- **Digest sensitivity**: changing one value in the table changes the
  digest (and, via the checkpoint test, the override-aware
  `workflow_resolved_hash`).
- **Performance**: a fixed-seed generated table (no randomness; closed-form
  generator, seeded Fisher–Yates shuffle) at 10,000 and 100,000 rows,
  timing `evaluate` and `digest_for` separately in a debug build.

**Authorship limitation.** The fixtures and their hard-coded expectations
were written by the implementer alongside the implementation, from the
documented contract. They are not independent third-party cases, and they
do not substitute for a governance-grade truth set; they are a
regression/determinism harness. Independent (non-implementer) cases and
real community samplesheets remain outstanding.

## Truth set (N = 18)

Commands: `cargo build -p oxo-flow-cli`, then
`cargo test --test metadata_contract` (16 integration tests; all pass).

| # | Case (`tests/fixtures/metadata/`) | Expected finding(s) | Exit |
|---|---|---|---|
| 1 | `clean.oxoflow` + `clean.tsv` | none; `metadata: []`, digest present | 0 |
| 2 | `clean_csv.oxoflow` + `clean.csv` | none; digest identical to #1 | 0 |
| 3 | `clean_json.oxoflow` + `clean.json` | none; digest identical to #1/#2 | 0 |
| 4 | `missing_required_column.oxoflow` | `META-001` (table-level, column `sample_prep`) | 1 |
| 5 | `missing_value.oxoflow` | `META-002` row 2, column `batch`, id `S2` | 1 |
| 6 | `duplicate_id.oxoflow` | `META-003` row 2, column `sample`, id `S1` | 1 |
| 7 | `duplicate_unique.oxoflow` | `META-004` row 2, column `value`, id `S2` | 1 |
| 8 | `enum_violation.oxoflow` | `META-005` row 2, column `qc`, id `S2` | 1 |
| 9 | `regex_violation.oxoflow` | `META-006` row 2, column `batch`, id `S2` | 1 |
| 10 | `type_violation.oxoflow` | `META-007` row 1, column `age`, id `S1` | 1 |
| 11 | `extra_column.oxoflow` | `META-012` table-level, column `notes` | 1 |
| 12 | `custom_id.oxoflow` (`id_column = "sample_id"`) | none | 0 |
| 13 | `coverage_warn.oxoflow` | `META-010` (`sample`,`S2`) + `META-010` (`sample`,`S3`) + `META-011` (row 2, `sample`, `EXTRA`) — **warnings** | 0 |
| 14 | `coverage_error.oxoflow` | same three tuples as #13 — **errors** | 1 |
| 15 | `pair_conflict.oxoflow` (empty schema opts into strict) | `META-008` (table-level, `control`, `P1`) — error | 1 |
| 16 | `pair_coverage.oxoflow` + `pair_coverage.tsv` | `META-009` (`control`,`N2`) + `META-009` (`experiment`,`T2`) — **warnings** | 0 |
| 17 | `freeform_duplicate.oxoflow` (NO schema) | `META-003` (row 2, `sample`, `S1`) + `META-010` (`sample`, `S2`) — **warnings**; free-form tables never fail | 0 |
| 18 | `empty_id.oxoflow` | parse error naming data row 2 and the empty id | 1 |

Additional assertions in the same suite:

- Metadata findings appear only in the `metadata` array — never duplicated
  into the pre-existing `errors` / `warnings` arrays.
- Human output prints exactly one `error [META-…]` / `warning [META-…]`
  line per finding (the coverage-error case pins 3 lines).
- `dry-run --json` exposes the same findings and digest, exits 1 on errors,
  and exits 0 on coverage warnings.
- Run-side end-to-end: `run` refuses an error fixture, prints
  `error [META-003]`, and produces no rule output; `run` proceeds on the
  coverage-warning fixture and on the schema-less duplicate fixture (rule
  output present, warning printed); `validate --as-include` skips the
  contract entirely.
- `run --provenance` records the same digest `validate --json` reports;
  `provenance verify --json` reports it; a changed table changes both the
  digest and `workflow_resolved_hash`.

Unit-level coverage fills the gaps the file fixtures cannot express:
`META-008` duplicate `pair_id` (identical duplicates are de-duplicated with
a warning and conflicting ones already hard-fail during load, so this arm
is reachable only through the string-parse path), strict vs free-form
severity gating, pair-id-keyed META-009 coverage, header-only
`metadata_file` contracts, required-site suggestion wording, strict
`id_column`, schema-declaration errors, typed digest canonicalization, and
duplicate-id digest ordering.

## Performance (debug build, macOS arm64)

Measured by `config::metadata::perf_tests::ten_thousand_and_one_hundred_thousand_rows_are_linear_and_stable`
(5 columns, 4 typed/regex/enum/unique column constraints):

| Rows | `evaluate` | `digest_for` |
|---|---|---|
| 10,000 | 54 ms | 56 ms |
| 100,000 | 592 ms | 590 ms |

Both phases are timed and asserted: each 100k phase under a generous 10 s
bound, and each within `60 × 10k + 1 s` — a quadratic implementation
(≈100× the 10k time) would fail. Repeated evaluation yields an identical
report and digest; row-shuffled copies of a 10k table with injected
violations yield the same digest and the same `(code, column, id)` finding
multiset.

## What this evidence does and does not support

- It supports: the diagnostics are deterministic and row-addressable; the
  digest is canonical across the three supported table formats under the
  declared types and across row order; validation and digest scale
  linearly over the measured range in a debug build.
- It does not support: independence of the cases (implementer-authored),
  biological correctness of any metadata value, a production-cohort
  benchmark (100k debug rows is a scale proxy, not a latency target), or
  breadth over real community samplesheets. Web admission parity is also
  not claimed: the web layer surfaces a metadata-contract failure through
  the spawned CLI rather than at admission (tracked as remaining exit
  evidence in `ROADMAP.md`).
