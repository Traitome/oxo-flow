//! Data discovery service — deterministic file format detection
//! and reference genome component discovery.
//!
//! Zero AI. 100% deterministic via extension matching and path traversal.

use crate::domains::observability::types::*;

/// Known bioinformatics file formats and their extensions.
static FORMAT_MAP: &[(&str, &str)] = &[
    (".fastq.gz", "FASTQ (gzipped)"),
    (".fq.gz", "FASTQ (gzipped)"),
    (".fastq", "FASTQ"),
    (".fq", "FASTQ"),
    (".bam", "BAM"),
    (".cram", "CRAM"),
    (".vcf.gz", "VCF (gzipped)"),
    (".vcf", "VCF"),
    (".bcf", "BCF"),
    (".bed", "BED"),
    (".gff", "GFF"),
    (".gtf", "GTF"),
    (".sam", "SAM"),
    (".bw", "BigWig"),
    (".bigwig", "BigWig"),
    (".tsv", "TSV"),
    (".csv", "CSV"),
    (".txt", "Text"),
];

/// The format label for extensions outside [`FORMAT_MAP`].
const UNKNOWN_FORMAT: &str = "unknown";
///
/// Detects formats via extension matching, identifies paired-end naming
/// patterns, and suggests an appropriate workflow template.
///
/// `scope` (#521): when set, every pattern must be relative (no `..`) and
/// is probed at `scope/<pattern>` — the client cannot use this endpoint to
/// stat arbitrary server paths. Responses still echo the client-supplied
/// path text, never the resolved server location.
pub fn analyze_files(
    paths: &[String],
    _max_depth: Option<usize>,
    scope: Option<&std::path::Path>,
) -> Result<DataAnalysisResponse, String> {
    let mut files = Vec::new();
    let mut formats = std::collections::HashSet::new();

    for pattern in paths {
        let path = match scope {
            Some(root) => {
                crate::workspace::check_relative_path(pattern)
                    .map_err(|e| format!("invalid data path: {e}"))?;
                root.join(std::path::Path::new(pattern))
            }
            None => std::path::PathBuf::from(pattern),
        };
        let filename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let (format_name, confidence) = FORMAT_MAP
            .iter()
            .find(|(ext, _)| filename.ends_with(ext))
            .map(|(_, name)| (name.to_string(), 0.95))
            .unwrap_or_else(|| (UNKNOWN_FORMAT.to_string(), 0.0));

        formats.insert(format_name.clone());

        // Extract sample name (everything before the first underscore)
        let sample_name = filename.split('_').next().map(|s| s.to_string());

        files.push(FileInfo {
            path: pattern.clone(),
            size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            format: format_name,
            format_confidence: confidence,
            paired_with: None,
            sample_name,
        });
    }

    let files = pair_files(files);

    // Derived from the same naming vocabulary as pairing (AGENTS.md: don't
    // mirror semantics — share them). Format/companion checks are pairing's
    // job; this flag only answers "do these names use a paired-end convention".
    let paired_end = files.iter().any(|f| mate_path(&f.path).is_some());

    let total_size: u64 = files.iter().map(|f| f.size).sum();

    let suggested = if formats.iter().any(|f| f.contains("FASTQ")) {
        Some(WorkflowSuggestion {
            template: if paired_end {
                "rnaseq-pe".into()
            } else {
                "qc".into()
            },
            confidence: if paired_end { 0.85 } else { 0.6 },
            reason: format!(
                "Detected FASTQ files{}",
                if paired_end {
                    " with paired-end naming"
                } else {
                    ""
                }
            ),
        })
    } else if formats.contains("BAM") {
        Some(WorkflowSuggestion {
            template: "variant-calling".into(),
            confidence: 0.7,
            reason: "BAM files detected".into(),
        })
    } else {
        None
    };

    Ok(DataAnalysisResponse {
        files,
        summary: DataSummary {
            total_size,
            formats_detected: formats.into_iter().collect(),
            paired_end_detected: paired_end,
            strand_specific: None,
        },
        suggested_workflow: suggested,
    })
}

