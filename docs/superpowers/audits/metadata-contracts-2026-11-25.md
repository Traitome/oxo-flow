# Metadata-contract truth set and performance evidence (issue #1368)

Date: 2026-11-25
Scope: `[workflow.metadata_schema]` — validated cohort/metadata contracts,
`META-001` … `META-012` findings, the canonical metadata digest, and the
deterministic 10k/100k-row performance check.

## What was measured

- **Truth set**: 16 independently-authored fixture workflows under
  `tests/fixtures/metadata/`, each run through the compiled `oxo-flow
  validate --json` binary by `tests/metadata_contract.rs` (not a unit-only
  helper). Every case asserts the exact `(code, severity, row, column, id)`
  tuple, the exit status, and (for the clean trio) the digest.
- **Format equivalence**: one logical table written as TSV, CSV, and JSON
  (with JSON numbers/booleans) under the same schema must produce one
  byte-identical `metadata_digest`.
- **Digest sensitivity**: changing one value in the table changes the
  digest (and, via the core checkpoint test, the override-aware
  `workflow_resolved_hash`).
- **Performance**: a fixed-seed generated table (no randomness; the
  generator is closed-form and a Fisher–Yates shuffle is seeded by a fixed
  LCG) at 10,000 and 100,000 rows, validated plus digested in a debug
  build.

## Truth set (N = 16)

Commands: `cargo test --test metadata_contract` (CLI binary must be built
first). All 9 integration tests pass; the 16 cases are distributed across
them.

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
| 13 | `coverage_warn.oxoflow` | `META-010` ×2, `META-011` ×1 — **warnings** | 0 |
| 14 | `coverage_error.oxoflow` | same codes as #13 — **errors** | 1 |
| 15 | `pair_conflict.oxoflow` | `META-008` (`experiment == control`) | 1 |
| 16 | `empty_id.oxoflow` | parse error naming data row 2 and the empty id | 1 |

Additional assertions in the same suite:

- Metadata findings appear only in the `metadata` array — never duplicated
  into the pre-existing `errors` / `warnings` arrays.
- Human output prints exactly one `error [META-…]` / `warning [META-…]`
  line per finding (the coverage-error case pins 3 lines).
- `dry-run --json` exposes the same findings and digest, exits 1 on errors,
  and exits 0 on coverage warnings.
- The `empty_id` parse error message names the offending data row.

Unit-level coverage fills the gaps the file fixtures cannot express:
`META-008` duplicate `pair_id` (identical duplicates are de-duplicated with
a warning and conflicting ones already hard-fail during load, so this arm
is reachable only through the string-parse path), `META-009` pair-member
coverage, exact sorting, schema-declaration errors (duplicate column,
invalid regex, empty enum, empty `id_column`), and digest order/whitespace
normalization.

## Performance (debug build, macOS arm64)

Measured by `config::metadata::tests::ten_thousand_and_one_hundred_thousand_rows_are_linear_and_stable`
(5 columns, 4 typed/regex/enum/unique column constraints):

| Rows | `evaluate` time |
|---|---|
| 10,000 | 70 ms |
| 100,000 | 862 ms |

The test asserts 100k under a generous 10 s bound and asserts the 100k time
stays within `60 × 10k + 1 s` — a quadratic validator (≈100× the 10k time)
would fail. Repeated evaluation yields an identical report and digest;
row-shuffled copies of a 10k table with injected violations yield the same
digest and the same `(code, column, id)` finding multiset.

## What this evidence does and does not support

- It supports: the diagnostics are deterministic and row-addressable; the
  digest is canonical across the three supported table formats and row
  order; validation scales linearly over the measured range in a debug
  build.
- It does not support: biological correctness of any metadata value, a
  production-cohort benchmark (100k debug rows is a scale proxy, not a
  latency target), or breadth over real community samplesheets. Web
  admission parity is also not claimed: the web layer still surfaces a
  metadata-contract failure through the spawned CLI rather than at
  admission (tracked as remaining exit evidence in `ROADMAP.md`).
