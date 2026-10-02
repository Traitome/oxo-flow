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


class VersionGateTests(unittest.TestCase):
    """#172: version_match is scored only when the query asks for a version
    (schema.md: expected_version is empty when the query does not ask; 45
    rows carried a version anyway and penalised correct answers)."""

    def _judge(self, query, expected_version, answer):
        with tempfile.TemporaryDirectory() as td:
            captures = Path(td) / "answers.csv"
            with open(captures, "w", newline="", encoding="utf-8") as fh:
                writer = csv.DictWriter(fh, fieldnames=["id", "trial", "answer", "error"])
                writer.writeheader()
                writer.writerow({"id": "tool-900", "trial": 1, "answer": answer, "error": ""})
            gold_rows = [
                {
                    "id": "tool-900",
                    "query": query,
                    "expected_tool": "fastp",
                    "expected_version": expected_version,
                    "negative_sample": "0",
                }
            ]
            return runner.judge_tool(gold_rows, str(captures))[0]

    def test_version_not_scored_when_query_never_asks(self):
        row = self._judge("adapter trimming and quality filtering", "1.3.7", "fastp")
        self.assertNotIn("version_match", row)
        self.assertEqual(row["overall"], 1.0)

    def test_version_scored_when_query_asks(self):
        row = self._judge("what is the latest version of fastp in bioconda", "1.3.7", "fastp 1.3.7")
        self.assertEqual(row["version_match"], 1.0)

    def test_missing_version_fails_when_query_asks(self):
        row = self._judge("what is the latest version of fastp in bioconda", "1.3.7", "fastp")
        self.assertEqual(row["version_match"], 0.0)


class DotEdgeParsingTests(unittest.TestCase):
    def test_parse_dot_edges_maps_labels(self):
        dot = 'digraph {\n    0 [ label = "fastp"]\n    1 [ label = "multiqc"]\n    0 -> 1 [ ]\n}'
        self.assertEqual(runner.parse_dot_edges(dot), {("fastp", "multiqc")})

    def test_parse_dot_edges_ignores_dangling_references(self):
        dot = 'digraph {\n    0 [ label = "a"]\n    0 -> 9 [ ]\n}'
        self.assertEqual(runner.parse_dot_edges(dot), set())

    def test_parse_dot_edges_empty_graph(self):
        self.assertEqual(runner.parse_dot_edges("digraph {\n}"), set())


class NegativeSampleJudgingTests(unittest.TestCase):
    """Regression tests for #172: a correct rejection of a fabricated tool has
    to be earnable — the answer necessarily echoes the queried name — while a
    hallucinated fallback suggestion must still fail."""

    def _judge(self, query, answer, known_names):
        with tempfile.TemporaryDirectory() as td:
            captures = Path(td) / "answers.csv"
            with open(captures, "w", newline="", encoding="utf-8") as fh:
                writer = csv.DictWriter(fh, fieldnames=["id", "trial", "answer", "error"])
                writer.writeheader()
                writer.writerow({"id": "tool-900", "trial": 1, "answer": answer, "error": ""})
            gold_rows = [
                {
                    "id": "tool-900",
                    "query": query,
                    "expected_tool": "",
                    "expected_version": "",
                    "negative_sample": "1",
                }
            ]
            with mock.patch("common.known_tool_names", return_value=known_names):
                results = runner.judge_tool(gold_rows, str(captures))
            return results[0]["no_hallucination"]

    def test_correct_rejection_naming_the_tool_and_databases_scores(self):
        # `fastq_super_cleaner` embeds the real tool name `fastq`; the queried
        # fake name must be excluded before scanning, and short KB names
        # (`bio`, `fc`) must not match inside longer words (`bioconda`,
        # `nf-core`).
        score = self._judge(
            "what is fastq_super_cleaner",
            "I could not find any tool named fastq_super_cleaner. "
            "It does not exist in bioconda or nf-core.",
            {"bio", "bioc", "fc", "fastq", "fastqc"},
        )
        self.assertEqual(score, 1.0)

    def test_spaced_echo_of_the_queried_name_is_excluded(self):
        score = self._judge(
            "what is variant_valkyrie",
            "No tool called variant valkyrie exists in bioconda.",
            {"variantvalkyrie", "bwa"},
        )
        self.assertEqual(score, 1.0)

    def test_rejection_suggesting_a_real_fallback_still_fails(self):
        score = self._judge(
            "what is fastq_super_cleaner",
            "No tool named fastq_super_cleaner — maybe you meant fastqc?",
            {"fastq", "fastqc"},
        )
        self.assertEqual(score, 0.0)

    def test_mentioning_any_other_known_tool_fails(self):
        score = self._judge(
            "what does aligninator do",
            "I could not find aligninator; it is not related to bwameth.",
            {"bwameth"},
        )
        self.assertEqual(score, 0.0)

    def test_short_names_do_not_match_inside_longer_words(self):
        self.assertFalse(
            runner.answer_mentions_known_tool(
                "It is not in bioconda or nf-core.", {"bio", "fc"}
            )
        )
        self.assertTrue(
            runner.answer_mentions_known_tool("Not found; use bwa instead.", {"bwa"})
        )


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
