import json
import tempfile
import unittest
from pathlib import Path

from statistical_report import (
    AnalysisError,
    analyze_arms,
    generate_synthetic_benchmark_dataset,
    load_benchmark_results,
    pass_at_k,
    render_markdown_report,
    validate_paired_dataset,
)


class StatisticalReportTests(unittest.TestCase):
    def test_synthetic_report_is_explicitly_noninferential(self):
        report = analyze_arms(generate_synthetic_benchmark_dataset(4))
        markdown = render_markdown_report(report)
        self.assertFalse(report["inferential"])
        self.assertIn("NOT EXPERIMENTAL EVIDENCE", markdown)
        self.assertNotIn("demonstrates statistically superior", markdown)

    def test_loader_preserves_pairing_identity(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            runs = []
            for variant in ("naive-prompt", "cli", "full"):
                runs.append(
                    {
                        "variant": variant,
                        "intent_id": "rna-seq",
                        "seed": 3,
                        "model": "test-model",
                        "success": variant != "naive-prompt",
                        "input_tokens": 10,
                        "output_tokens": 5,
                        "est_cost_usd": 0.1,
                    }
                )
            (root / "results.json").write_text(json.dumps({"runs": runs}))
            dataset = load_benchmark_results(root)
            self.assertEqual(validate_paired_dataset(dataset), [("test-model", "rna-seq", 3)])
            self.assertEqual(dataset["arm_b"][0]["intent_id"], "rna-seq")

    def test_loader_rejects_missing_pairing_identity(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "results.json").write_text(
                json.dumps(
                    {
                        "runs": [
                            {
                                "variant": "naive-prompt",
                                "success": True,
                                "model": "m",
                            }
                        ]
                    }
                )
            )
            dataset = load_benchmark_results(root)
            with self.assertRaises(AnalysisError):
                validate_paired_dataset(dataset)

    def test_unpaired_arms_fail_closed(self):
        dataset = generate_synthetic_benchmark_dataset(3)
        dataset["arm_b"] = dataset["arm_b"][:-1]
        with self.assertRaisesRegex(AnalysisError, "not paired"):
            analyze_arms(dataset)

    def test_duplicate_cell_fails_closed(self):
        dataset = generate_synthetic_benchmark_dataset(2)
        dataset["arm_c"].append(dict(dataset["arm_c"][0]))
        with self.assertRaisesRegex(AnalysisError, "duplicate paired cell"):
            validate_paired_dataset(dataset)

    def test_pass_at_k_empty_set_fails_closed(self):
        # Regression: the `n - c < k` shortcut used to run before the domain
        # guard, so an empty set estimated pass@1 as 1.0 instead of 0.0.
        self.assertEqual(pass_at_k(0, 0, 1), 0.0)

    def test_pass_at_k_k_exceeding_n_fails_closed(self):
        self.assertEqual(pass_at_k(2, 2, 3), 0.0)


if __name__ == "__main__":
    unittest.main()
