import tempfile
import unittest
from pathlib import Path

from provenance_manifest import (
    DESIGN_DOC_OPTOUT,
    build_manifest,
    check_provenance,
    discover_report_paths,
    report_has_measured_number,
)


def _write(directory: Path, name: str, text: str) -> str:
    path = directory / name
    path.write_text(text, encoding="utf-8")
    return str(path)


MEASURED_REPORT = """# Report

| Cohort | Nodes | Latency (ms) |
| :---: | :---: | :---: |
| 10 | 30 | 118.69 |
"""

PROSE_ONLY_REPORT = """# Report

> Status: no measured result.

## 1. Purpose

This report contains only prose and lists.

- one
- two
"""

# Prose numbers detected by the broadened rule (review P2-2): a decimal with
# units, a percentage, and scientific notation, none of which live in a table.
PROSE_NUMBER_REPORT = """# Report

The median latency was 118.69 ms across the cohort.

Coverage reached 99.9% of targets.

Residual error fell to 1.2e-3 after normalization.
"""

# A design doc that carries dotted version / section strings but no real
# measurement must NOT be flagged (these are the false positives the broadened
# rule must avoid: "26.10.6", "1.98.0", "3.12.4", "127.0.0.1", "WCAG 2.1",
# "section 1.4", "v1.2", "### Task 2.1").
NO_MEASUREMENT_DOC = """# Plan

Built with rustc 1.98.0 and python3 3.12.4 on glibc 26.10.6.

See section 2 and step 4; bind to 127.0.0.1.

All components must meet WCAG 2.1 AA contrast.

### Task 2.1: Data Model

Pinned the spec at version 1.2 of the design.
"""

# Review P2 residual: the plan doc row with fraction form must fail closed.
FRACTION_REPORT = """# Plan

Seeded fault-injection campaign: 7/10 proposals, 3/10 applied, 3/10 verified.
"""

# Word-percent form must also fail closed.
WORD_PERCENT_REPORT = """# Plan

Coverage reached 49 percent of the target matrix.
"""

# A genuine decimal (a measurement, not a dotted identifier) must still trip.
GENUINE_DECIMAL_DOC = """# Report

Median latency was 118.69 ms across the cohort.
"""

# Two-segment dotted identifiers in section/version context must NOT trip.
DOTTED_IDENTIFIER_DOC = """# Plan

See section 1.4 for the rationale and released as v1.2 of the design.
"""

# The bare-integer table rule is fail-closed on success criteria in the
# validation protocol, which is why that doc is an explicit DESIGN_DOC_OPTOUT.
BARE_INTEGER_ROW = """# Protocol

| Pillar | Criterion |
| :--- | :--- |
| Software | `make ci` exits 0 with providers disabled |
"""


