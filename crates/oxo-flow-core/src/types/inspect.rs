//! Sub-millisecond binary biological file header introspectors.
//!
//! Inspects initial 64 KiB file slices (local or remote object storage range reads)
//! to classify biological format, compression stream encoding, sort order, companion
//! index accompaniment, reference assembly build, and read group contracts without
//! reading entire files.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{
    BioFormat, BioTypeSpec, CompressionKind, IndexKind, SortOrder, TypeViolation,
    normalize_reference_build,
};

/// Bounded byte budget for header introspection: 64 KiB is sufficient to capture
/// BGZF metadata, BAM/CRAM/BCF magic headers, SAM `@HD`/`@SQ`/`@RG` blocks,
/// and VCF `##fileformat` metadata.
pub const HEADER_INSPECT_BUDGET: usize = 65536;

/// Error encountered during binary header introspection.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IntrospectError {
    /// Input byte slice or file was completely empty.
    #[error("file or header slice is empty (0 bytes)")]
    EmptyInput,
    /// I/O failure during file read.
    #[error("i/o error reading file: {0}")]
    Io(String),
    /// Format magic or framing was completely invalid.
    #[error("unrecognized biological file framing: {0}")]
    Unrecognized(String),
}

/// Extracted biological metadata from header introspection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct IntrospectedBioType {
    /// Classified biological format.
    pub format: BioFormat,
    /// Detected compression encoding.
    pub compression: CompressionKind,
    /// Declared or inferred sort order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<SortOrder>,
    /// Inferred or declared genome reference assembly build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_build: Option<String>,
    /// Parsed read group records (`@RG`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_groups: Vec<BTreeMap<String, String>>,
    /// Extracted sequence contig names (`@SQ SN` / `##contig=<ID=...>`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contigs: Vec<String>,
    /// Summary line of header metadata for diagnostic reporting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_header_summary: Option<String>,
}

impl IntrospectedBioType {
    /// Verify whether this introspected file satisfies an expected `BioTypeSpec`.
    pub fn satisfies_spec(&self, expected: &BioTypeSpec) -> Result<(), Vec<TypeViolation>> {
        let mut violations = Vec::new();

        // 1. Format check (TYPE-001)
        if self.format != expected.format && expected.format != BioFormat::Generic {
            violations.push(TypeViolation {
                code: "TYPE-001".to_string(),
                message: format!(
                    "biological format mismatch: file on disk is {}, but rule requires {}",
                    self.format, expected.format
                ),
                suggestion: Some(format!(
                    "convert {} to {} before pipeline execution",
                    self.format, expected.format
                )),
                coercion_rule: None,
            });
        }

        // 2. Sort order check (TYPE-002)
        if let Some(expected_order) = expected.sort_order
            && expected_order != SortOrder::Unsorted
            && expected_order != SortOrder::Unknown
        {
            let current_order = self.sort_order.unwrap_or(SortOrder::Unsorted);
            if !current_order.satisfies(expected_order) {
                violations.push(TypeViolation {
                    code: "TYPE-002".to_string(),
                    message: format!(
                        "sort order mismatch: file header declares {}, but rule requires {}",
                        current_order, expected_order
                    ),
                    suggestion: Some(format!(
                        "sort alignments by {} using samtools sort",
                        expected_order
                    )),
                    coercion_rule: Some("samtools sort -o {output} {input}".to_string()),
                });
            }
        }

        // 3. Reference build compatibility (TYPE-004)
        if let (Some(my_build), Some(expected_build)) =
            (&self.reference_build, &expected.reference_build)
        {
            let norm_my = normalize_reference_build(my_build);
            let norm_expected = normalize_reference_build(expected_build);
            if norm_my != norm_expected {
                violations.push(TypeViolation {
                    code: "TYPE-004".to_string(),
                    message: format!(
                        "reference build conflict: file header references '{}' ({}), but rule requires '{}' ({})",
                        my_build, norm_my, expected_build, norm_expected
                    ),
                    suggestion: Some(
                        "realign reads against matching reference genome build or perform coordinate liftover"
                            .to_string(),
                    ),
                    coercion_rule: None,
                });
            }
        }

        // 4. Read group contract (TYPE-005)
        if let Some(expected_rg) = &expected.read_groups
            && expected_rg.required
        {
            let has_required_rg = if self.read_groups.is_empty() {
                false
            } else if expected_rg.fields.is_empty() {
                true
            } else {
                self.read_groups.iter().any(|rg| {
                    expected_rg
                        .fields
                        .iter()
                        .all(|field| rg.contains_key(field.as_str()))
                })
            };

            if !has_required_rg {
                violations.push(TypeViolation {
                    code: "TYPE-005".to_string(),
                    message: "missing read group contract: rule requires valid @RG header tags, but file header lacks them"
                        .to_string(),
                    suggestion: Some(
                        "inject read groups with gatk AddOrReplaceReadGroups or samtools addreplacerg"
                            .to_string(),
                    ),
                    coercion_rule: Some(
                        "gatk AddOrReplaceReadGroups -I {input} -O {output} -RGID 1 -RGLB lib1 -RGPL ILLUMINA -RGPU unit1 -RGSM sample"
                            .to_string(),
                    ),
                });
            }
        }

        // 5. Compression stream check (TYPE-006)
        if let Some(expected_comp) = expected.compression
            && expected_comp != CompressionKind::None
            && !self.compression.satisfies(expected_comp)
        {
            violations.push(TypeViolation {
                code: "TYPE-006".to_string(),
                message: format!(
                    "compression mismatch: rule requires {}, but file is {}",
                    expected_comp, self.compression
                ),
                suggestion: Some(format!("compress file with {}", expected_comp)),
                coercion_rule: Some("bgzip -c {input} > {output}".to_string()),
            });
        }

        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }
}

