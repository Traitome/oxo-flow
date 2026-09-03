import csv
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import runner


class ToolJudgingTests(unittest.TestCase):
    def test_negative_answer_that_suggests_real_tool_fails(self):
        with tempfile.TemporaryDirectory() as td:
            captures = Path(td) / "answers.csv"
            with open(captures, "w", newline="", encoding="utf-8") as fh:
                writer = csv.DictWriter(fh, fieldnames=["id", "trial", "answer", "error"])
                writer.writeheader()
                writer.writerow(
                    {
                        "id": "tool-001",
                        "trial": 1,
                        "answer": "not found, maybe you meant fastqc",
                        "error": "",
                    }
                )
            gold_rows = [
                {
                    "id": "tool-001",
                    "expected_tool": "",
                    "expected_version": "",
                    "negative_sample": "1",
                }
            ]
            with mock.patch("common.known_tool_names", return_value={"fastqc"}):
                results = runner.judge_tool(gold_rows, str(captures))
            self.assertEqual(results[0]["no_hallucination"], 0.0)


class SummaryTests(unittest.TestCase):
    def test_per_item_summary_computes_pass_at_k(self):
        rows = [
            {"id": "wf-001", "trial": 1, "overall": 0.5},
            {"id": "wf-001", "trial": 2, "overall": 1.0},
            {"id": "wf-002", "trial": 1, "overall": 0.0},
        ]
        items = runner.per_item_summary(rows)
        self.assertEqual(items[0]["pass_at_k"], 1.0)
        self.assertEqual(items[1]["pass_at_k"], 0.0)

    def test_pick_capture_file_prefers_one_file_per_trial(self):
        with tempfile.TemporaryDirectory() as td:
            item_dir = Path(td) / "wf-001"
            item_dir.mkdir()
            (item_dir / "trial-001.oxoflow").write_text("[workflow]\nname='x'\n", encoding="utf-8")
            nested = item_dir / "trial-002"
            nested.mkdir()
            (nested / "generated.oxoflow").write_text("[workflow]\nname='y'\n", encoding="utf-8")
            files = runner.pick_capture_file(td, "wf-001")
            self.assertEqual([f["trial"] for f in files], [1, 2])


class ValidityWarningTests(unittest.TestCase):
    def test_summary_warns_on_same_family_and_preview_mode(self):
        summary = {"n_items": 2, "by_difficulty": {"easy": {"n": 2}}, "by_query_type": {"alias": {"n": 2}}}
        gold_rows = [{"gold_draft_by": "claude"}, {"gold_draft_by": "claude"}]
        manifest = {"provider": {"kind": "claude"}, "include_unreviewed": True}
        warnings = runner.build_validity_warnings(summary, gold_rows, manifest)
        self.assertTrue(any("same-family" in w or "gold drafter" in w for w in warnings))
        self.assertTrue(any("preview-only" in w for w in warnings))


if __name__ == "__main__":
    unittest.main()
