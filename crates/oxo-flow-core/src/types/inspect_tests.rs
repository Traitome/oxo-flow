//! Unit tests for sub-millisecond biological binary header introspectors.

use std::fs::File;
use std::io::Write;
use std::time::Instant;

use tempfile::tempdir;

use super::inspect::*;
use super::*;

#[test]
fn test_introspect_sam_text() {
    let sam_header = "@HD\tVN:1.6\tSO:coordinate\n@SQ\tSN:chr1\tLN:248956422\tAS:GRCh38\n@RG\tID:RG1\tSM:SAMPLE1\tPL:ILLUMINA\n";
    let info = introspect_bytes(sam_header.as_bytes()).expect("valid SAM header");

    assert_eq!(info.format, BioFormat::Sam);
    assert_eq!(info.sort_order, Some(SortOrder::Coordinate));
    assert_eq!(info.reference_build.as_deref(), Some("GRCh38"));
    assert_eq!(info.contigs, vec!["chr1"]);
    assert_eq!(info.read_groups.len(), 1);
    assert_eq!(
        info.read_groups[0].get("ID").map(String::as_str),
        Some("RG1")
    );
    assert_eq!(
        info.read_groups[0].get("SM").map(String::as_str),
        Some("SAMPLE1")
    );
}

#[test]
fn test_introspect_vcf_text() {
    let vcf_header = "##fileformat=VCFv4.2\n##reference=\"GRCh38\"\n##contig=<ID=chr1,length=248956422,assembly=GRCh38>\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n";
    let info = introspect_bytes(vcf_header.as_bytes()).expect("valid VCF header");

    assert_eq!(info.format, BioFormat::Vcf);
    assert_eq!(info.sort_order, Some(SortOrder::Coordinate));
    assert_eq!(info.reference_build.as_deref(), Some("GRCh38"));
    assert_eq!(info.contigs, vec!["chr1"]);
}

#[test]
fn test_introspect_fastq_text() {
    let fq = "@READ_1/1\nACGTACGT\n+\nIIIIIIII\n@READ_2/1\nTTTTCCCC\n+\nJJJJJJJJ\n";
    let info = introspect_bytes(fq.as_bytes()).expect("valid FASTQ");

    assert_eq!(info.format, BioFormat::Fastq);
    assert_eq!(info.compression, CompressionKind::None);
}

#[test]
fn test_introspect_fasta_text() {
    let fa = ">chr1 human chromosome 1\nACGTACGTACGT\n";
    let info = introspect_bytes(fa.as_bytes()).expect("valid FASTA");

    assert_eq!(info.format, BioFormat::Fasta);
    assert!(info.raw_header_summary.as_ref().unwrap().contains("chr1"));
}

#[test]
fn test_introspect_cram_magic() {
    let mut cram_bytes = vec![b'C', b'R', b'A', b'M', 3, 0];
    cram_bytes.resize(128, 0);
    let info = introspect_bytes(&cram_bytes).expect("valid CRAM magic");

    assert_eq!(info.format, BioFormat::Cram);
    assert!(info.raw_header_summary.as_ref().unwrap().contains("v3.0"));
}

#[test]
fn test_introspect_bed_text() {
    let bed = "chr1\t1000\t2000\tregion1\nchr1\t3000\t4000\tregion2\n";
    let info = introspect_bytes(bed.as_bytes()).expect("valid BED");

    assert_eq!(info.format, BioFormat::Bed);
}

#[test]
fn test_introspect_bgzf_bam_synthetic() {
    // Construct uncompressed BAM payload: magic + l_text + sam_text
    let sam_text =
        "@HD\tVN:1.6\tSO:coordinate\n@SQ\tSN:1\tLN:249250621\n@RG\tID:rgA\tSM:sA\tPL:ILLUMINA\n";
    let mut payload = Vec::new();
    payload.extend_from_slice(b"BAM\x01");
    let l_text = sam_text.len() as u32;
    payload.extend_from_slice(&l_text.to_le_bytes());
    payload.extend_from_slice(sam_text.as_bytes());

    // Compress with flate2 (gzip)
    let mut gz_encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz_encoder.write_all(&payload).unwrap();
    let compressed = gz_encoder.finish().unwrap();

    let info = introspect_bytes(&compressed).expect("valid compressed BAM");
    assert_eq!(info.format, BioFormat::Bam);
    assert_eq!(info.sort_order, Some(SortOrder::Coordinate));
    // Fingerprint length 249250621 maps to GRCh37
    assert_eq!(info.reference_build.as_deref(), Some("GRCh37"));
    assert_eq!(info.read_groups.len(), 1);
    assert_eq!(
        info.read_groups[0].get("ID").map(String::as_str),
        Some("rgA")
    );
}

#[test]
fn test_introspect_path_with_spec_and_index_check() {
    let dir = tempdir().unwrap();
    let bam_path = dir.path().join("sample.bam");
    let bai_path = dir.path().join("sample.bam.bai");

    // Write minimal SAM/BAM text
    let sam_text =
        "@HD\tVN:1.6\tSO:coordinate\n@SQ\tSN:chr1\tLN:248956422\n@RG\tID:1\tSM:s1\tPL:ILLUMINA\n";
    let mut payload = Vec::new();
    payload.extend_from_slice(b"BAM\x01");
    let l_text = sam_text.len() as u32;
    payload.extend_from_slice(&l_text.to_le_bytes());
    payload.extend_from_slice(sam_text.as_bytes());

    let mut f = File::create(&bam_path).unwrap();
    f.write_all(&payload).unwrap();

    let expected = BioTypeSpec {
        format: BioFormat::Bam,
        sort_order: Some(SortOrder::Coordinate),
        index: Some(IndexKind::Bai),
        reference_build: Some("GRCh38".to_string()),
        read_groups: Some(ReadGroupContract {
            required: true,
            fields: vec!["ID".to_string(), "SM".to_string()],
        }),
        ..Default::default()
    };

    // Missing BAI index should fire TYPE-003
    let err = introspect_path_with_spec(&bam_path, &expected).unwrap_err();
    assert_eq!(err.len(), 1);
    assert_eq!(err[0].code, "TYPE-003");

    // Create BAI companion index file
    File::create(&bai_path).unwrap();

    // Now inspection succeeds completely
    let start = Instant::now();
    let res = introspect_path_with_spec(&bam_path, &expected);
    let elapsed = start.elapsed();
    assert!(res.is_ok());
    // Guarantee sub-millisecond execution
    assert!(
        elapsed.as_millis() < 10,
        "header inspection must be sub-millisecond"
    );
}