/// Check if a gzip extra field contains the standard BGZF subfield (`BC`, len 2).
fn is_bgzf_extra(extra: &[u8]) -> bool {
    let mut offset = 0;
    while offset + 4 <= extra.len() {
        let si1 = extra[offset];
        let si2 = extra[offset + 1];
        let slen = u16::from_le_bytes([extra[offset + 2], extra[offset + 3]]) as usize;
        offset += 4;
        if si1 == b'B' && si2 == b'C' && slen == 2 {
            return true;
        }
        offset += slen;
    }
    false
}

/// Detect whether raw bytes begin with a standard GZIP or BGZF header.
fn detect_gzip_or_bgzf(bytes: &[u8]) -> Option<CompressionKind> {
    if bytes.len() < 10 || bytes[0] != 0x1f || bytes[1] != 0x8b || bytes[2] != 0x08 {
        return None;
    }
    let flags = bytes[3];
    let has_extra = (flags & 0x04) != 0;

    if has_extra && bytes.len() >= 12 {
        let xlen = u16::from_le_bytes([bytes[10], bytes[11]]) as usize;
        if bytes.len() >= 12 + xlen && is_bgzf_extra(&bytes[12..12 + xlen]) {
            return Some(CompressionKind::Bgzip);
        }
    }

    Some(CompressionKind::Gzip)
}

/// Introspect biological metadata from a bounded header byte slice.
pub fn introspect_bytes(bytes: &[u8]) -> Result<IntrospectedBioType, IntrospectError> {
    if bytes.is_empty() {
        return Err(IntrospectError::EmptyInput);
    }

    // Step 1: Detect compression stream
    let compression = detect_gzip_or_bgzf(bytes).unwrap_or(CompressionKind::None);

    // Step 2: Decompress initial payload if compressed
    let payload = match compression {
        CompressionKind::Gzip | CompressionKind::Bgzip => {
            let mut decoder = flate2::read::MultiGzDecoder::new(bytes);
            let mut decompressed = Vec::with_capacity(HEADER_INSPECT_BUDGET);
            let mut buffer = [0u8; 8192];
            while decompressed.len() < HEADER_INSPECT_BUDGET {
                match decoder.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => decompressed.extend_from_slice(&buffer[..n]),
                    Err(_) => {
                        // Truncated multi-block slice is expected when reading only initial 64 KiB
                        if decompressed.is_empty() {
                            return Err(IntrospectError::Unrecognized(
                                "corrupted or truncated gzip header slice".to_string(),
                            ));
                        }
                        break;
                    }
                }
            }
            decompressed
        }
        CompressionKind::None | CompressionKind::Zstd => bytes.to_vec(),
    };

    if payload.is_empty() {
        return Err(IntrospectError::EmptyInput);
    }

    // Step 3: Classify binary magic bytes
    if payload.len() >= 4 && &payload[..4] == b"BAM\x01" {
        return parse_bam_header(&payload, compression);
    }
    if payload.len() >= 4 && &payload[..4] == b"CRAM" {
        return parse_cram_header(&payload);
    }
    if payload.len() >= 4 && (&payload[..4] == b"BCF\x02" || &payload[..3] == b"BCF") {
        return parse_bcf_header(&payload, compression);
    }

    // Step 4: Text-based inspection (SAM, VCF, FASTQ, FASTA, BED, GTF/GFF3)
    let text = String::from_utf8_lossy(&payload);
    parse_text_header(&text, compression)
}

