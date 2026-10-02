import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_gold


class NegativeNameTests(unittest.TestCase):
    def test_fabricated_name_colliding_with_kb_is_flagged(self):
        rows = [{"id": "tool-046", "negative_sample": "1", "query": "what is bwa_mem4"}]
        findings, checked = check_gold.check_negative_names(rows, {"bwamem4"})
        self.assertEqual(checked, 1)
        self.assertEqual(len(findings), 1)
        self.assertIn("bwa_mem4", findings[0])

    def test_embedded_real_name_in_longer_fake_is_not_a_collision(self):
        # `fastq_super_cleaner` contains the real tool `fastq`, but the whole
        # fabricated name must collide to invalidate the item.
        rows = [{"id": "tool-048", "negative_sample": "1", "query": "what is fastq_super_cleaner"}]
        findings, checked = check_gold.check_negative_names(rows, {"fastq"})
        self.assertEqual(findings, [])
        self.assertEqual(checked, 1)

    def test_unparseable_query_is_flagged(self):
        rows = [{"id": "tool-999", "negative_sample": "1", "query": "tell me about something"}]
        findings, checked = check_gold.check_negative_names(rows, set())
        self.assertEqual(checked, 0)
        self.assertEqual(len(findings), 1)


class VersionDriftTests(unittest.TestCase):
    def test_mismatch_flagged_and_unreviewed_rows_ignored(self):
        rows = [
            {"id": "tool-001", "negative_sample": "0", "review_status": "approved",
             "expected_tool": "fastp", "expected_version": "1.3.6"},
            {"id": "tool-002", "negative_sample": "0", "review_status": "draft",
             "expected_tool": "fastp", "expected_version": "1.3.6"},
            {"id": "tool-003", "negative_sample": "0", "review_status": "approved",
             "expected_tool": "not-in-kb", "expected_version": "1.0"},
            {"id": "tool-004", "negative_sample": "1", "review_status": "approved",
             "expected_tool": "fastp", "expected_version": "1.3.6"},
        ]
        findings, checked = check_gold.check_version_drift(rows, {"fastp": "1.3.7"})
        self.assertEqual(checked, 1)
        self.assertEqual(len(findings), 1)
        self.assertIn("tool-001", findings[0])


class ProvenanceTests(unittest.TestCase):
    def test_approved_non_negative_row_without_url_is_flagged(self):
        rows = [{"id": "tool-041", "negative_sample": "0", "review_status": "approved",
                 "provenance_url": "", "review_comment": ""}]
        findings, checked = check_gold.check_provenance(rows)
        self.assertEqual(checked, 1)
        self.assertEqual(len(findings), 1)

    def test_resolves_claim_without_url_is_flagged_even_for_negatives(self):
        rows = [{"id": "tool-046", "negative_sample": "1", "review_status": "approved",
                 "provenance_url": "", "review_comment": "provenance URL resolves (sweep)"}]
        findings, _ = check_gold.check_provenance(rows)
        self.assertEqual(len(findings), 1)
        self.assertIn("resolves", findings[0])

    def test_negative_row_with_clean_comment_passes(self):
        rows = [{"id": "tool-046", "negative_sample": "1", "review_status": "approved",
                 "provenance_url": "", "review_comment": "no provenance URL by construction"}]
        findings, _ = check_gold.check_provenance(rows)
        self.assertEqual(findings, [])


class WorkflowEdgeTests(unittest.TestCase):
    def _repo_with_reference(self, tmp):
        ref_dir = Path(tmp) / "examples" / "gallery"
        ref_dir.mkdir(parents=True)
        (ref_dir / "wf.oxoflow").write_text(
            '[workflow]\nname = "x"\n\n'
            '[[rules]]\nname = "a"\noutput = ["x.txt"]\n\n'
            '[[rules]]\nname = "b"\ninput = ["x.txt"]\noutput = ["y.txt"]\n',
            encoding="utf-8",
        )
        return tmp

    def _row(self):
        return {
            "id": "wf-004",
            "reference_repo": "examples/gallery",
            "reference_file": "wf.oxoflow",
            "expected_steps": json.dumps(["a", "b"]),
            "expected_dag_edges": json.dumps([["a", "b"]]),
        }

    def test_declared_edge_absent_from_engine_graph_is_flagged(self):
        with tempfile.TemporaryDirectory() as tmp:
            self._repo_with_reference(tmp)
            findings, checked, skipped, _ = check_gold.check_workflow_edges(
                [self._row()], tmp, lambda path: (set(), set(), None)
            )
        self.assertEqual((checked, skipped), (1, 0))
        self.assertEqual(len(findings), 1)
        self.assertIn("a -> b", findings[0])

    def test_declared_edge_present_in_engine_graph_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            self._repo_with_reference(tmp)
            findings, checked, _, _ = check_gold.check_workflow_edges(
                [self._row()], tmp, lambda path: ({"a", "b"}, {("a", "b")}, None)
            )
        self.assertEqual(findings, [])
        self.assertEqual(checked, 1)

    def test_engine_error_is_reported(self):
        with tempfile.TemporaryDirectory() as tmp:
            self._repo_with_reference(tmp)
            findings, _, _, _ = check_gold.check_workflow_edges(
                [self._row()], tmp, lambda path: (set(), set(), "boom")
            )
        self.assertEqual(len(findings), 1)
        self.assertIn("engine graph failed", findings[0])

    def test_community_references_are_skipped_without_fetch(self):
        with tempfile.TemporaryDirectory() as tmp:
            row = self._row()
            row["reference_repo"] = "oxo-flow-rnaseq"
            findings, checked, skipped, warnings = check_gold.check_workflow_edges(
                [row], tmp, lambda path: (set(), set(), None)
            )
        self.assertEqual((findings, checked, skipped, warnings), ([], 0, 1, []))


class ExpectedToolTests(unittest.TestCase):
    def test_tool_absent_from_the_kb_is_flagged(self):
        rows = [{"id": "tool-131", "negative_sample": "0", "review_status": "approved",
                 "expected_tool": "upp_align"}]
        findings, checked = check_gold.check_expected_tools(rows, {"fastqc"})
        self.assertEqual(checked, 1)
        self.assertEqual(len(findings), 1)
        self.assertIn("upp_align", findings[0])

    def test_known_tool_and_unreviewed_rows_are_ignored(self):
        rows = [
            {"id": "tool-001", "negative_sample": "0", "review_status": "approved", "expected_tool": "fastp"},
            {"id": "tool-002", "negative_sample": "0", "review_status": "draft", "expected_tool": "mystery"},
            {"id": "tool-003", "negative_sample": "1", "review_status": "approved", "expected_tool": ""},
        ]
        findings, checked = check_gold.check_expected_tools(rows, {"fastp"})
        self.assertEqual((findings, checked), ([], 1))


class VersionIntentTests(unittest.TestCase):
    def test_query_without_version_is_warned(self):
        rows = [
            {"id": "tool-001", "negative_sample": "0", "query": "adapter trimming", "expected_version": "1.3.6"},
            {"id": "tool-002", "negative_sample": "0", "query": "what is its latest version", "expected_version": "1.0"},
        ]
        self.assertEqual(check_gold.check_version_intent(rows), ["tool-001"])


if __name__ == "__main__":
    unittest.main()