/// Return the expected mate path for a recognised paired-end naming
/// convention **in the file name**: `_R1`/`_R2` (a following suffix such as
/// `_001` is preserved; a digit immediately after the marker means the token
/// is not an R1/R2 marker — `_R10` is not `_R1`) or `_1.`/`_2.` anywhere in
/// the file name. Directory components are never interpreted.
fn mate_path(path: &str) -> Option<String> {
    let file_name = std::path::Path::new(path).file_name()?.to_str()?;
    let (marker, mate) = [
        ("_R1", "_R2"),
        ("_R2", "_R1"),
        ("_1.", "_2."),
        ("_2.", "_1."),
    ]
    .into_iter()
    .find(|(marker, _)| {
        let Some(index) = file_name.rfind(marker) else {
            return false;
        };
        // `_R10` (a digit follows the marker) is a sample-name token, not an
        // R1 marker; `_R1foo` still matches. The `_1.`/`_2.` forms carry
        // their separator, so any suffix is allowed (`_1.trimmed.fastq.gz`).
        if marker.starts_with("_R") {
            let next = file_name[index + marker.len()..].chars().next();
            return next.is_none_or(|c| !c.is_ascii_digit());
        }
        true
    })?;
    let index = file_name.rfind(marker)?;
    let mate_name = format!(
        "{}{mate}{}",
        &file_name[..index],
        &file_name[index + marker.len()..]
    );
    // `strip_suffix` rather than byte arithmetic: `Path::file_name()` ignores
    // trailing separators, so a path ending in `/` or `\` would otherwise
    // slice at a non-boundary (and panic on multi-byte names). Such a path is
    // malformed for a file input; no mate is the safe answer.
    let prefix = path.strip_suffix(file_name)?;
    Some(format!("{prefix}{mate_name}"))
}

/// Fill `FileInfo::paired_with` for mates present in the same request.
///
/// A mate is recorded only when its path is one of the analysed inputs, its
/// detected format matches and is not `unknown` (a `_R1.fastq.gz` is not
/// paired with a `_R2.bam`), and the claim is mutual — the mate must resolve
/// back to this file. The recorded value is the client-supplied path text,
/// matching `FileInfo::path`.
fn pair_files(files: Vec<FileInfo>) -> Vec<FileInfo> {
    let mates: Vec<Option<String>> = {
        let present: std::collections::HashMap<&str, &str> = files
            .iter()
            .map(|file| (file.path.as_str(), file.format.as_str()))
            .collect();
        files
            .iter()
            .map(|file| {
                if file.format == UNKNOWN_FORMAT {
                    return None;
                }
                let mate = mate_path(&file.path)?;
                let mate_format = present.get(mate.as_str())?;
                if *mate_format != file.format.as_str() {
                    return None;
                }
                // Asymmetric names (`a_R1_R2.fastq.gz`) can propose a mate
                // that does not claim us back; never publish such a pairing.
                (mate_path(&mate).as_deref() == Some(file.path.as_str())).then_some(mate)
            })
            .collect()
    };
    files
        .into_iter()
        .zip(mates)
        .map(|(file, paired_with)| FileInfo {
            paired_with,
            ..file
        })
        .collect()
}