/// Parse uncompressed BAM header text following `BAM\x01`.
fn parse_bam_header(
    payload: &[u8],
    compression: CompressionKind,
) -> Result<IntrospectedBioType, IntrospectError> {
    if payload.len() < 8 {
        return Ok(IntrospectedBioType {
            format: BioFormat::Bam,
            compression,
            ..Default::default()
        });
    }

    let l_text = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;
    let end = 8 + l_text.min(payload.len().saturating_sub(8));
    let header_str = String::from_utf8_lossy(&payload[8..end]);

    let mut info = parse_sam_header_text(&header_str);
    info.format = BioFormat::Bam;
    info.compression = compression;
    Ok(info)
}

/// Parse CRAM magic and version.
fn parse_cram_header(payload: &[u8]) -> Result<IntrospectedBioType, IntrospectError> {
    let mut summary = "CRAM".to_string();
    if payload.len() >= 6 {
        summary = format!("CRAM v{}.{}", payload[4], payload[5]);
    }
    Ok(IntrospectedBioType {
        format: BioFormat::Cram,
        compression: CompressionKind::None,
        raw_header_summary: Some(summary),
        ..Default::default()
    })
}

/// Parse binary BCF header.
fn parse_bcf_header(
    payload: &[u8],
    compression: CompressionKind,
) -> Result<IntrospectedBioType, IntrospectError> {
    let mut info = IntrospectedBioType {
        format: BioFormat::Bcf,
        compression,
        ..Default::default()
    };

    if payload.len() >= 8 {
        let l_text = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;
        let end = 8 + l_text.min(payload.len().saturating_sub(8));
        let header_text = String::from_utf8_lossy(&payload[8..end]);
        let parsed = parse_vcf_header_text(&header_text);
        info.reference_build = parsed.reference_build;
        info.contigs = parsed.contigs;
        info.raw_header_summary = parsed.raw_header_summary;
    }

    Ok(info)
}

/// Parse plain text biological formats (SAM, VCF, FASTQ, FASTA, BED, GFF3, GTF).
fn parse_text_header(
    text: &str,
    compression: CompressionKind,
) -> Result<IntrospectedBioType, IntrospectError> {
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return Err(IntrospectError::EmptyInput);
    }

    // 1. VCF text format
    if trimmed.starts_with("##fileformat=VCF") {
        let mut info = parse_vcf_header_text(trimmed);
        info.compression = compression;
        return Ok(info);
    }

    // 2. SAM text format
    if trimmed.starts_with("@HD") || trimmed.starts_with("@SQ") || trimmed.starts_with("@RG") {
        let mut info = parse_sam_header_text(trimmed);
        info.format = BioFormat::Sam;
        info.compression = compression;
        return Ok(info);
    }

    // 3. FASTQ format (4-line framing cycle: @id, seq, +, qual)
    if trimmed.starts_with('@') && is_fastq_framing(trimmed) {
        return Ok(IntrospectedBioType {
            format: BioFormat::Fastq,
            compression,
            raw_header_summary: Some("FASTQ sequencing reads".to_string()),
            ..Default::default()
        });
    }

    // 4. FASTA format
    if trimmed.starts_with('>') {
        let first_line = trimmed.lines().next().unwrap_or("").trim_start_matches('>');
        return Ok(IntrospectedBioType {
            format: BioFormat::Fasta,
            compression,
            raw_header_summary: Some(format!("FASTA: {}", first_line)),
            ..Default::default()
        });
    }

    // 5. GFF3 / GTF
    if trimmed.starts_with("##gff-version") || is_gtf_gff_table(trimmed) {
        let format = if trimmed.contains("gene_id") || trimmed.contains("transcript_id") {
            BioFormat::Gtf
        } else {
            BioFormat::Gff3
        };
        return Ok(IntrospectedBioType {
            format,
            compression,
            ..Default::default()
        });
    }

    // 6. BED (interval table with chr, start, end)
    if is_bed_table(trimmed) {
        return Ok(IntrospectedBioType {
            format: BioFormat::Bed,
            compression,
            raw_header_summary: Some("BED intervals".to_string()),
            ..Default::default()
        });
    }

    Ok(IntrospectedBioType {
        format: BioFormat::Generic,
        compression,
        ..Default::default()
    })
}

