#!/usr/bin/env bash
# biotype-truth-set.sh — run the BioType truth set through the REAL CLI
# validation surface and print a 2x2 confusion matrix.
#
# Truth set (18 cases):
#   * 15 typed/adversarial cases under tests/truthset/biotype/cases/
#     (t01..t06 transplanted Nextflow/Snakemake format edges; a06..a14
#     hand-authored adversarial probes at the TYPE-code boundaries).
#   *  3 real repository .oxoflow files under examples/gallery/ that were
#     NOT written for the checker (all untyped, expected accept).
#
# Every typed case carries its pre-registered expected verdict in a header
# comment; the EXPECT table below is the mechanical source of truth for the
# matrix so it is computed, not eyeballed.
#
# Usage:  scripts/biotype-truth-set.sh [path-to-oxo-flow-binary]
# Exit:   0 always (a measurement, not a gate); prints the matrix + summary.
#
# Identity (ledger §8): EXPECT was frozen at the case-authoring commit (see
# the report's pre-registration timestamp). Re-running against a newer
# checker may change ACTUAL only.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${1:-$ROOT/target/debug/oxo-flow}"

if [[ ! -x "$BIN" ]]; then
  echo "oxo-flow binary not found/executable at: $BIN" >&2
  echo "build it first: cargo build -p oxo-flow-cli" >&2
  exit 2
fi

# repo-relative-path <TAB> expected(accept|reject) <TAB> expected-codes(comma|-) <TAB> note
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
tests/truthset/biotype/cases/a08_multichannel_default_mismatch.oxoflow	reject	TYPE-002	multi-out producer -> default-keyed consumer (FALSE-ACCEPT)
tests/truthset/biotype/cases/a09_multichannel_namekey_default.oxoflow	reject	TYPE-002	CONTROL for a08, path-keyed consumer
tests/truthset/biotype/cases/a10_build_alias_hg38_grch38.oxoflow	accept	-	hg38 alias == GRCh38
tests/truthset/biotype/cases/a11_multichannel_namekey_mismatch.oxoflow	reject	TYPE-002	multi-out named producer -> named consumer (FALSE-ACCEPT)
tests/truthset/biotype/cases/a12_bam_implicit_bgzip.oxoflow	accept	-	plain BAM vs consumer bgzip requirement (FALSE-REJECT)
tests/truthset/biotype/cases/a13_somatic_to_germline_consumer.oxoflow	reject	TYPE-007	somatic VCF -> germline joint-calling
tests/truthset/biotype/cases/a14_typed_consumer_untyped_producer.oxoflow	accept	-	TYPE-008 advisory edge must not fail
examples/gallery/13_simple_variant_calling.oxoflow	accept	-	real repo example, untyped
examples/gallery/06_rnaseq_quantification.oxoflow	accept	-	real repo example, untyped
examples/gallery/09_single_cell_rnaseq.oxoflow	accept	-	real repo example, untyped
TABLE

printf '%-58s %-8s %-8s %-7s %-22s %s\n' CASE EXPECT ACTUAL CORRECT CODES NOTE
printf '%.0s-' {1..130}; echo

ta=0; fa=0; tr=0; fr=0
while IFS=$'\t' read -r case exp codes note; do
  [[ -z "$case" ]] && continue
  out="$("$BIN" validate "$ROOT/$case" --json 2>/dev/null)"
  valid="$(printf '%s' "$out" | grep -o '"valid": *[a-z]*' | grep -o '[a-z]*$')"
  if [[ "$valid" == "true" ]]; then actual=accept; else actual=reject; fi
  seen="$(printf '%s' "$out" | grep -o 'TYPE-[0-9][0-9][0-9]' | sort -u | paste -sd, -)"
  if [[ "$exp" == "$actual" ]]; then correct=yes; else correct=NO; fi
  case "$exp/$actual" in
    accept/accept) ta=$((ta+1));;
    accept/reject) fr=$((fr+1));;
    reject/reject) tr=$((tr+1));;
    reject/accept) fa=$((fa+1));;
  esac
  printf '%-58s %-8s %-8s %-7s %-22s %s\n' "$case" "$exp" "$actual" "$correct" "${seen:--}" "$note"
done < "$EXPECT"
rm -f "$EXPECT"

echo
echo "2x2 confusion matrix (rows = expected, cols = actual):"
printf '  %-16s %-8s %-8s\n' "" accept reject
printf '  %-16s %-8d %-8d\n' EXPECT-ACCEPT "$ta" "$fr"
printf '  %-16s %-8d %-8d\n' EXPECT-REJECT "$fa" "$tr"
echo
echo "true-accept=$ta  false-accept=$fa  true-reject=$tr  false-reject=$fr"
echo "A non-zero false-accept/false-reject cell is a defect report about the"
echo "checker, not a failure of this harness."
