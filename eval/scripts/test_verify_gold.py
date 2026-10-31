import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import verify_gold


class VerifyGoldTests(unittest.TestCase):
    def test_canonical_fields_hash_deterministic(self):
        row1 = {"query": "align reads", "expected_tool": "bwa", "expected_source": "bioconda"}
        row2 = {"expected_source": "bioconda", "query": "align reads", "expected_tool": "bwa"}
        h1 = verify_gold.canonical_fields_hash(row1, "tool")
        h2 = verify_gold.canonical_fields_hash(row2, "tool")
        self.assertEqual(h1, h2)
        self.assertEqual(len(h1), 64)

    def test_audit_passes_on_current_repo(self):
        res = verify_gold.audit_gold_set(verify_gold.REPO_ROOT)
        self.assertTrue(res["ok"], f"Expected audit to pass, got: {res.get('findings')}")
        stats = res["stats"]
        self.assertEqual(stats["total_gold_rows"], 257)
        self.assertEqual(stats["total_evidence_rows"], 257)
        self.assertEqual(stats["hash_matched"], 257)
        self.assertEqual(stats["hash_mismatched"], 0)
        self.assertEqual(stats["reviewer_invalid"], 0)
        self.assertEqual(stats["rejected"], 6)

    def test_tampered_gold_field_triggers_hash_mismatch(self):
        with tempfile.TemporaryDirectory() as tmp:
            gold_dir = Path(tmp) / "eval" / "gold"
            review_dir = Path(tmp) / "eval" / "review"
            gold_dir.mkdir(parents=True)
            review_dir.mkdir(parents=True)

            # Minimal 1-row tool set
            tool_row = {
                "id": "tool-001",
                "layer": "tool",
                "query": "adapter trimming",
                "query_type": "purpose",
                "expected_tool": "fastp",
                "expected_version": "",
                "expected_source": "bioconda",
                "negative_sample": "0",
                "provenance_url": "https://example.com/fastp",
                "provenance_date": "2026-10-31",
                "difficulty": "easy",
                "gold_draft_by": "claude",
                "review_status": "approved",
                "reviewer": "wangshx",
                "review_comment": "verified",
                "review_date": "2026-10-31",
            }

            # Empty rule/workflow files
            for layer in ("rule", "workflow"):
                with open(gold_dir / f"{layer}.csv", "w", encoding="utf-8") as fh:
                    fh.write("id,review_status,reviewer\n")

            h = verify_gold.canonical_fields_hash(tool_row, "tool")
            evidence_row = {
                "id": "tool-001",
                "layer": "tool",
                "verdict": "approved",
                "reviewer": "wangshx",
                "verified_at": "2026-10-31T00:00:00Z",
                "provenance_url": "https://example.com/fastp",
                "source_type": "bioconda_recipe",
                "fields_hash": h,
                "expert_role": "lead_bioinformatician",
                "scientific_justification": "verified",
                "negative_purity_verified": False,
            }

            # Write valid state
            with open(review_dir / "evidence.jsonl", "w", encoding="utf-8") as fh:
                fh.write(json.dumps(evidence_row) + "\n")

            # Write CSV with tampered tool
            tampered_row = dict(tool_row)
            tampered_row["expected_tool"] = "tampered_tool"
            with open(gold_dir / "tool.csv", "w", encoding="utf-8") as fh:
                fh.write(",".join(tool_row.keys()) + "\n")
                fh.write(",".join(str(tampered_row[k]) for k in tool_row.keys()) + "\n")

            res = verify_gold.audit_gold_set(tmp)
            self.assertFalse(res["ok"])
            self.assertIn("cryptographic fields_hash mismatch", res["findings"][0])

    def test_automated_reviewer_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            gold_dir = Path(tmp) / "eval" / "gold"
            review_dir = Path(tmp) / "eval" / "review"
            gold_dir.mkdir(parents=True)
            review_dir.mkdir(parents=True)

            tool_row = {
                "id": "tool-001",
                "layer": "tool",
                "query": "adapter trimming",
                "query_type": "purpose",
                "expected_tool": "fastp",
                "expected_version": "",
                "expected_source": "bioconda",
                "negative_sample": "0",
                "provenance_url": "https://example.com/fastp",
                "provenance_date": "2026-10-31",
                "difficulty": "easy",
                "gold_draft_by": "claude",
                "review_status": "approved",
                "reviewer": "wangshx-automated",
                "review_comment": "automated sweep",
                "review_date": "2026-10-31",
            }
            for layer in ("rule", "workflow"):
                with open(gold_dir / f"{layer}.csv", "w", encoding="utf-8") as fh:
                    fh.write("id,review_status,reviewer\n")

            h = verify_gold.canonical_fields_hash(tool_row, "tool")
            evidence_row = {
                "id": "tool-001",
                "layer": "tool",
                "verdict": "approved",
                "reviewer": "wangshx-automated",
                "verified_at": "2026-10-31T00:00:00Z",
                "provenance_url": "https://example.com/fastp",
                "source_type": "bioconda_recipe",
                "fields_hash": h,
                "expert_role": "lead_bioinformatician",
                "scientific_justification": "verified",
                "negative_purity_verified": False,
            }
            with open(review_dir / "evidence.jsonl", "w", encoding="utf-8") as fh:
                fh.write(json.dumps(evidence_row) + "\n")

            with open(gold_dir / "tool.csv", "w", encoding="utf-8") as fh:
                fh.write(",".join(tool_row.keys()) + "\n")
                fh.write(",".join(str(tool_row[k]) for k in tool_row.keys()) + "\n")

            res = verify_gold.audit_gold_set(tmp)
            self.assertFalse(res["ok"])
            self.assertTrue(any("invalid reviewer handle" in f for f in res["findings"]))


if __name__ == "__main__":
    unittest.main()