/// Verify 4-line FASTQ cycle framing and quality length agreement.
fn is_fastq_framing(text: &str) -> bool {
    let mut lines = text.lines();
    let Some(l1) = lines.next() else { return false };
    let Some(l2) = lines.next() else { return false };
    let Some(l3) = lines.next() else { return false };
    let Some(l4) = lines.next() else { return false };

    l1.starts_with('@')
        && l3.starts_with('+')
        && l2.len() == l4.len()
        && l2
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == '.' || c == '-')
}

/// Verify 9-column tab-delimited GFF/GTF table structure.
fn is_gtf_gff_table(text: &str) -> bool {
    for line in text.lines().take(5) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() >= 8 && cols[3].parse::<u64>().is_ok() && cols[4].parse::<u64>().is_ok() {
            return true;
        }
    }
    false
}

/// Verify BED table structure (at least chrom, start, end).
fn is_bed_table(text: &str) -> bool {
    for line in text.lines().take(5) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("track ") {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() >= 3 && cols[1].parse::<u64>().is_ok() && cols[2].parse::<u64>().is_ok() {
            return true;
        }
    }
    false
}

/// Parse SAM header lines (`@HD`, `@SQ`, `@RG`).
fn parse_sam_header_text(header: &str) -> IntrospectedBioType {
    let mut sort_order = None;
    let mut contigs = Vec::new();
    let mut contig_lens = Vec::new();
    let mut read_groups = Vec::new();
    let mut explicit_build = None;

    for line in header.lines() {
        if let Some(rest) = line.strip_prefix("@HD") {
            for tag in rest.split('\t') {
                if let Some(so) = tag.strip_prefix("SO:") {
                    sort_order = match so.trim() {
                        "coordinate" => Some(SortOrder::Coordinate),
                        "queryname" => Some(SortOrder::Queryname),
                        "unsorted" => Some(SortOrder::Unsorted),
                        _ => Some(SortOrder::Unknown),
                    };
                }
            }
        } else if let Some(rest) = line.strip_prefix("@SQ") {
            let mut sn = None;
            let mut ln = None;
            let mut as_build = None;
            for tag in rest.split('\t') {
                if let Some(val) = tag.strip_prefix("SN:") {
                    sn = Some(val.to_string());
                } else if let Some(val) = tag.strip_prefix("LN:") {
                    ln = val.parse::<u64>().ok();
                } else if let Some(val) = tag.strip_prefix("AS:") {
                    as_build = Some(val.to_string());
                }
            }
            if let Some(name) = sn {
                contigs.push(name);
            }
            if let Some(len) = ln {
                contig_lens.push(len);
            }
            if explicit_build.is_none() && as_build.is_some() {
                explicit_build = as_build;
            }
        } else if let Some(rest) = line.strip_prefix("@RG") {
            let mut rg_map = BTreeMap::new();
            for tag in rest.split('\t') {
                if let Some((k, v)) = tag.split_once(':') {
                    rg_map.insert(k.to_string(), v.to_string());
                }
            }
            if !rg_map.is_empty() {
                read_groups.push(rg_map);
            }
        }
    }

    let reference_build = explicit_build.or_else(|| infer_reference_build(&contigs, &contig_lens));

    IntrospectedBioType {
        format: BioFormat::Sam,
        compression: CompressionKind::None,
        sort_order,
        reference_build,
        read_groups,
        contigs,
        raw_header_summary: Some(format!("SAM header ({} contigs)", contig_lens.len())),
    }
}

