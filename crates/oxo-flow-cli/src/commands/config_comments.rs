//! Extract per-key descriptions from `#` comments in a workflow's `[config]`
//! section.
//!
//! The engine's TOML parser discards comments, so `info` re-reads the raw
//! file text and associates comment lines with the `[config]` keys they
//! describe. The association rules are deliberately simple and were validated
//! against the oxo-community staging workflows:
//!
//! - a contiguous block of `#` lines immediately above a key line (no blank
//!   line in between) is that key's description;
//! - a trailing `#` comment on the key line itself is the fallback;
//! - comments inside multi-line values or outside `[config]` never associate;
//! - comments above `[config.<name>]` subtable headers describe the table key.

use std::collections::BTreeMap;

/// Extract `[config]` key descriptions from raw workflow text.
///
/// Description lines are joined with single spaces; empty `#` lines and pure
/// decorator lines (`# ----`) are dropped.
pub fn extract_config_descriptions(text: &str) -> BTreeMap<String, String> {
    let mut descriptions: BTreeMap<String, String> = BTreeMap::new();
    let mut section: Option<String> = None;
    let mut pending: Vec<String> = Vec::new();
    let mut value_depth = 0usize;

    for line in text.lines() {
        let line = line.trim();

        // Inside a multi-line value: only track bracket balance. This runs
        // before header parsing so value lines like `["x"]` are never taken
        // for section headers. Comment lines here describe array elements,
        // not the parameter.
        if section.as_deref() == Some("config") && value_depth > 0 {
            value_depth = bracket_balance(line, value_depth);
            continue;
        }

        // `[name]`, `[name.sub]`, `[[name]]` — any header leaves `[config]`.
        if let Some(header) = section_header(line) {
            let is_config = header == "config";
            let is_subtable = header.starts_with("config.");
            section = Some(header.clone());
            // `[config.<name>]`: pending comments describe the table key.
            if is_subtable {
                attach(&mut descriptions, &mut pending, &header["config.".len()..]);
            }
            if !is_config && !is_subtable {
                pending.clear();
            }
            value_depth = 0;
            continue;
        }
        if section.as_deref() != Some("config") {
            continue;
        }

        if let Some(comment) = comment_text(line) {
            // Empty `#` lines and pure decorator lines carry no meaning.
            if !comment.is_empty() && !is_decorator(&comment) {
                pending.push(comment);
            }
            continue;
        }

        if line.is_empty() {
            // A blank line severs the comment block from any following key.
            pending.clear();
            continue;
        }

        let Some((key, value)) = key_line(line) else {
            continue;
        };
        value_depth = open_bracket_depth(value);
        if pending.is_empty() {
            if let Some(trailing) = trailing_comment(value) {
                descriptions.insert(key, trailing);
            }
        } else {
            attach(&mut descriptions, &mut pending, &key);
        }
    }
    descriptions
}

/// `[name]`, `[name.sub]`, `[[name]]` → header name (any trailing comment
/// stripped); other lines → `None`.
fn section_header(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with('[') {
        return None;
    }
    let inner = trimmed.trim_start_matches('[');
    let inner = inner.strip_prefix('[').unwrap_or(inner);
    let end = inner.find(']')?;
    let name = inner[..end].trim();
    let rest = inner[end + 1..].trim_start();
    let closes = if rest.starts_with(']') { 1 } else { 0 };
    if closes == 0 && !rest.is_empty() && !rest.starts_with('#') {
        return None;
    }
    (!name.is_empty()).then(|| name.to_string())
}

/// `# comment` → `comment` (leading `#` and one space stripped, trimmed);
/// non-comment lines → `None`.
fn comment_text(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with('#') {
        return None;
    }
    let body = trimmed.trim_start_matches('#').trim();
    Some(body.to_string())
}

/// Pure decorator lines (`# ----`, `# ====`) carry no meaning.
fn is_decorator(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c == '-' || c == '=')
}

/// `key = value` → `(key, value-after-=)` for bare TOML keys; `None` for
/// other lines. Value lines inside `[...]` arrays are handled by the caller.
fn key_line(line: &str) -> Option<(String, &str)> {
    let eq = line.find('=')?;
    let key = line[..eq].trim();
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some((key.to_string(), line[eq + 1..].trim()))
}

