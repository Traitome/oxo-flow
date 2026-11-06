//! Shared `protected_output` semantics.
//!
//! Every destroy path — failure invalidation, the disk-reclaim gate, `clean` —
//! must apply identical protection semantics. This module is the single
//! matcher they all call; hand-copied per-file matchers drifted in the past
//! (one of them failed open: an unparseable pattern read as "unprotected"
//! and the engine deleted a file the user asked to protect).
//!
//! Config validation rejects patterns the glob parser refuses (`***`,
//! mid-component `**`, invalid `[a-b]` ranges) so a protection typo is a
//! hard error at load time. At match time an unparseable pattern fails
//! closed — it protects — as defense in depth for callers that skip
//! config validation.

use glob::MatchOptions;

/// Match options pinned for every protected-output match.
///
/// `require_literal_separator` keeps `*` to a single path component — the
/// same semantics the filesystem-walking `glob::glob` applies, so a pattern
/// cannot protect more in one destroy path than in another.
fn match_options() -> MatchOptions {
    MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    }
}

/// Replace oxo-flow `{wildcard}` chunks with a single-component `*` so
/// `results/{sample}.bam` matches concrete per-instance paths. Raw shell
/// globs (`*`, `?`, `[...]`) pass through untouched.
#[must_use]
pub fn wildcard_to_glob(pattern: &str) -> String {
    let mut result = String::new();
    let mut in_wildcard = false;
    for character in pattern.chars() {
        match character {
            '{' => {
                in_wildcard = true;
                result.push('*');
            }
            '}' => {
                in_wildcard = false;
            }
            _ if !in_wildcard => result.push(character),
            _ => {}
        }
    }
    result
}

/// Validate one `protected_output` declaration. Returns a human-readable
/// reason when the glob parser refuses it — config validation surfaces this
/// as a hard error so the pattern can never silently degrade to
/// "unprotected" on a destroy path.
///
/// # Errors
/// The glob parser's rejection reason for the (wildcard-converted) pattern.
pub fn validate_protected_pattern(pattern: &str) -> Result<(), String> {
    glob::Pattern::new(&wildcard_to_glob(pattern))
        .map(|_| ())
        .map_err(|error| format!("invalid protected_output pattern {pattern:?}: {error}"))
}

/// Whether one protected-output declaration covers a candidate path
/// (pure string match — the invalidation snapshot must match before any
/// output exists on disk). Honored forms: exact literal paths, `{wildcard}`
/// patterns, and shell-glob patterns.
///
/// Fail-closed: an unparseable pattern protects the candidate — the user's
/// intent was to protect, and config validation rejects such patterns
/// upstream.
#[must_use]
pub fn protected_pattern_matches(pattern: &str, candidate: &str) -> bool {
    if pattern == candidate {
        return true;
    }
    match glob::Pattern::new(&wildcard_to_glob(pattern)) {
        Ok(pat) => pat.matches_with(candidate, match_options()),
        Err(error) => {
            tracing::warn!(
                "invalid protected_output pattern {pattern:?} ({error}) — treating candidates as protected"
            );
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_literal_matches_only_itself() {
        assert!(protected_pattern_matches("results/final.bam", "results/final.bam"));
        assert!(!protected_pattern_matches("results/final.bam", "results/other.bam"));
    }

    #[test]
    fn wildcard_form_matches_concrete_instances_within_one_component() {
        assert!(protected_pattern_matches("results/{sample}.bam", "results/S1.bam"));
        assert!(!protected_pattern_matches(
            "results/{sample}.bam",
            "results/S1/extra.bam"
        ));
    }

    #[test]
    fn shell_glob_stays_within_one_path_component() {
        assert!(protected_pattern_matches("results/*.bam", "results/S1.bam"));
        assert!(!protected_pattern_matches("results/*.bam", "results/S1/extra.bam"));
    }

    #[test]
    fn invalid_pattern_fails_closed() {
        assert!(validate_protected_pattern("***").is_err());
        assert!(validate_protected_pattern("results/[a-b").is_err());
        assert!(validate_protected_pattern("results/*.bam").is_ok());
        assert!(validate_protected_pattern("results/{sample}.bam").is_ok());
        assert!(protected_pattern_matches("***", "anything/at/all"));
    }
}
