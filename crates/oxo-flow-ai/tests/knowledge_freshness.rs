//! Knowledge freshness drift guard (issue #153).
//!
//! `knowledge_meta.json` documents every embedded knowledge data file: its
//! record count and generation timestamp. This test keeps the metadata in
//! sync with the data files on disk — the update pipeline must not bump one
//! side without the other. It is strict on every count once the metadata
//! file exists; until the update pipeline produces it (the first run), the
//! test notes the absence and passes, so pre-generation builds stay green.
//!
//! The 45-day staleness gate deliberately does NOT live here — it belongs
//! to the CI release pipeline, not unit tests.

use oxo_flow_ai::knowledge::meta::KnowledgeMeta;
use std::path::Path;

const META_REL: &str = "src/knowledge/knowledge_meta.json";

#[test]
fn knowledge_meta_parses_and_counts_match_data_files() {
    let meta_path = Path::new(META_REL);
    if !meta_path.exists() {
        eprintln!("SKIP: {META_REL} not generated yet — freshness drift guard inactive");
        return;
    }

    let raw = std::fs::read_to_string(meta_path)
        .unwrap_or_else(|e| panic!("cannot read {META_REL}: {e}"));
    let meta = KnowledgeMeta::parse(&raw).unwrap_or_else(|e| panic!("{META_REL} must parse: {e}"));
    assert!(
        !meta.sources.is_empty(),
        "{META_REL} must record at least one source"
    );

    let data_dir = Path::new("src/knowledge");
    for src in &meta.sources {
        // Every source must have a valid RFC 3339 generation timestamp
        // (what staleness display and the release gate parse it with).
        assert!(
            src.generated_date().is_some(),
            "source {} has an unparsable generated_at: {}",
            src.name,
            src.generated_at
        );

        // The referenced data file must exist and its record (non-empty
        // line) count must equal the metadata count.
        let data_path = data_dir.join(&src.data_file);
        assert!(
            data_path.exists(),
            "source {} references missing data file {}",
            src.name,
            src.data_file
        );
        let records = std::fs::read_to_string(&data_path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", data_path.display()))
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count();
        assert_eq!(
            src.count, records,
            "count drift for {}: meta says {} but {} has {} records",
            src.name, src.count, src.data_file, records
        );
    }

    // Reverse direction: every embedded JSONL data file must be described
    // by exactly one meta source — a new data file without metadata would
    // be invisible to `ai status` and the freshness display.
    let covered: std::collections::BTreeSet<String> = std::fs::read_dir(data_dir)
        .expect("knowledge dir must exist")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .collect();
    let meta_files: std::collections::BTreeSet<String> = meta
        .sources
        .iter()
        .map(|src| src.data_file.clone())
        .collect();
    assert_eq!(
        covered, meta_files,
        "embedded data files and knowledge_meta.json entries must match exactly"
    );
}