/// Bracket depth opened by a value line (`known_indels = [` → 1, balanced
/// arrays/strings → 0). Quote-aware so `"["` inside strings does not count.
fn open_bracket_depth(value: &str) -> usize {
    bracket_balance(value, 0)
}

/// New bracket depth after scanning `text`, starting from `depth` (which the
/// caller guarantees is > 0 when scanning continuation lines).
fn bracket_balance(text: &str, mut depth: usize) -> usize {
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    for c in text.chars() {
        match in_string {
            Some(quote) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == quote {
                    in_string = None;
                }
            }
            None => match c {
                '"' | '\'' => in_string = Some(c),
                '[' => depth += 1,
                ']' => depth = depth.saturating_sub(1),
                _ => {}
            },
        }
    }
    depth
}

/// Trailing `# comment` outside quotes, with non-whitespace content;
/// `None` when the line has no trailing comment (e.g. `key = "#FF0000"`).
fn trailing_comment(value: &str) -> Option<String> {
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    for (idx, c) in value.char_indices() {
        match in_string {
            Some(quote) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == quote {
                    in_string = None;
                }
            }
            None => match c {
                '"' | '\'' => in_string = Some(c),
                '#' => {
                    let comment = value[idx + 1..].trim();
                    return (!comment.is_empty()).then(|| comment.to_string());
                }
                _ => {}
            },
        }
    }
    None
}

/// Join pending comment lines into a description for `key`.
fn attach(descriptions: &mut BTreeMap<String, String>, pending: &mut Vec<String>, key: &str) {
    if let Some(description) = join(pending) {
        descriptions.insert(key.to_string(), description);
    }
    pending.clear();
}

/// `--- Banner line ---` (starts and ends with 3+ dashes).
fn is_banner(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.len() >= 6 && trimmed.starts_with("---") && trimmed.ends_with("---")
}

/// `--- MultiQC ---` → `MultiQC`.
fn strip_banner_dashes(line: &str) -> String {
    line.trim()
        .trim_start_matches('-')
        .trim()
        .trim_end_matches('-')
        .trim()
        .to_string()
}

