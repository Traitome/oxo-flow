//! BioTypes: semantic biological typing and compile-time DAG contract verification.
//!
//! BioTypes introduces biological data contracts (format, sort order, index accompaniment,
//! reference genome build, compression) across workflow rules, enabling compile-time
//! verification and automated adapter rule synthesis.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

pub mod checker;
pub mod inspect;
#[cfg(test)]
mod inspect_tests;
#[cfg(test)]
mod tests;

pub use checker::{TypeDiagnostic, check_workflow_types};
pub use inspect::{
    IntrospectError, IntrospectedBioType, introspect_bytes, introspect_file,
    introspect_path_with_spec,
};

/// Standard biological file format category.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum BioFormat {
    /// FASTQ sequencing reads.
    Fastq,
    /// Sequence Alignment/Map (plain text SAM).
    Sam,
    /// Binary Alignment/Map (BAM).
    Bam,
    /// Compressed Reference-oriented Alignment Map (CRAM).
    Cram,
    /// Variant Call Format (VCF).
    Vcf,
    /// Binary Variant Call Format (BCF).
    Bcf,
    /// Browser Extensible Data (BED).
    Bed,
    /// Gene Transfer Format (GTF).
    Gtf,
    /// General Feature Format version 3 (GFF3).
    Gff3,
    /// FASTA reference or sequence file.
    Fasta,
    /// Single-cell AnnData HDF5 storage (H5AD).
    H5ad,
    /// R data serialization format (RDS).
    Rds,
    /// Apache Parquet columnar storage.
    Parquet,
    /// Tab-separated tabular values (TSV).
    Tsv,
    /// Generic unconstrained biological data.
    #[default]
    Generic,
}

impl BioFormat {
    /// Returns canonical format identifier.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fastq => "fastq",
            Self::Sam => "sam",
            Self::Bam => "bam",
            Self::Cram => "cram",
            Self::Vcf => "vcf",
            Self::Bcf => "bcf",
            Self::Bed => "bed",
            Self::Gtf => "gtf",
            Self::Gff3 => "gff3",
            Self::Fasta => "fasta",
            Self::H5ad => "h5ad",
            Self::Rds => "rds",
            Self::Parquet => "parquet",
            Self::Tsv => "tsv",
            Self::Generic => "generic",
        }
    }
}

impl fmt::Display for BioFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for BioFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fastq" | "fq" => Ok(Self::Fastq),
            "sam" => Ok(Self::Sam),
            "bam" => Ok(Self::Bam),
            "cram" => Ok(Self::Cram),
            "vcf" => Ok(Self::Vcf),
            "bcf" => Ok(Self::Bcf),
            "bed" => Ok(Self::Bed),
            "gtf" => Ok(Self::Gtf),
            "gff" | "gff3" => Ok(Self::Gff3),
            "fasta" | "fa" | "fna" => Ok(Self::Fasta),
            "h5ad" => Ok(Self::H5ad),
            "rds" => Ok(Self::Rds),
            "parquet" => Ok(Self::Parquet),
            "tsv" | "txt" => Ok(Self::Tsv),
            "generic" => Ok(Self::Generic),
            other => Err(format!("unknown biological format: '{other}'")),
        }
    }
}

/// Record ordering within genomic alignment or variant files.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    /// Natural or unsorted read stream.
    #[default]
    Unsorted,
    /// Grouped by query template name.
    Queryname,
    /// Ordered by genomic contig and coordinate offset.
    Coordinate,
    /// Unknown or arbitrary sorting.
    Unknown,
}

impl SortOrder {
    /// Check whether this sort order satisfies an expected sort order requirement.
    #[must_use]
    pub fn satisfies(&self, expected: SortOrder) -> bool {
        match expected {
            SortOrder::Unsorted | SortOrder::Unknown => true,
            SortOrder::Coordinate => *self == SortOrder::Coordinate,
            SortOrder::Queryname => *self == SortOrder::Queryname,
        }
    }

    /// Returns string identifier.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unsorted => "unsorted",
            Self::Queryname => "queryname",
            Self::Coordinate => "coordinate",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for SortOrder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for SortOrder {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "unsorted" => Ok(Self::Unsorted),
            "queryname" | "query_name" | "qname" => Ok(Self::Queryname),
            "coordinate" | "coord" => Ok(Self::Coordinate),
            "unknown" => Ok(Self::Unknown),
            other => Err(format!("unknown sort order: '{other}'")),
        }
    }
}

/// Associated index file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IndexKind {
    /// BAM index (.bai).
    Bai,
    /// Coordinate-sorted index for large chromosomes (.csi).
    Csi,
    /// CRAM index (.crai).
    Crai,
    /// Tabix index for tab-delimited genomic data (.tbi).
    Tbi,
    /// FASTA sequence index (.fai).
    Fai,
}

impl IndexKind {
    /// Returns string identifier.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bai => "bai",
            Self::Csi => "csi",
            Self::Crai => "crai",
            Self::Tbi => "tbi",
            Self::Fai => "fai",
        }
    }
}

