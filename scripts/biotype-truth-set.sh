#!/usr/bin/env bash
# biotype-truth-set.sh — run the BioType truth set through the REAL CLI
# validation surface and print a 2x2 confusion matrix plus a diagnostic-code
# cross-check (#1262, broadened by #1163).
#
# Truth set (43 cases):
#   * 35 typed/adversarial cases under tests/truthset/biotype/cases/
#     (t01..t06 hand-authored adversarial edges modelled on the Nextflow/
#      Snakemake import-fixture format edges — only t01/t06 track edges
#      literally present in rnaseq.ok.nf; a06..a14 hand-authored adversarial
#      probes at the TYPE-code boundaries; b01..b20 the #1163 breadth wave,
#      each authored from the published contract (schema/spec/TYPE-* catalogue)
#      rather than from the checker source).
#   * 8 real repository .oxoflow files under examples/gallery/ that were
#     NOT written for the checker (7 untyped + 17_typed_variant_calling, the
#     real typed pipeline added by #1272; all expected accept).
#
# Every typed case carries its pre-registered expected verdict in a header
# comment; the EXPECT table below is the mechanical source of truth for the
# matrix and the codes cross-check so they are computed, not eyeballed.
#
# Scoring (#1262):
#   * VERDICT (primary): read from the REAL admission surface,
#     validate <case> --json (the `valid` field). The 2x2 matrix uses verdict
#     only; the codes cross-check does NOT weaken it, the two are separate.
#   * CODES (cross-check): the case's ACTUAL code set is the union of the
#     TYPE-NNN codes seen on `validate --json` and on `lint --json`, compared
#     for set-equality against the EXPECT expected-codes column. A case is
#     codes-correct=yes only if the sets are EQUAL. The lint surface is
#     load-bearing: the advisory codes (TYPE-008/009/010) are dropped by
#     `validate_workflow_types` and never reach `validate --json`, so reading
#     lint too is what lets an accept-with-advisory case score its advisory
#     code instead of an empty set. A verdict-pass/codes-fail case stays
#     visible as its own codes cell rather than folded into a green verdict.
#
# Usage:  scripts/biotype-truth-set.sh [path-to-oxo-flow-binary] [json-out-path]
# Exit:   0 always (a measurement, not a gate); prints the matrix + summary.
#         When json-out-path is given, a machine-readable per-case result file
#         (command, commit SHA, workflow path, expected, actual) is written there.
#
# Identity (ledger §8): EXPECT was frozen at the case-authoring commit (see
# the report's pre-registration timestamp). Re-running against a newer
# checker may change ACTUAL only.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${1:-$ROOT/target/debug/oxo-flow}"
JSON_OUT="${2:-}"

if [[ ! -x "$BIN" ]]; then
  echo "oxo-flow binary not found/executable at: $BIN" >&2
  echo "build it first: cargo build -p oxo-flow-cli" >&2
  exit 2
fi

SHA="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"

