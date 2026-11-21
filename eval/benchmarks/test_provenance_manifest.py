import tempfile
import unittest
from pathlib import Path

from provenance_manifest import (
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
# rule must avoid: "26.10.6", "1.98.0", "3.12.4", "127.0.0.1").
NO_MEASUREMENT_DOC = """# Plan

Built with rustc 1.98.0 and python3 3.12.4 on glibc 26.10.6.

See section 2 and step 4; bind to 127.0.0.1.
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

    def test_discovery_finds_new_report_and_gate_fails_closed(self):
        # Review P2-1 repro: a NEW report added under the plans directory that
        # carries a measured number and no manifest entry must FAIL the gate.
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            _write(d, "cohort-scaling-report.md", MEASURED_REPORT)
            _write(d, "NEW-report.md", MEASURED_REPORT)
            # A non-report plan doc with a big numeric table must be ignored.
            _write(
                d,
                "design-plan.md",
                "| Col | N |\n| :--- | :--- |\n| x | 100 |\n",
            )
            paths = discover_report_paths(d)
            names = sorted(Path(abs_).name for _, abs_ in paths)
            self.assertEqual(names, ["NEW-report.md", "cohort-scaling-report.md"])
            manifest = build_manifest()
            errors = check_provenance(manifest, paths)
        self.assertTrue(errors)
        self.assertTrue(
            any("NEW-report.md" in e and "no provenance entry" in e for e in errors),
            errors,
        )

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


if __name__ == "__main__":
    unittest.main()