impl fmt::Display for IndexKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for IndexKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "bai" => Ok(Self::Bai),
            "csi" => Ok(Self::Csi),
            "crai" => Ok(Self::Crai),
            "tbi" => Ok(Self::Tbi),
            "fai" => Ok(Self::Fai),
            other => Err(format!("unknown index format: '{other}'")),
        }
    }
}

/// Data compression stream encoding.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum CompressionKind {
    /// Plain uncompressed stream.
    #[default]
    None,
    /// Standard RFC 1952 DEFLATE gzip.
    Gzip,
    /// Block GZIP format (BGZF) allowing random-access seek.
    Bgzip,
    /// Facebook Zstandard compression.
    Zstd,
}

impl CompressionKind {
    /// Check whether this compression satisfies expected compression requirement.
    /// Note that BGZF is valid GZIP, but standard GZIP does not satisfy BGZF requirements.
    #[must_use]
    pub fn satisfies(&self, expected: CompressionKind) -> bool {
        match expected {
            CompressionKind::None => *self == CompressionKind::None,
            CompressionKind::Gzip => {
                *self == CompressionKind::Gzip || *self == CompressionKind::Bgzip
            }
            CompressionKind::Bgzip => *self == CompressionKind::Bgzip,
            CompressionKind::Zstd => *self == CompressionKind::Zstd,
        }
    }

    /// Returns string identifier.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Gzip => "gzip",
            Self::Bgzip => "bgzip",
            Self::Zstd => "zstd",
        }
    }
}

impl fmt::Display for CompressionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for CompressionKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "plain" | "" => Ok(Self::None),
            "gzip" | "gz" => Ok(Self::Gzip),
            "bgzip" | "bgz" => Ok(Self::Bgzip),
            "zstd" | "zst" => Ok(Self::Zstd),
            other => Err(format!("unknown compression format: '{other}'")),
        }
    }
}

/// Read group contract specification for alignment files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReadGroupContract {
    /// Whether valid `@RG` headers are mandatory for this type.
    #[serde(default)]
    pub required: bool,
    /// Specific `@RG` fields required (e.g. `["ID", "SM", "PL"]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<String>,
}

/// Biological semantic type specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BioTypeSpec {
    /// Biological format classification.
    pub format: BioFormat,
    /// Record sorting discipline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<SortOrder>,
    /// Companion index file requirement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<IndexKind>,
    /// Target genome assembly / reference build (e.g. "GRCh38", "hg19").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_build: Option<String>,
    /// Compression stream wrapper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compression: Option<CompressionKind>,
    /// Read group header contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_groups: Option<ReadGroupContract>,
    /// Extensible biological domain metadata attributes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
}

impl BioTypeSpec {
    /// Create a new type specification with a basic format.
    #[must_use]
    pub fn new(format: BioFormat) -> Self {
        Self {
            format,
            ..Default::default()
        }
    }

    /// Canonical BAM with coordinate sort and BAI index.
    #[must_use]
    pub fn bam_coordinate(reference_build: impl Into<String>) -> Self {
        Self {
            format: BioFormat::Bam,
            sort_order: Some(SortOrder::Coordinate),
            index: Some(IndexKind::Bai),
            reference_build: Some(reference_build.into()),
            compression: Some(CompressionKind::Bgzip),
            ..Default::default()
        }
    }

    /// Canonical paired FASTQ reads with gzip compression.
    #[must_use]
    pub fn fastq_gzip() -> Self {
        Self {
            format: BioFormat::Fastq,
            compression: Some(CompressionKind::Gzip),
            ..Default::default()
        }
    }

    /// Canonical indexed VCF with tabix index.
    #[must_use]
    pub fn vcf_indexed(reference_build: impl Into<String>) -> Self {
        Self {
            format: BioFormat::Vcf,
            index: Some(IndexKind::Tbi),
            reference_build: Some(reference_build.into()),
            compression: Some(CompressionKind::Bgzip),
            ..Default::default()
        }
    }
}

/// Rule-level biological type declarations.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RuleTypesSpec {
    /// Input channel -> type name or inline type spec.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, String>,
    /// Output channel -> type name or inline type spec.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, String>,
}

/// Specific violation identified during biological subtyping check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeViolation {
    /// Diagnostic identifier (`TYPE-001` .. `TYPE-006`).
    pub code: String,
    /// Clear explanation of the semantic mismatch.
    pub message: String,
    /// Actionable user suggestion to satisfy the contract.
    pub suggestion: Option<String>,
    /// Automated synthesis coercion rule template when applicable.
    pub coercion_rule: Option<String>,
}

/// Canonicalize reference build identifiers to resolve common nomenclature aliases.
#[must_use]
pub fn normalize_reference_build(build: &str) -> String {
    let s = build.trim().to_ascii_lowercase();
    match s.as_str() {
        "hg38" | "grch38" | "grch38.p14" | "hg38_no_alt" => "GRCh38".to_string(),
        "hg19" | "grch37" | "b37" | "human_g1k_v37" => "GRCh37".to_string(),
        "t2t" | "t2t-chm13" | "t2t_chm13" | "chm13" => "T2T-CHM13".to_string(),
        "mm10" | "grcm38" => "GRCm38".to_string(),
        "mm39" | "grcm39" => "GRCm39".to_string(),
        _ => build.trim().to_string(),
    }
}
