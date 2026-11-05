//! Benchmark: Biological binary header introspection performance.
//!
//! Target: <1ms per file, sub-millisecond bounded initial 64 KiB read.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::io::Write;
use tempfile::NamedTempFile;

use oxo_flow_core::types::inspect::{introspect_bytes, introspect_file, introspect_path_with_spec};
use oxo_flow_core::types::{BioFormat, BioTypeSpec, IndexKind, SortOrder};

fn bench_header_inspection(c: &mut Criterion) {
    let mut group = c.benchmark_group("header_inspection");

    let sam_header = "@HD\tVN:1.6\tSO:coordinate\n@SQ\tSN:chr1\tLN:248956422\tAS:GRCh38\n@RG\tID:RG1\tSM:SAMPLE1\tPL:ILLUMINA\n";
    let vcf_header = "##fileformat=VCFv4.2\n##reference=\"GRCh38\"\n##contig=<ID=chr1,length=248956422,assembly=GRCh38>\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n";
    let fq_text = "@READ_1/1\nACGTACGT\n+\nIIIIIIII\n@READ_2/1\nTTTTCCCC\n+\nJJJJJJJJ\n";

    // Synthetic BAM payload: BAM\x01 + l_text + sam_header
    let mut bam_payload = Vec::new();
    bam_payload.extend_from_slice(b"BAM\x01");
    let l_text = sam_header.len() as u32;
    bam_payload.extend_from_slice(&l_text.to_le_bytes());
    bam_payload.extend_from_slice(sam_header.as_bytes());

    let mut gz_encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz_encoder.write_all(&bam_payload).unwrap();
    let compressed_bam = gz_encoder.finish().unwrap();

    // Create temporary file for file-based inspection
    let mut tmp_file = NamedTempFile::new().unwrap();
    tmp_file.write_all(&compressed_bam).unwrap();
    tmp_file.flush().unwrap();
    let tmp_path = tmp_file.path().to_path_buf();

    // Temporary index file
    let bai_path = tmp_path.with_extension("bam.bai");
    std::fs::File::create(&bai_path).unwrap();

    let expected_spec = BioTypeSpec {
        format: BioFormat::Bam,
        sort_order: Some(SortOrder::Coordinate),
        index: Some(IndexKind::Bai),
        reference_build: Some("GRCh38".to_string()),
        ..Default::default()
    };

    group.bench_function(BenchmarkId::new("introspect_bytes", "sam_text"), |b| {
        b.iter(|| {
            let res = introspect_bytes(black_box(sam_header.as_bytes()));
            black_box(res)
        })
    });

    group.bench_function(BenchmarkId::new("introspect_bytes", "vcf_text"), |b| {
        b.iter(|| {
            let res = introspect_bytes(black_box(vcf_header.as_bytes()));
            black_box(res)
        })
    });

    group.bench_function(BenchmarkId::new("introspect_bytes", "fastq_text"), |b| {
        b.iter(|| {
            let res = introspect_bytes(black_box(fq_text.as_bytes()));
            black_box(res)
        })
    });

    group.bench_function(
        BenchmarkId::new("introspect_bytes", "compressed_bam"),
        |b| {
            b.iter(|| {
                let res = introspect_bytes(black_box(&compressed_bam));
                black_box(res)
            })
        },
    );

    group.bench_function(BenchmarkId::new("introspect_file", "compressed_bam"), |b| {
        b.iter(|| {
            let res = introspect_file(black_box(&tmp_path));
            black_box(res)
        })
    });

    group.bench_function(
        BenchmarkId::new("introspect_path_with_spec", "bam_with_bai"),
        |b| {
            b.iter(|| {
                let res =
                    introspect_path_with_spec(black_box(&tmp_path), black_box(&expected_spec));
                black_box(res)
            })
        },
    );

    group.finish();
}

criterion_group!(benches, bench_header_inspection);
criterion_main!(benches);
