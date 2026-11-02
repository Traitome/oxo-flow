import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from cohort_driver import BenchmarkError, benchmark_cohort_expansion


class CohortDriverTests(unittest.TestCase):
    def test_uses_actual_plan_length(self):
        completed = subprocess.CompletedProcess(
            args=[],
            returncode=0,
            stdout=json.dumps({"plan": [{"name": "a"}, {"name": "b"}]}),
            stderr="",
        )
        with tempfile.TemporaryDirectory() as tmp, mock.patch(
            "cohort_driver.subprocess.run", return_value=completed
        ):
            result = benchmark_cohort_expansion("oxo-flow", 1, Path(tmp))
        self.assertEqual(result["dag_nodes"], 2)
        self.assertGreater(result["throughput_nodes_per_sec"], 0)
        self.assertTrue(result["measurement_valid"])

    def test_missing_plan_is_not_fabricated(self):
        completed = subprocess.CompletedProcess(
            args=[], returncode=0, stdout=json.dumps({"summary": {}}), stderr=""
        )
        with tempfile.TemporaryDirectory() as tmp, mock.patch(
            "cohort_driver.subprocess.run", return_value=completed
        ):
            with self.assertRaisesRegex(BenchmarkError, "no measured 'plan'"):
                benchmark_cohort_expansion("oxo-flow", 10, Path(tmp))

    def test_invalid_json_fails_closed(self):
        completed = subprocess.CompletedProcess(
            args=[], returncode=0, stdout="not-json", stderr=""
        )
        with tempfile.TemporaryDirectory() as tmp, mock.patch(
            "cohort_driver.subprocess.run", return_value=completed
        ):
            with self.assertRaisesRegex(BenchmarkError, "valid JSON"):
                benchmark_cohort_expansion("oxo-flow", 10, Path(tmp))

    def test_dry_run_failure_fails_closed(self):
        completed = subprocess.CompletedProcess(
            args=[], returncode=1, stdout="", stderr="validation failed"
        )
        with tempfile.TemporaryDirectory() as tmp, mock.patch(
            "cohort_driver.subprocess.run", return_value=completed
        ):
            with self.assertRaisesRegex(BenchmarkError, "validation failed"):
                benchmark_cohort_expansion("oxo-flow", 10, Path(tmp))


if __name__ == "__main__":
    unittest.main()