/// Discover reference genome components for a given genome build.
///
/// Searches common reference directories and reports which components
/// are available and which are missing.
pub fn discover_reference(
    genome: &str,
    components: &[String],
) -> Result<ReferenceResponse, String> {
    let search_dirs = [
        format!("/data/references/{genome}"),
        format!("/reference/{genome}"),
        format!("./reference/{genome}"),
    ];

    let mut found = Vec::new();
    let mut missing = Vec::new();
    let mut download_cmds = Vec::new();

    for comp in components {
        let basename = match comp.as_str() {
            "fasta" => format!("{genome}.fa"),
            "gtf" => format!("{genome}.gtf"),
            "star_index" => format!("{genome}_star_index"),
            "bowtie2_index" => format!("{genome}_bowtie2_index"),
            "bwa_index" => format!("{genome}_bwa_index"),
            other => other.to_string(),
        };

        let exists = search_dirs
            .iter()
            .any(|dir| std::path::Path::new(&format!("{dir}/{basename}")).exists());

        if exists {
            found.push(comp.clone());
        } else {
            missing.push(comp.clone());
            match comp.as_str() {
                "fasta" => download_cmds.push(format!(
                    "wget ftp://ftp.ensembl.org/pub/release-110/fasta/homo_sapiens/dna/Homo_sapiens.{genome}.dna.primary_assembly.fa.gz"
                )),
                "gtf" => download_cmds.push(format!(
                    "wget ftp://ftp.ensembl.org/pub/release-110/gtf/homo_sapiens/Homo_sapiens.{genome}.110.gtf.gz"
                )),
                _ => {}
            }
        }
    }

    Ok(ReferenceResponse {
        found,
        missing,
        download_commands: download_cmds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unknown_format() {
        let result = analyze_files(&["/tmp/foo.xyz".into()], None, None).unwrap();
        assert_eq!(result.files[0].format, "unknown");
        assert_eq!(result.files[0].format_confidence, 0.0);
    }

    #[test]
    fn test_fastq_gz_detection() {
        let result = analyze_files(&["/data/sample_R1.fastq.gz".into()], None, None).unwrap();
        assert!(result.files[0].format.contains("FASTQ"));
        assert!(result.files[0].format_confidence > 0.9);
        assert!(result.summary.paired_end_detected);
    }

    #[test]
    fn paired_with_links_r1_r2_mates() {
        let r1 = "/data/s1_R1.fastq.gz".to_string();
        let r2 = "/data/s1_R2.fastq.gz".to_string();
        let result = analyze_files(&[r1.clone(), r2.clone()], None, None).unwrap();
        assert_eq!(result.files[0].paired_with.as_deref(), Some(r2.as_str()));
        assert_eq!(result.files[1].paired_with.as_deref(), Some(r1.as_str()));
    }

    #[test]
    fn paired_with_links_underscore_one_two_mates() {
        let r1 = "/data/s1_1.fastq.gz".to_string();
        let r2 = "/data/s1_2.fastq.gz".to_string();
        let result = analyze_files(&[r1.clone(), r2.clone()], None, None).unwrap();
        assert_eq!(result.files[0].paired_with.as_deref(), Some(r2.as_str()));
        assert_eq!(result.files[1].paired_with.as_deref(), Some(r1.as_str()));
    }

    #[test]
    fn unpaired_r1_records_no_mate() {
        let result = analyze_files(&["/data/s1_R1.fastq.gz".into()], None, None).unwrap();
        assert_eq!(result.files[0].paired_with, None);
    }

    #[test]
    fn paired_with_requires_the_same_detected_format() {
        let result = analyze_files(
            &["/data/s1_R1.fastq.gz".into(), "/data/s1_R2.bam".into()],
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.files[0].paired_with, None);
        assert_eq!(result.files[1].paired_with, None);
    }

    #[test]
    fn r10_names_are_not_r1_r2_mates() {
        let result = analyze_files(
            &[
                "/data/sample_R10.fastq.gz".into(),
                "/data/sample_R20.fastq.gz".into(),
            ],
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.files[0].paired_with, None);
        assert_eq!(result.files[1].paired_with, None);
    }

    #[test]
    fn r1_suffix_is_preserved_in_the_mate() {
        let r1 = "/data/s1_R1_001.fastq.gz".to_string();
        let r2 = "/data/s1_R2_001.fastq.gz".to_string();
        let result = analyze_files(&[r1.clone(), r2.clone()], None, None).unwrap();
        assert_eq!(result.files[0].paired_with.as_deref(), Some(r2.as_str()));
        assert_eq!(result.files[1].paired_with.as_deref(), Some(r1.as_str()));
    }

    #[test]
    fn directory_markers_are_not_pairing_markers() {
        let result = analyze_files(
            &[
                "/d/old_R1/reads.fastq.gz".into(),
                "/d/old_R2/reads.fastq.gz".into(),
            ],
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.files[0].paired_with, None);
        assert_eq!(result.files[1].paired_with, None);
        assert!(!result.summary.paired_end_detected);
    }

    #[test]
    fn asymmetric_pair_names_are_not_published() {
        // `a_R1_R2` proposes `a_R2_R2`, which does not claim it back.
        let result = analyze_files(
            &["/d/a_R1_R2.fastq.gz".into(), "/d/a_R2_R2.fastq.gz".into()],
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.files[0].paired_with, None);
        assert_eq!(result.files[1].paired_with, None);
    }

    #[test]
    fn unknown_formats_are_not_paired() {
        let result =
            analyze_files(&["/d/s_R1.xyz".into(), "/d/s_R2.xyz".into()], None, None).unwrap();
        assert_eq!(result.files[0].paired_with, None);
        assert_eq!(result.files[1].paired_with, None);
        assert!(
            result.summary.paired_end_detected,
            "the naming convention is still reported even when the format is unknown"
        );
    }

    #[test]
    fn underscore_one_two_marker_anywhere_in_the_file_name_pairs() {
        let r1 = "/d/s_1.trimmed.fastq.gz".to_string();
        let r2 = "/d/s_2.trimmed.fastq.gz".to_string();
        let result = analyze_files(&[r1.clone(), r2.clone()], None, None).unwrap();
        assert_eq!(result.files[0].paired_with.as_deref(), Some(r2.as_str()));
        assert_eq!(result.files[1].paired_with.as_deref(), Some(r1.as_str()));
    }

    #[test]
    fn scoped_mode_echoes_only_the_client_mate_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("s_R1.fastq.gz"), b"x").unwrap();
        std::fs::write(dir.path().join("s_R2.fastq.gz"), b"x").unwrap();
        let result = analyze_files(
            &["s_R1.fastq.gz".into(), "s_R2.fastq.gz".into()],
            None,
            Some(dir.path()),
        )
        .unwrap();
        let mate = result.files[0].paired_with.as_deref().unwrap();
        assert_eq!(mate, "s_R2.fastq.gz");
        assert!(
            !mate.contains(dir.path().to_str().unwrap()),
            "the mate must stay client-relative, never a server path"
        );
    }

    #[test]
    fn trailing_separator_paths_do_not_panic_or_pair() {
        // `Path::file_name()` ignores a trailing separator, so byte arithmetic
        // would slice at a non-boundary for a multi-byte file name. A file
        // input must not end in a separator; no pairing is the safe result.
        let result = analyze_files(
            &[
                "/数据/数据_R1.fastq.gz/".into(),
                "/数据/数据_R2.fastq.gz/".into(),
            ],
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.files[0].paired_with, None);
        assert_eq!(result.files[1].paired_with, None);
    }

    #[test]
    fn scoped_analyze_rejects_escape_paths() {
        // #521: with a scope, absolute paths and `..` are refused — the
        // size probe must not become an arbitrary-path oracle.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sample_R1.fastq.gz"), b"x").unwrap();
        let scope = dir.path();

        let result = analyze_files(&["sample_R1.fastq.gz".into()], None, Some(scope)).unwrap();
        assert!(result.files[0].size > 0, "in-scope probe sees the file");
        assert_eq!(
            result.files[0].path, "sample_R1.fastq.gz",
            "response echoes the client path"
        );

        for bad in ["/etc/passwd", "../escape.fastq", "sub/../../../etc/passwd"] {
            let err = analyze_files(&[bad.to_string()], None, Some(scope)).unwrap_err();
            assert!(err.contains("invalid data path"), "{bad}: {err}");
        }
    }

    #[test]
    fn test_reference_discovery() {
        let result =
            discover_reference("hg38", &["fasta".into(), "gtf".into(), "star_index".into()])
                .unwrap();
        // Most components will be missing in CI, but the function should not error
        assert_eq!(result.found.len() + result.missing.len(), 3);
    }
}
