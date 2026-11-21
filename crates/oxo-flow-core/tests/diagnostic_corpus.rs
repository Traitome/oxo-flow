//! Corpus-scale false-positive / false-negative measurement for the 31-pattern
//! runtime diagnostics engine (issue #1316).
//!
//! Report-only harness: it drives the *real* `DiagnosticsEngine` pattern table
//! (no model, no container, no network) over a small realistic corpus of
//! multi-line failure tails from STAR/BWA/samtools/GATK/conda/docker/etc.
//! output and asserts a documented baseline. Each positive block embeds the
//! pattern's text inside surrounding tool output rather than copying a bare
//! canonical-trigger line. It changes no regex and no engine behavior; the
//! deliverable is a measurement, not a proof that the engine is correct.
//!
//! Method mirrors `scientific_preflight_adversarial.rs` (#1244):
//! positive (must fire) / benign handshake (must stay silent) /
//! mention-only (must stay silent) blocks. All three sets are parsed and
//! asserted here, so every reported count is a measurement.
//!
//! Fixtures live in `tests/fixtures/diag_corpus/`.

use std::collections::BTreeSet;

use oxo_flow_core::diagnostics::DiagnosticsEngine;

const POSITIVE_LOG: &str = include_str!("fixtures/diag_corpus/positive.log");
const BENIGN_LOG: &str = include_str!("fixtures/diag_corpus/benign.log");
const MENTION_ONLY_LOG: &str = include_str!("fixtures/diag_corpus/mention_only.log");

/// A positive corpus block: a realistic failure tail that MUST fire `id`
/// at `exit`.
struct Positive {
    id: String,
    exit: Option<i32>,
    body: String,
}

/// Parse the positive corpus. Blocks are blank-line separated; each block
/// opens with `# intended=<id> exit=<code>` and the remaining non-comment
/// lines are the stderr/stdout tail.
fn parse_positive(src: &str) -> Vec<Positive> {
    let mut out = Vec::new();
    let mut current: Option<Positive> = None;
    for raw in src.lines() {
        let line = raw.trim_end();
        if let Some(header) = line.strip_prefix("# intended=") {
            if let Some(p) = current.take() {
                out.push(p);
            }
            let mut parts = header.split_whitespace();
            let id = parts.next().expect("intended=id").to_string();
            let exit = parts
                .next()
                .and_then(|kv| kv.strip_prefix("exit="))
                .and_then(|v| v.parse::<i32>().ok());
            current = Some(Positive {
                id,
                exit,
                body: String::new(),
            });
        } else if line.starts_with('#') {
            // Non-header comment (e.g. the file preamble): ignore.
        } else if line.is_empty() {
            if let Some(p) = current.take() {
                out.push(p);
            }
        } else if let Some(p) = current.as_mut() {
            if !p.body.is_empty() {
                p.body.push('\n');
            }
            p.body.push_str(line);
        }
    }
    if let Some(p) = current.take() {
        out.push(p);
    }
    out
}

/// Parse the benign / mention-only corpus: blank-line-separated blocks, with
/// `#` comment lines ignored.
fn parse_blocks(src: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current = String::new();
    for raw in src.lines() {
        let line = raw.trim_end();
        if line.starts_with('#') {
            continue;
        }
        if line.is_empty() {
            if !current.is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
        } else {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }
    }
    if !current.is_empty() {
        blocks.push(current);
    }
    blocks
}

/// Pattern ids fired by `body` at `exit`, excluding only the generic
/// `unknown_error` fallback. The content-independent OOM/timeout injections
/// (driven purely by exit code / signal, not by stderr content) are retained:
/// they are the intended diagnostic for the `oom_killed` / `timeout` positives.
fn fired_patterns(engine: &DiagnosticsEngine, body: &str, exit: Option<i32>) -> Vec<String> {
    let context = oxo_flow_core::diagnostics::DiagnosticContext::new("corpus_rule", body, exit);
    engine
        .analyze_context(&context)
        .into_iter()
        .filter_map(|r| r.error_pattern)
        .filter(|id| id != "unknown_error")
        .collect()
}

/// Source drift gate: pattern ids literally present in the engine source,
/// used to catch a fixture/engine desync (mirrors #1244's `source_rule_ids`).
fn source_pattern_ids(src: &str) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for line in src.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("id: \"")
            && let Some(end) = rest.find('"')
        {
            ids.insert(rest[..end].to_string());
        }
    }
    ids
}

