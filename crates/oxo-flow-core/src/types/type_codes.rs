//! Canonical BioTypes diagnostic code strings (`TYPE-*`).
//!
//! The biological subtyping checker ([`super::checker`]) and the header
//! introspectors ([`super::inspect`]) emit `TYPE-*` diagnostics. Before this
//! module, every code was a hand-typed string literal at each emit site, and
//! the advisory-code table ([`super::checker::ADVISORY_TYPE_CODES`]) retyped a
//! subset of them again. Hand-copied code strings drift from their origin, so
//! this module is the single source of truth: every emit site and the advisory
//! table reference these constants (issue #1328).

/// Format incompatibility across a producer/consumer channel.
pub const TYPE_001: &str = "TYPE-001";
/// Sort-order conflict across a producer/consumer channel.
pub const TYPE_002: &str = "TYPE-002";
/// Missing companion index artifact.
pub const TYPE_003: &str = "TYPE-003";
/// Reference-build (assembly) conflict.
pub const TYPE_004: &str = "TYPE-004";
/// Missing read-group (`@RG`) metadata required downstream.
pub const TYPE_005: &str = "TYPE-005";
/// Block-compression stream mismatch.
pub const TYPE_006: &str = "TYPE-006";
/// Extensible attribute-contract mismatch.
pub const TYPE_007: &str = "TYPE-007";
/// Advisory: consumer declares a type but the producer declares none (unverifiable edge).
pub const TYPE_008: &str = "TYPE-008";
/// Advisory: consumer names a reference build while the producer omits one.
pub const TYPE_009: &str = "TYPE-009";
/// Advisory: both rules declare types but no output channel pairs with an input channel.
pub const TYPE_010: &str = "TYPE-010";
/// A `types.<name>` reference does not resolve to any `[types.*]` declaration.
pub const TYPE_UNKNOWN: &str = "TYPE-UNKNOWN";
/// Header introspection of a file failed (see [`super::inspect`]).
pub const TYPE_INSPECT: &str = "TYPE-INSPECT";

/// Every `TYPE-*` code in the catalog, in declaration order.
///
/// The later `oxo-flow diagnostics list types` enumerator (issue #1328, second
/// half) reads this array so the emitted catalog and the emit sites cannot
/// drift. Its length and uniqueness are pinned by the guard test in this module.
pub const ALL_TYPE_CODES: [&str; 12] = [
    TYPE_001,
    TYPE_002,
    TYPE_003,
    TYPE_004,
    TYPE_005,
    TYPE_006,
    TYPE_007,
    TYPE_008,
    TYPE_009,
    TYPE_010,
    TYPE_UNKNOWN,
    TYPE_INSPECT,
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Emit sites that must route their code through this module.
    const CHECKER_SRC: &str = include_str!("checker.rs");
    const INSPECT_SRC: &str = include_str!("inspect.rs");

    /// The constant identifiers referenced as `type_codes::<ident>` in the two
    /// emit-site files. Byte-for-byte the twelve names declared above.
    const EXPECTED_NAMES: [&str; 12] = [
        "TYPE_001",
        "TYPE_002",
        "TYPE_003",
        "TYPE_004",
        "TYPE_005",
        "TYPE_006",
        "TYPE_007",
        "TYPE_008",
        "TYPE_009",
        "TYPE_010",
        "TYPE_UNKNOWN",
        "TYPE_INSPECT",
    ];

    #[test]
    fn all_type_codes_are_unique() {
        let set: BTreeSet<&str> = ALL_TYPE_CODES.iter().copied().collect();
        assert_eq!(
            set.len(),
            ALL_TYPE_CODES.len(),
            "duplicate TYPE-* value in ALL_TYPE_CODES: {ALL_TYPE_CODES:?}"
        );
    }

    #[test]
    fn all_type_code_count_is_pinned() {
        assert_eq!(
            ALL_TYPE_CODES.len(),
            12,
            "TYPE-* catalog count changed; update ALL_TYPE_CODES and EXPECTED_NAMES together"
        );
    }

    #[test]
    fn no_inline_type_code_emit_literal_survives() {
        for (name, src) in [("checker.rs", CHECKER_SRC), ("inspect.rs", INSPECT_SRC)] {
            assert!(
                !src.contains("code: \"TYPE-"),
                "{name} still emits an inline `code: \"TYPE-...\"` literal; route it through type_codes::"
            );
        }
    }

    #[test]
    fn emit_sites_reference_every_constant_and_hold_no_orphans() {
        let mut referenced: BTreeSet<String> = BTreeSet::new();
        for src in [CHECKER_SRC, INSPECT_SRC] {
            for tail in src.split("type_codes::").skip(1) {
                let ident: String = tail
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !ident.is_empty() {
                    referenced.insert(ident);
                }
            }
        }

        let expected: BTreeSet<String> = EXPECTED_NAMES.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            referenced, expected,
            "the set of type_codes:: references in the emit sites must equal the constant set \
             (no orphan constant, no stray reference)"
        );
        assert_eq!(
            referenced.len(),
            ALL_TYPE_CODES.len(),
            "number of distinct type_codes:: use sites must equal the number of constants"
        );
    }
}