class ProvenanceManifestTests(unittest.TestCase):
    def test_measured_row_detected_and_prose_ignored(self):
        self.assertTrue(report_has_measured_number(MEASURED_REPORT))
        self.assertFalse(report_has_measured_number(PROSE_ONLY_REPORT))

    def test_prose_number_and_version_false_positives(self):
        # Broadened detection: a prose measurement fails closed...
        self.assertTrue(report_has_measured_number(PROSE_NUMBER_REPORT))
        # ...but dotted version / section / IP strings are not measurements.
        self.assertFalse(report_has_measured_number(NO_MEASUREMENT_DOC))

    def test_discovery_scans_every_md_and_gate_fails_closed(self):
        # Review P2 residual: discovery is no longer suffix-gated. A NEW *.md
        # doc under the plans directory (not named "*-report.md") that carries
        # a measured number and no manifest entry must FAIL the gate.
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            _write(d, "cohort-scaling-report.md", MEASURED_REPORT)
            _write(d, "NEW-implementation-plan.md", MEASURED_REPORT)
            paths = discover_report_paths(d)
            names = sorted(Path(abs_).name for _, abs_ in paths)
            self.assertEqual(
                names, ["NEW-implementation-plan.md", "cohort-scaling-report.md"]
            )
            manifest = build_manifest()
            errors = check_provenance(manifest, paths)
        self.assertTrue(errors)
        self.assertTrue(
            any(
                "NEW-implementation-plan.md" in e and "no provenance entry" in e
                for e in errors
            ),
            errors,
        )

    def test_fraction_and_word_percent_fail_closed(self):
        # Review P2 residual: fraction form (7/10) and word percent (49 percent)
        # are real measured forms and must trip the detector.
        self.assertTrue(report_has_measured_number(FRACTION_REPORT))
        self.assertTrue(report_has_measured_number(WORD_PERCENT_REPORT))
        # And when the doc has no provenance entry the gate fails closed.
        with tempfile.TemporaryDirectory() as tmp:
            fraction = _write(Path(tmp), "fraction-plan.md", FRACTION_REPORT)
            manifest = build_manifest()
            manifest["artifacts"] = {}
            errors = check_provenance(manifest, [("fraction-plan.md", fraction)])
        self.assertTrue(errors)
        self.assertIn("no provenance entry", errors[0])

    def test_implementation_plan_row_is_gated(self):
        # The plan doc carrying measured fractions must have a manifest entry,
        # so the real repo gate passes for it (not silently skipped).
        manifest = build_manifest()
        self.assertIn(
            "docs/superpowers/plans/industrial-scientific-implementation-plan.md",
            manifest["artifacts"],
        )

    def test_dotted_identifiers_not_flagged_genuine_decimal_is(self):
        # P3 regression: two-segment dotted identifiers (vX.Y, section N.M,
        # "### Task 2.1", "WCAG 2.1") are NOT measurements...
        self.assertFalse(report_has_measured_number(DOTTED_IDENTIFIER_DOC))
        self.assertFalse(report_has_measured_number(NO_MEASUREMENT_DOC))
        # ...while a genuine decimal still is.
        self.assertTrue(report_has_measured_number(GENUINE_DECIMAL_DOC))

    def test_bare_integer_row_is_why_protocol_is_opted_out(self):
        # The fail-closed bare-integer table rule fires on success criteria,
        # which is the documented reason the validation protocol is an explicit
        # DESIGN_DOC_OPTOUT entry rather than a scanned doc.
        self.assertTrue(report_has_measured_number(BARE_INTEGER_ROW))

    def test_prose_amount_report_fails_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = _write(Path(tmp), "prose-number.md", PROSE_NUMBER_REPORT)
            manifest = build_manifest()
            manifest["artifacts"] = {}
            errors = check_provenance(manifest, [("prose-number.md", path)])
        self.assertTrue(errors)
        self.assertIn("no provenance entry", errors[0])

    def test_control_manifest_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            measured = _write(d, "measured.md", MEASURED_REPORT)
            prose = _write(d, "prose.md", PROSE_ONLY_REPORT)
            manifest = build_manifest()
            manifest["artifacts"] = {
                "measured.md": manifest["artifacts"][
                    "docs/superpowers/plans/cohort-scaling-report.md"
                ]
            }
            errors = check_provenance(
                manifest, [("measured.md", measured), ("prose.md", prose)]
            )
        self.assertEqual(errors, [])

    def test_missing_provenance_fails_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            measured = _write(Path(tmp), "measured.md", MEASURED_REPORT)
            manifest = build_manifest()
            manifest["artifacts"] = {}  # no entry for the measured report
            errors = check_provenance(manifest, [("measured.md", measured)])
        self.assertTrue(errors)
        self.assertIn("no provenance entry", errors[0])

    def test_malformed_commit_fails_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            measured = _write(Path(tmp), "measured.md", MEASURED_REPORT)
            manifest = build_manifest()
            entry = dict(
                manifest["artifacts"]["docs/superpowers/plans/cohort-scaling-report.md"]
            )
            entry["source_commit"] = "not-a-commit"
            manifest["artifacts"] = {"measured.md": entry}
            errors = check_provenance(manifest, [("measured.md", measured)])
        self.assertTrue(any("source_commit" in e for e in errors))

    def test_bad_input_digest_fails_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            measured = _write(Path(tmp), "measured.md", MEASURED_REPORT)
            manifest = build_manifest()
            entry = dict(
                manifest["artifacts"]["docs/superpowers/plans/cohort-scaling-report.md"]
            )
            entry["inputs"] = [{"path": "x", "sha256": "deadbeef"}]
            manifest["artifacts"] = {"measured.md": entry}
            errors = check_provenance(manifest, [("measured.md", measured)])
        self.assertTrue(any("sha256" in e for e in errors))

    def test_build_manifest_is_key_ordered(self):
        manifest = build_manifest()
        keys = list(manifest["artifacts"].keys())
        self.assertEqual(keys, sorted(keys))

    def test_design_doc_optout_is_exactly_the_one_protocol_doc(self):
        # P3-1 (review PR #1307): DESIGN_DOC_OPTOUT is a narrow allow-list, not a
        # category. Pin its membership AND count so appending a second measured
        # doc silently un-gating it fails this test. A second opt-out entry must
        # be a deliberate, reviewed change to THIS test, never a quiet append.
        self.assertEqual(len(DESIGN_DOC_OPTOUT), 1)
        self.assertEqual(
            DESIGN_DOC_OPTOUT,
            frozenset(
                {"docs/superpowers/plans/linux-server-validation-protocol.md"}
            ),
        )
        self.assertIn(
            "docs/superpowers/plans/linux-server-validation-protocol.md",
            DESIGN_DOC_OPTOUT,
        )

    def test_context_guard_does_not_suppress_following_measurement(self):
        # P3-2 (review PR #1307): the dotted-keyword context guard must only
        # suppress when the number IS the reference itself. A measurement that
        # follows a section/figure/table/step/phase/p keyword via a comma or a
        # unit is a genuine measurement and must be detected (was a false
        # negative: rstrip() ate intervening digits+punctuation).
        for text in (
            "see table 3, 118.69 ms latency",
            "p 0.05 threshold",
            "phase 3, 3.12 us",
            # Round-5 P3: a reference AND a measurement in the same line -- the
            # measurement ("118.69 ms") must still be flagged even though the
            # line also carries a dotted version reference ("2.0").
            "version 2.0 took 118.69 ms",
            # A bare decimal with no keyword at all is always a measurement.
            "latency 118.69 ms",
            # Round-6 P3: statistical keywords name a quantity, never a
            # location, so a single-digit zero-major reading is detected
            # ("p 0.5"/"p 0.9"); only the reference-noun keywords suppress.
            "p 0.5",
            "p 0.9",
            # Round-6 P2: a full lowercase word ending a SENTENCE ("... the
            # appendix.") is a sentence boundary, not a reference context, so
            # the measurement that starts the next sentence is detected.
            "In the appendix. 2.5 million reads",
            "as noted in the table. 4.2 samples",
            "Check the version. 1.5 participants",
            "the results for this phase. 3.1 of the cohort failed",
            "at the end of this chapter. 3.5 samples dropped",
        ):
            self.assertTrue(report_has_measured_number(text), text)

    def test_context_guard_still_clean_for_real_references(self):
        # ...while true version/section/identifier strings stay clean (False).
        for text in (
            "section 1.4",
            "v1.2",
            "### Task 2.1",
            "127.0.0.1",
            "3.12.4",
            "version 2.0",
            "P1 section 3.1-3.7",
        ):
            self.assertFalse(report_has_measured_number(text), text)

    def test_abbreviated_and_zero_major_references_are_not_measurements(self):
        # Round-5 P3 regression (review PR #1307): the narrowed round-4 guard
        # leaked abbreviated keyword forms (trailing period), zero-major ids,
        # and comma-followed references into the measured set. All of these are
        # location / version references and must read as NO measurement.
        for text in (
            "Fig. 4.2",
            "Sec. 2.1",
            "No. 3.1",
            "Ch. 5.1",
            "pp. 12.3",
            "Tab. 3.1",
            "v. 2.0",
            "v0.8",
            "version 0.2",
            "section 0.1",
            "see section 1.4, which describes it",
            "Per section 2.1, apply the filter",
        ):
            self.assertFalse(report_has_measured_number(text), text)


if __name__ == "__main__":
    unittest.main()
