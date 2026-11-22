"""Regression tests for the reports-regeneration gate (#1305).

Ledger §8: every remediation ships a test that fails on the original defect.
The defect here was acceptance-2 of #1305: the generators behind the
publication-facing reports had NO committed, fail-closed regeneration path, so
a reader could not confirm the committed artifacts still matched the
generators' output. These tests pin the drift-detection contracts of
eval/benchmarks/reports_regen.py without re-running the generators (the gate
itself runs in `make reports-regen` / `make ci`):

  * a byte drift in a markdown snapshot is detected with a diff (fails closed);
  * a value drift in a compared JSON key is detected;
  * a NEW generator key that is neither compared nor excluded is an error, so
    new fields can never silently escape the gate;
  * the cohort payload comparison excludes exactly the named host-dependent
    fields — latency/throughput are never compared, the deterministic plan
    contract always is;
  * the snapshot set is exactly the four artifacts the gate documents.
"""
import importlib.util
import json
import os
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))

_spec = importlib.util.spec_from_file_location(
    "reports_regen", os.path.join(HERE, "reports_regen.py"))
regen = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(regen)

SNAPSHOT_DIR = regen.SNAPSHOT_DIR


class ByteDiffTests(unittest.TestCase):
    def test_identical_text_yields_no_diff(self):
        self.assertIsNone(regen._diff_text("a\nb\n", "a\nb\n", "x.md"))

    def test_drifted_text_yields_diff(self):
        diff = regen._diff_text("a\nb\n", "a\nB\n", "x.md")
        self.assertIsNotNone(diff)
        self.assertIn("-b", diff)
        self.assertIn("+B", diff)


class ComparedKeyDiffTests(unittest.TestCase):
    COMPARED = {"evidence_mode", "inferential", "interpretation_warning",
                "target_topology", "results"}
    EXCLUDED = {"host_inventory"}

    def _snapshot(self, extra=None, drop=None, value=None):
        obj = {
            "evidence_mode": "model_projection",
            "inferential": False,
            "host_inventory": {"cores": 10, "os": "Darwin"},
            "interpretation_warning": "not empirical",
            "target_topology": {"description": "fixed"},
            "results": [1, 2, 3],
        }
        if extra:
            obj[extra] = "sneaky"
        if drop:
            del obj[drop]
        if value is not None:
            obj["results"] = value
        return json.dumps(obj)

    def test_identical_compared_keys_pass(self):
        self.assertIsNone(regen._diff_compared_keys(
            self._snapshot(), self._snapshot(), "hw.json",
            self.COMPARED, self.EXCLUDED))

    def test_host_inventory_drift_is_excluded_not_compared(self):
        # The whole point of the exclusion: host identity differences must
        # NOT fail the gate (they are not measurements).
        other_host = self._snapshot().replace('"cores": 10', '"cores": 4')
        self.assertIsNone(regen._diff_compared_keys(
            self._snapshot(), other_host, "hw.json",
            self.COMPARED, self.EXCLUDED))

    def test_compared_value_drift_fails_closed(self):
        diff = regen._diff_compared_keys(
            self._snapshot(), self._snapshot(value=[9]), "hw.json",
            self.COMPARED, self.EXCLUDED)
        self.assertIsNotNone(diff)
        self.assertIn("results", diff)

    def test_new_generator_key_fails_closed(self):
        # A generator that grows a new field must never silently escape the
        # gate: an unknown key is an error even when every compared key agrees.
        problems = regen._diff_compared_keys(
            self._snapshot(), self._snapshot(extra="new_metric"), "hw.json",
            self.COMPARED, self.EXCLUDED)
        self.assertIsNotNone(problems)
        self.assertIn("new_metric", problems)
        self.assertIn("neither compared nor explicitly excluded", problems)


class CohortPayloadTests(unittest.TestCase):
    ROW = {
        "num_samples": 10,
        "dag_nodes": 30,
        "expansion_latency_ms": 2462.5,
        "parse_success": True,
        "measurement_valid": True,
        "throughput_nodes_per_sec": 12.2,
    }

    def _summary(self):
        return json.dumps({"benchmark": "cohort_expansion_latency",
                           "results": [dict(self.ROW)]})

    def test_latency_and_throughput_excluded_plan_contract_kept(self):
        payload = regen._cohort_payload(self._summary())
        self.assertEqual(payload["results[10].dag_nodes"], 30)
        self.assertTrue(payload["results[10].parse_success"])
        self.assertTrue(payload["results[10].measurement_valid"])
        for key in payload:
            self.assertNotIn("latency", key)
            self.assertNotIn("throughput", key)

    def test_latency_drift_does_not_change_payload(self):
        drifted = dict(self.ROW, expansion_latency_ms=0.01,
                       throughput_nodes_per_sec=99999.9)
        other = json.dumps({"benchmark": "cohort_expansion_latency",
                            "results": [drifted]})
        self.assertEqual(
            regen._cohort_payload(self._summary()),
            regen._cohort_payload(other))

    def test_node_count_drift_changes_payload(self):
        drifted = dict(self.ROW, dag_nodes=31)
        other = json.dumps({"benchmark": "cohort_expansion_latency",
                            "results": [drifted]})
        self.assertNotEqual(
            regen._cohort_payload(self._summary()),
            regen._cohort_payload(other))

    def test_new_row_field_fails_closed(self):
        drifted = dict(self.ROW, new_field=1)
        other = json.dumps({"benchmark": "cohort_expansion_latency",
                            "results": [drifted]})
        with self.assertRaisesRegex(RuntimeError, "new_field"):
            regen._cohort_payload(other)

    def test_new_top_level_key_fails_closed(self):
        other = json.dumps({"benchmark": "b", "results": [dict(self.ROW)],
                            "meta": {}})
        with self.assertRaisesRegex(RuntimeError, "meta"):
            regen._cohort_payload(other)


class SnapshotSetTests(unittest.TestCase):
    def test_all_documented_snapshots_are_committed(self):
        # The gate documents exactly four snapshots; a missing file would make
        # `--check` crash instead of diff, which is a worse failure mode.
        expected = {
            "sus_survey_harness_check.md",
            "statistical_synthetic_fixture.md",
            "hardware_scaling_projection.json",
            "cohort_scaling_n10.json",
        }
        present = set(os.listdir(SNAPSHOT_DIR))
        self.assertEqual(present, expected)

    def test_manifest_covers_all_four_reports(self):
        # Acceptance 1 of #1305: EVERY publication-facing report carries a
        # machine-readable provenance record. If a new report lands under
        # docs/superpowers/plans/ with a measured number, provenance-check
        # fails; this test additionally pins that the four named reports are
        # manifest keys.
        import sys

        sys.path.insert(0, HERE)
        from provenance_manifest import build_manifest

        artifacts = build_manifest()["artifacts"]
        for name in (
            "docs/superpowers/plans/cohort-scaling-report.md",
            "docs/superpowers/plans/sus-survey-report.md",
            "docs/superpowers/plans/statistical-benchmark-report.md",
            "docs/superpowers/plans/hardware-scaling-report.md",
            "docs/superpowers/plans/industrial-scientific-implementation-plan.md",
        ):
            self.assertIn(name, artifacts)


if __name__ == "__main__":
    unittest.main()