/// Join comment lines with single spaces; `None` when nothing meaningful.
/// Banner lines (`--- section ---`) act as section markers: when the block
/// also has regular text they are dropped; a lone banner keeps its interior.
fn join(lines: &[String]) -> Option<String> {
    let kept: Vec<String> = if lines.iter().any(|line| !is_banner(line)) {
        lines
            .iter()
            .filter(|line| !is_banner(line))
            .cloned()
            .collect()
    } else {
        lines.iter().map(|line| strip_banner_dashes(line)).collect()
    };
    let description = kept.join(" ");
    if description.is_empty() {
        None
    } else {
        Some(description)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_comment_line_above_key_becomes_description() {
        let text = "[config]\n# Output directory.\nout_dir = \"results\"\n";
        assert_eq!(
            extract_config_descriptions(text),
            BTreeMap::from([("out_dir".to_string(), "Output directory.".to_string())])
        );
    }

    #[test]
    fn multi_line_block_joins_with_spaces_and_skips_empty_lines() {
        let text = "[config]\n# Optional: pre-trained classifier\n#\n# for the 16S region.\nclassifier = \"\"\n";
        assert_eq!(
            extract_config_descriptions(text),
            BTreeMap::from([(
                "classifier".to_string(),
                "Optional: pre-trained classifier for the 16S region.".to_string()
            )])
        );
    }

    #[test]
    fn trailing_comment_is_fallback_when_no_block_above() {
        let text = "[config]\nsave_output_as_bam = false # CRAM output mode\n";
        assert_eq!(
            extract_config_descriptions(text),
            BTreeMap::from([(
                "save_output_as_bam".to_string(),
                "CRAM output mode".to_string()
            )])
        );
    }

    #[test]
    fn preceding_block_wins_over_trailing_comment() {
        let text = "[config]\n# Block comment.\nkey = 1 # trailing comment\n";
        assert_eq!(
            extract_config_descriptions(text),
            BTreeMap::from([("key".to_string(), "Block comment.".to_string())])
        );
    }

    #[test]
    fn hash_inside_quoted_value_is_not_a_trailing_comment() {
        let text = "[config]\ncolor = \"#FF0000\"\n";
        assert_eq!(extract_config_descriptions(text), BTreeMap::new());
    }

    #[test]
    fn blank_line_severs_comment_block_from_key() {
        // The comment reads as a section note, not a key description.
        let text = "[config]\n# Section-level note.\n\nrmats_cstat = 0.0001\n";
        assert_eq!(extract_config_descriptions(text), BTreeMap::new());
    }

    #[test]
    fn comments_inside_multiline_arrays_do_not_associate() {
        let text = "[config]\n# Known indels.\nknown_indels = [\n  # chr1\n  \"/a.vcf.gz\",\n  \"/b.vcf.gz\",\n]\n# Next key.\nnext_key = 1\n";
        assert_eq!(
            extract_config_descriptions(text),
            BTreeMap::from([
                ("known_indels".to_string(), "Known indels.".to_string()),
                ("next_key".to_string(), "Next key.".to_string()),
            ])
        );
    }

    #[test]
    fn bracket_inside_quoted_array_element_does_not_break_balance() {
        let text =
            "[config]\n# Paired brackets.\nkeys = [\n  \"a[b]\",\n]\n# After array.\nafter = 1\n";
        assert_eq!(
            extract_config_descriptions(text),
            BTreeMap::from([
                ("keys".to_string(), "Paired brackets.".to_string()),
                ("after".to_string(), "After array.".to_string()),
            ])
        );
    }

    #[test]
    fn comments_outside_config_section_are_ignored() {
        let text = "# Workflow-level note.\n[workflow]\nname = \"wf\"\n\n[config]\nkey = 1\n\n[[rules]]\n# Rule shell note — not a config description.\nname = \"align\"\n";
        assert_eq!(extract_config_descriptions(text), BTreeMap::new());
    }

    #[test]
    fn dangling_block_before_next_section_header_is_discarded() {
        // Mirrors gallery 06: a comment block above [[sample_groups]].
        let text = "[config]\nreference = \"/ref/genome.fa\"\n\n# Sample cohort for {sample} expansion.\n[[sample_groups]]\nname = \"cohort\"\n";
        assert_eq!(extract_config_descriptions(text), BTreeMap::new());
    }

    #[test]
    fn subtable_header_takes_pending_comments_for_table_key() {
        let text = "[config]\n# Alignment settings.\n[config.align]\nthreads = 4\n";
        assert_eq!(
            extract_config_descriptions(text),
            BTreeMap::from([("align".to_string(), "Alignment settings.".to_string())])
        );
    }

    #[test]
    fn decorator_lines_are_filtered_from_descriptions() {
        let text = "[config]\n# ----\n# Meaningful text.\n# ----\nkey = 1\n";
        assert_eq!(
            extract_config_descriptions(text),
            BTreeMap::from([("key".to_string(), "Meaningful text.".to_string())])
        );
    }

    #[test]
    fn comments_above_config_header_do_not_attach_to_first_key() {
        // Only comments INSIDE `[config]` associate.
        let text = "# User configuration.\n[config]\nkey = 1\n";
        assert_eq!(extract_config_descriptions(text), BTreeMap::new());
    }

    #[test]
    fn lone_banner_line_keeps_interior_text_without_dashes() {
        // `--- MultiQC ---` is a section marker; the interior names the
        // group the key belongs to.
        let text = "[config]\n# --- MultiQC ---\nmultiqc_title = \"\"\n";
        assert_eq!(
            extract_config_descriptions(text),
            BTreeMap::from([("multiqc_title".to_string(), "MultiQC".to_string())])
        );
    }

    #[test]
    fn banner_line_is_dropped_when_block_has_regular_text() {
        // Section marker + description: keep the description only.
        let text = "[config]\n# --- Input reads (upstream samplesheet) ---\n# Directory holding raw files.\nraw_dir = \"raw\"\n";
        assert_eq!(
            extract_config_descriptions(text),
            BTreeMap::from([(
                "raw_dir".to_string(),
                "Directory holding raw files.".to_string()
            )])
        );
    }

    #[test]
    fn array_element_line_that_looks_like_a_header_is_not_one() {
        // `["x"]` as the last array element (no trailing comma) must not be
        // parsed as a section header while inside the multi-line value.
        let text = "[config]\n# Nested arrays.\nkeys = [\n  [\"x\"]\n]\n# After.\nafter = 1\n";
        assert_eq!(
            extract_config_descriptions(text),
            BTreeMap::from([
                ("keys".to_string(), "Nested arrays.".to_string()),
                ("after".to_string(), "After.".to_string()),
            ])
        );
    }
}