# repo-relative-path <TAB> expected(accept|reject) <TAB> expected-codes(comma|-) <TAB> note
# The expected-codes column is ASSERTED (set-equality) since #1262 — see Scoring.
EXPECT="$(mktemp)"
cat > "$EXPECT" <<'TABLE'
tests/truthset/biotype/cases/t01_star_bam_to_featurecounts.oxoflow	accept	-	nextflow STAR->featureCounts edge
tests/truthset/biotype/cases/t02_bwa_sam_to_sorted_bam_consumer.oxoflow	reject	TYPE-001,TYPE-002	nextflow bwa-mem2 SAM->MarkDuplicates BAM
tests/truthset/biotype/cases/t03_bcftools_oz_to_indexed_consumer.oxoflow	accept	-	bcftools bgzip VCF -> tabix consumer
tests/truthset/biotype/cases/t04_gzip_vcf_to_tabix_consumer.oxoflow	reject	TYPE-003,TYPE-006	plain gzip VCF -> bgzip+tabix consumer
tests/truthset/biotype/cases/t05_cram_hg19_to_grch38_consumer.oxoflow	reject	TYPE-004	CRAM hg19 -> GRCh38 consumer
tests/truthset/biotype/cases/t06_fastq_gz_passthrough.oxoflow	accept	-	trimmomatic gz FASTQ -> salmon
tests/truthset/biotype/cases/a06_queryname_bam_to_coordinate_consumer.oxoflow	reject	TYPE-002	queryname BAM -> coordinate consumer
tests/truthset/biotype/cases/a07_bam_no_readgroup_to_bqsr.oxoflow	reject	TYPE-005	no-@RG BAM -> BQSR (RG required)
tests/truthset/biotype/cases/a08_multichannel_default_mismatch.oxoflow	accept	TYPE-010	multi-out producer -> default-keyed consumer (advisory TYPE-010 in checker, #1254)
tests/truthset/biotype/cases/a09_multichannel_namekey_default.oxoflow	reject	TYPE-002	CONTROL for a08, path-keyed consumer
tests/truthset/biotype/cases/a10_build_alias_hg38_grch38.oxoflow	accept	-	hg38 alias == GRCh38
tests/truthset/biotype/cases/a11_multichannel_namekey_mismatch.oxoflow	accept	TYPE-010	multi-out named producer -> named consumer (advisory TYPE-010 in checker, #1254)
tests/truthset/biotype/cases/a12_bam_implicit_bgzip.oxoflow	accept	-	plain BAM vs consumer bgzip requirement (FALSE-REJECT)
tests/truthset/biotype/cases/a13_somatic_to_germline_consumer.oxoflow	reject	TYPE-007	somatic VCF -> germline joint-calling
tests/truthset/biotype/cases/a14_typed_consumer_untyped_producer.oxoflow	accept	TYPE-008	TYPE-008 advisory edge must not fail
tests/truthset/biotype/cases/b01_bcf_to_vcf_consumer.oxoflow	reject	TYPE-001	BCF producer -> text-VCF consumer (format boundary)
tests/truthset/biotype/cases/b02_cram_to_bam_consumer.oxoflow	reject	TYPE-001	CRAM producer -> BAM consumer (format boundary)
tests/truthset/biotype/cases/b03_fasta_fai_consumer.oxoflow	reject	TYPE-003	FASTA with no .fai -> index=fai consumer
tests/truthset/biotype/cases/b04_gtf_accept.oxoflow	accept	-	GRCh38 GTF -> GRCh38 GTF consumer
tests/truthset/biotype/cases/b05_bed_build_match.oxoflow	accept	-	GRCh38 BED -> GRCh38 BED consumer
tests/truthset/biotype/cases/b06_bed_to_gtf_consumer.oxoflow	reject	TYPE-001	BED producer -> GTF consumer (format boundary)
tests/truthset/biotype/cases/b07_bam_bai_to_csi_consumer.oxoflow	reject	TYPE-003	BAM[bai] -> BAM[csi] consumer (index-kind boundary)
tests/truthset/biotype/cases/b08_build_alias_mm39_grcm39.oxoflow	accept	-	mm39 alias == GRCm39
tests/truthset/biotype/cases/b09_build_mismatch_grch37_grch38.oxoflow	reject	TYPE-004	GRCh37 VCF -> GRCh38 joint-caller (no aliasing)
tests/truthset/biotype/cases/b10_untyped_pure_accept.oxoflow	accept	-	untyped trim+quant (opt-in: no types => no diagnostic)
tests/truthset/biotype/cases/b11_untyped_bed_bam_accept.oxoflow	accept	-	untyped BED+BAM error-prone edge stays silent
tests/truthset/biotype/cases/b12_multichannel_paired_accept.oxoflow	accept	-	paired r1/r2 channels pair across producer rules (no TYPE-010)
tests/truthset/biotype/cases/b13_germline_to_somatic_consumer.oxoflow	reject	TYPE-007	germline VCF -> somatic consumer (attribute boundary)
tests/truthset/biotype/cases/b14_coordinate_to_queryname_consumer.oxoflow	reject	TYPE-002	coordinate BAM -> queryname consumer (sort boundary)
tests/truthset/biotype/cases/b15_bgzip_vcf_both_sides_accept.oxoflow	accept	-	bgzip VCF+tbi both sides, non-BAM compression short-circuit
tests/truthset/biotype/cases/b16_named_types_build_mismatch.oxoflow	reject	TYPE-004	named [types.*] hg19 -> GRCh38 (named-table resolution)
tests/truthset/biotype/cases/b17_genuinely_unpaired_advisory.oxoflow	accept	TYPE-010	two-channel producer -> default-keyed consumer (advisory TYPE-010)
tests/truthset/biotype/cases/b18_cram_implied_bgzip.oxoflow	reject	TYPE-006	plain CRAM -> CRAM[compression=bgzip] (CRAM is NOT BGZF)
tests/truthset/biotype/cases/b19_build_incomplete_advisory.oxoflow	accept	TYPE-009	producer names no build, consumer names GRCh38 (advisory TYPE-009)
tests/truthset/biotype/cases/b20_unsorted_consumer_accept.oxoflow	accept	-	coordinate BAM -> sort_order=unsorted consumer (no ordering constraint)
examples/gallery/13_simple_variant_calling.oxoflow	accept	-	real repo example, untyped
examples/gallery/06_rnaseq_quantification.oxoflow	accept	-	real repo example, untyped
examples/gallery/09_single_cell_rnaseq.oxoflow	accept	-	real repo example, untyped
examples/gallery/07_wgs_germline.oxoflow	accept	-	real repo example, untyped
examples/gallery/08_multiomics_integration.oxoflow	accept	-	real repo example, untyped
examples/gallery/14_paired_experiment_control.oxoflow	accept	-	real repo example, untyped
examples/gallery/16_16s_qiime2_amplicon.oxoflow	accept	-	real repo example, untyped
examples/gallery/17_typed_variant_calling.oxoflow	accept	-	real repo example, TYPED [types.*]/[rules.types] contracts (#1272)
TABLE

if [[ -n "$JSON_OUT" ]]; then
  : > "$JSON_OUT"
  printf '{\n  "generated_at": "%s",\n  "commit": "%s",\n  "binary": "%s",\n  "cases": [\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$SHA" "$BIN" > "$JSON_OUT"
  printf '%s' '' >/dev/null
  first=1
fi

printf '%-55s %-7s %-7s %-6s %-6s %-20s %s\n' CASE EXPECT ACTUAL VERDICT CODES CODE-SET NOTE
printf '%.0s-' {1..140}; echo

ta=0; fa=0; tr=0; fr=0; codes_ok=0; codes_bad=0
while IFS=$'\t' read -r case exp codes note; do
  [[ -z "$case" ]] && continue
  # VERDICT surface: the REAL admission gate (fatal TYPE-00x only).
  out="$("$BIN" validate "$ROOT/$case" --json 2>/dev/null)"
  valid="$(printf '%s' "$out" | grep -o '"valid": *[a-z]*' | grep -o '[a-z]*$')"
  if [[ "$valid" == "true" ]]; then actual=accept; else actual=reject; fi
  if [[ "$exp" == "$actual" ]]; then correct=yes; else correct=NO; fi
  case "$exp/$actual" in
    accept/accept) ta=$((ta+1));;
    accept/reject) fr=$((fr+1));;
    reject/reject) tr=$((tr+1));;
    reject/accept) fa=$((fa+1));;
  esac
  # CODES cross-check: union of the TYPE-NNN codes on the admission surface
  # (validate: the fatal codes) and the full diagnostic surface (lint: which
  # additionally carries the advisory TYPE-008/009/010 the gate drops).
  seen_valid="$(printf '%s' "$out" | grep -o 'TYPE-[0-9][0-9][0-9]' | sort -u | paste -sd, -)"
  lint_out="$("$BIN" lint "$ROOT/$case" --json 2>/dev/null)"
  seen_lint="$(printf '%s' "$lint_out" | grep -o 'TYPE-[0-9][0-9][0-9]' | sort -u | paste -sd, -)"
  seen="$(printf '%s\n%s\n' "$seen_valid" "$seen_lint" | tr ',' '\n' | grep -v '^$' | sort -u | paste -sd, -)"
  codes="$(printf '%s' "$codes" | tr ',' '\n' | grep -v '^$' | sort -u | paste -sd, -)"
  [[ -z "$codes" ]] && codes='-'
  [[ -z "$seen" ]] && seen='-'
  if [[ "$codes" == "$seen" ]]; then codes_correct=yes; codes_ok=$((codes_ok+1)); else codes_correct=NO; codes_bad=$((codes_bad+1)); fi
  printf '%-55s %-7s %-7s %-6s %-6s %-20s %s\n' "$case" "$exp" "$actual" "$correct" "$codes_correct" "$seen" "$note"
  if [[ "$correct" == "yes" && "$codes_correct" == "NO" ]]; then
    printf '%-55s %s\n' "" "  ^ verdict-pass / codes-fail: expected codes [$codes], actual [$seen]"
  fi
  if [[ -n "$JSON_OUT" ]]; then
    [[ $first -eq 1 ]] || printf ',\n' >> "$JSON_OUT"
    first=0
    printf '    {"case": "%s", "expected": "%s", "actual": "%s", "verdict_correct": "%s", "expected_codes": "%s", "actual_codes": "%s", "codes_correct": "%s", "note": "%s"}' \
      "$case" "$exp" "$actual" "$correct" "$codes" "$seen" "$codes_correct" "$note" >> "$JSON_OUT"
  fi
