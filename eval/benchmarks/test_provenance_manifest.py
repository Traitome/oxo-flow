import tempfile
import unittest
from pathlib import Path

from provenance_manifest import (
    build_manifest,
    check_provenance,
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


class ProvenanceManifestTests(unittest.TestCase):
    def test_measured_row_detected_and_prose_ignored(self):
        self.assertTrue(report_has_measured_number(MEASURED_REPORT))
        self.assertFalse(report_has_measured_number(PROSE_ONLY_REPORT))

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