#[test]
fn corpus_positive_blocks_fire_their_intended_pattern() {
    let engine = DiagnosticsEngine::new();
    let positives = parse_positive(POSITIVE_LOG);
    assert_eq!(
        positives.len(),
        31,
        "positive corpus must cover all 31 patterns"
    );

    let mut tp = 0usize;
    let mut fn_ = 0usize;
    let mut collateral = 0usize;
    let mut lines = Vec::new();
    for p in &positives {
        let hits = fired_patterns(&engine, &p.body, p.exit);
        let intended = hits.iter().any(|h| h == &p.id);
        if intended {
            tp += 1;
        } else {
            fn_ += 1;
        }
        let extras: Vec<&str> = hits
            .iter()
            .filter(|h| *h != &p.id)
            .map(String::as_str)
            .collect();
        collateral += extras.len();
        lines.push(format!(
            "{:24} exit={:?} intended_hit={} extras={:?}",
            p.id, p.exit, intended, extras
        ));
        assert!(
            intended,
            "positive block for pattern '{}' did NOT fire it (hits={:?}) at exit {:?}",
            p.id, hits, p.exit
        );
    }

    println!("== positive confusion baseline ==");
    println!("patterns=31 TP={tp} FN={fn_} collateral_matches={collateral}");
    for l in &lines {
        println!("{l}");
    }
    assert_eq!(tp, 31);
    assert_eq!(fn_, 0);
}

#[test]
fn corpus_benign_and_mention_blocks_stay_silent() {
    let engine = DiagnosticsEngine::new();

    // Realistic non-zero exits a failed rule actually reports (deliberately
    // excludes 137/152 so content-independent OOM/timeout injection cannot
    // mask a content false positive).
    let exits = [Some(1), Some(2), Some(130)];

    let mut benign_blocks = 0usize;
    let mut benign_violations: Vec<String> = Vec::new();
    for block in parse_blocks(BENIGN_LOG) {
        benign_blocks += 1;
        for exit in exits {
            let hits = fired_patterns(&engine, &block, exit);
            if !hits.is_empty() {
                benign_violations.push(format!("benign at exit {exit:?} fired {hits:?}:\n{block}"));
            }
        }
    }

    let mut mention_blocks = 0usize;
    let mut mention_violations: Vec<String> = Vec::new();
    for block in parse_blocks(MENTION_ONLY_LOG) {
        mention_blocks += 1;
        for exit in exits {
            let hits = fired_patterns(&engine, &block, exit);
            if !hits.is_empty() {
                mention_violations.push(format!(
                    "mention-only at exit {exit:?} fired {hits:?}:\n{block}"
                ));
            }
        }
    }

    println!("== negative confusion baseline ==");
    println!(
        "benign_blocks={benign_blocks} benign_fp={} \
         mention_blocks={mention_blocks} mention_fp={}",
        benign_violations.len(),
        mention_violations.len()
    );
    assert!(
        benign_violations.is_empty(),
        "benign corpus false positives:\n{}",
        benign_violations.join("\n---\n")
    );
    assert!(
        mention_violations.is_empty(),
        "mention-only corpus false positives:\n{}",
        mention_violations.join("\n---\n")
    );
}

#[test]
fn positive_corpus_covers_every_pattern_exactly_once() {
    let engine = DiagnosticsEngine::new();
    let positives = parse_positive(POSITIVE_LOG);

    let mut covered = BTreeSet::new();
    for p in &positives {
        assert!(
            covered.insert(p.id.clone()),
            "pattern '{}' has more than one positive corpus block",
            p.id
        );
    }

    let engine_ids: BTreeSet<String> = engine.pattern_ids().iter().map(|s| s.to_string()).collect();
    assert_eq!(
        engine_ids, covered,
        "positive corpus ids must exactly match the engine's registered pattern ids"
    );
}

/// Realism guard: every positive block must be a genuine multi-line failure
/// tail. A block that is a single bare line is almost always a verbatim copy
/// of the pattern's own canonical trigger, which trivially matches because it
/// literally contains the regex text rather than surrounding real tool output.
#[test]
fn positive_corpus_blocks_are_all_multi_line() {
    let positives = parse_positive(POSITIVE_LOG);
    for p in &positives {
        let body_lines = p.body.lines().filter(|l| !l.trim().is_empty()).count();
        assert!(
            body_lines >= 2,
            "positive block '{}' is a single line ({:?}); it must carry surrounding \
             real tool-output context, not a bare canonical-trigger line",
            p.id,
            p.body
        );
    }
}

/// Drift gate: the fixture table must stay in sync with the ids literally
/// declared in the engine source. If a pattern is added/renamed without a
/// corpus block (or vice versa), this fails.
#[test]
fn positive_corpus_matches_engine_source_ids() {
    let source_ids = source_pattern_ids(include_str!("../src/diagnostics.rs"));
    let positives = parse_positive(POSITIVE_LOG);
    let fixture_ids: BTreeSet<String> = positives.into_iter().map(|p| p.id).collect();

    assert_eq!(
        source_ids.len(),
        31,
        "expected 31 pattern ids in diagnostics.rs, found {}",
        source_ids.len()
    );
    assert_eq!(
        source_ids,
        fixture_ids,
        "corpus fixtures drifted from diagnostics.rs: missing={:?} extra={:?}",
        source_ids.difference(&fixture_ids).collect::<Vec<_>>(),
        fixture_ids.difference(&source_ids).collect::<Vec<_>>(),
    );
}