done < "$EXPECT"
rm -f "$EXPECT"

echo
echo "2x2 confusion matrix (rows = expected, cols = actual) — VERDICT scoring:"
printf '  %-16s %-8s %-8s\n' "" accept reject
printf '  %-16s %-8d %-8d\n' EXPECT-ACCEPT "$ta" "$fr"
printf '  %-16s %-8d %-8d\n' EXPECT-REJECT "$fa" "$tr"
echo
echo "true-accept=$ta  false-accept=$fa  true-reject=$tr  false-reject=$fr"
echo
echo "diagnostic-code cross-check (expected-code set == actual-code set):"
echo "codes-correct=$codes_ok  codes-fail=$codes_bad  (of $((codes_ok+codes_bad)))"
echo "A verdict-pass / codes-fail case is a defect report about a MISSING or"
echo "UNEXPECTED diagnostic code, even though the accept/reject verdict was right."
echo
echo "A non-zero false-accept/false-reject cell is a defect report about the"
echo "checker, not a failure of this harness."

if [[ -n "$JSON_OUT" ]]; then
  printf '\n  ],\n  "confusion_matrix": {"true_accept": %d, "false_accept": %d, "true_reject": %d, "false_reject": %d},\n' \
    "$ta" "$fa" "$tr" "$fr" >> "$JSON_OUT"
  printf '  "codes_correct": %d,\n  "codes_fail": %d,\n  "cases_total": %d\n}\n' \
    "$codes_ok" "$codes_bad" "$((codes_ok+codes_bad))" >> "$JSON_OUT"
  echo "wrote machine-readable results to: $JSON_OUT"
fi