/// Parse VCF header lines (`##fileformat`, `##reference`, `##contig`).
fn parse_vcf_header_text(header: &str) -> IntrospectedBioType {
    let mut contigs = Vec::new();
    let mut contig_lens = Vec::new();
    let mut explicit_build = None;

    for line in header.lines() {
        if !line.starts_with("##") {
            if line.starts_with("#CHROM") {
                break;
            }
            continue;
        }

        if let Some(ref_val) = line.strip_prefix("##reference=") {
            let val = ref_val.trim().trim_matches('"');
            if !val.is_empty() {
                explicit_build = Some(val.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("##contig=<") {
            let body = rest.trim_end_matches('>').trim_end();
            let mut id = None;
            let mut length = None;
            let mut as_tag = None;
            for part in body.split(',') {
                if let Some((k, v)) = part.split_once('=') {
                    match k.trim() {
                        "ID" => id = Some(v.trim().trim_matches('"').to_string()),
                        "length" => length = v.trim().parse::<u64>().ok(),
                        "assembly" => as_tag = Some(v.trim().trim_matches('"').to_string()),
                        _ => {}
                    }
                }
            }
            if let Some(name) = id {
                contigs.push(name);
            }
            if let Some(len) = length {
                contig_lens.push(len);
            }
            if explicit_build.is_none() && as_tag.is_some() {
                explicit_build = as_tag;
            }
        }
    }

    let reference_build = explicit_build.or_else(|| infer_reference_build(&contigs, &contig_lens));

    IntrospectedBioType {
        format: BioFormat::Vcf,
        compression: CompressionKind::None,
        sort_order: Some(SortOrder::Coordinate),
        reference_build,
        read_groups: Vec::new(),
        contigs,
        raw_header_summary: Some("VCF header".to_string()),
    }
}

/// Infer reference genome assembly from chromosome length fingerprints.
fn infer_reference_build(contigs: &[String], lengths: &[u64]) -> Option<String> {
    for (&len, name) in lengths.iter().zip(contigs.iter()) {
        let lower = name.to_ascii_lowercase();
        if lower == "chr1" || lower == "1" {
            match len {
                248956422 => return Some("GRCh38".to_string()),
                249250621 => return Some("GRCh37".to_string()),
                248387328 => return Some("T2T-CHM13".to_string()),
                195471971 => return Some("GRCm38".to_string()),
                195154279 => return Some("GRCm39".to_string()),
                _ => {}
            }
        }
    }
    None
}

/// Introspect a local file on disk using a bounded initial read.
pub fn introspect_file(path: &Path) -> Result<IntrospectedBioType, IntrospectError> {
    let mut file = File::open(path).map_err(|e| IntrospectError::Io(e.to_string()))?;
    let mut buffer = vec![0u8; HEADER_INSPECT_BUDGET];
    let n = file
        .read(&mut buffer)
        .map_err(|e| IntrospectError::Io(e.to_string()))?;
    buffer.truncate(n);
    introspect_bytes(&buffer)
}

/// Introspect a file and verify its compliance against an expected `BioTypeSpec`,
/// additionally checking companion index existence on disk.
pub fn introspect_path_with_spec(
    path: &Path,
    expected: &BioTypeSpec,
) -> Result<IntrospectedBioType, Vec<TypeViolation>> {
    let mut violations = Vec::new();

    let introspected = match introspect_file(path) {
        Ok(t) => t,
        Err(e) => {
            violations.push(TypeViolation {
                code: "TYPE-001".to_string(),
                message: format!("failed to introspect header of '{}': {}", path.display(), e),
                suggestion: Some("verify file integrity and permissions".to_string()),
                coercion_rule: None,
            });
            return Err(violations);
        }
    };

    if let Err(mut v) = introspected.satisfies_spec(expected) {
        violations.append(&mut v);
    }

    // Check companion index accompaniment on disk (TYPE-003)
    if let Some(expected_index) = expected.index {
        let has_index = check_index_exists_on_disk(path, expected_index);
        if !has_index {
            let coercion_rule = match expected_index {
                IndexKind::Bai | IndexKind::Csi => Some("samtools index {input}".to_string()),
                IndexKind::Crai => Some("samtools index {input}".to_string()),
                IndexKind::Tbi => Some("tabix -p vcf {input}".to_string()),
                IndexKind::Fai => Some("samtools faidx {input}".to_string()),
            };
            violations.push(TypeViolation {
                code: "TYPE-003".to_string(),
                message: format!(
                    "missing companion index: expected '{}' for file '{}' was not found on disk",
                    expected_index,
                    path.display()
                ),
                suggestion: Some(format!(
                    "create companion index for {} using index tool",
                    expected.format
                )),
                coercion_rule,
            });
        }
    }

    if violations.is_empty() {
        Ok(introspected)
    } else {
        Err(violations)
    }
}

/// Check if companion index file exists adjacent to target file on disk.
fn check_index_exists_on_disk(path: &Path, index: IndexKind) -> bool {
    let ext = index.as_str();
    let direct_ext = path.with_extension(ext);
    if direct_ext.exists() {
        return true;
    }
    let appended = format!("{}.{}", path.display(), ext);
    if Path::new(&appended).exists() {
        return true;
    }
    // Specific aliases: .bam.bai vs .bai, .vcf.gz.tbi vs .tbi
    match index {
        IndexKind::Bai => {
            let s = format!("{}.bai", path.display());
            Path::new(&s).exists()
        }
        IndexKind::Crai => {
            let s = format!("{}.crai", path.display());
            Path::new(&s).exists()
        }
        IndexKind::Tbi => {
            let s = format!("{}.tbi", path.display());
            Path::new(&s).exists()
        }
        IndexKind::Fai => {
            let s = format!("{}.fai", path.display());
            Path::new(&s).exists()
        }
        IndexKind::Csi => {
            let s = format!("{}.csi", path.display());
            Path::new(&s).exists()
        }
    }
}
