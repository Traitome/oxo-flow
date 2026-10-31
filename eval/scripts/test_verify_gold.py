import csv
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import verify_gold


def write_gold_dir(tmp, tool_rows, ledger_rows):
    """Materialize a minimal eval tree: gold CSVs + evidence ledger.

    Uses csv.writer (RFC 4180 quoting) so fixtures survive any review
    comments or JSON cells added later.
    """
    gold_dir = Path(tmp) / "eval" / "gold"
    review_dir = Path(tmp) / "eval" / "review"
    gold_dir.mkdir(parents=True)
    review_dir.mkdir(parents=True)

    for layer, rows in (("tool", tool_rows), ("rule", []), ("workflow", [])):
        with open(gold_dir / f"{layer}.csv", "w", encoding="utf-8", newline="") as fh:
            if rows:
                writer = csv.writer(fh)
                writer.writerow(rows[0].keys())
                writer.writerows([r.values() for r in rows])
            else:
                fh.write("id,review_status,reviewer\n")

    with open(review_dir / "evidence.jsonl", "w", encoding="utf-8") as fh:
        for rec in ledger_rows:
            fh.write(json.dumps(rec) + "\n")


def make_tool_row(**overrides):
    row = {
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
    row.update(overrides)
    return row


def make_evidence_row(tool_row, **overrides):
    rec = {
        "id": tool_row["id"],
        "layer": "tool",
        "verdict": tool_row["review_status"],
        "reviewer": tool_row["reviewer"],
        "verified_at": "2026-10-31T00:00:00Z",
        "provenance_url": tool_row["provenance_url"],
        "source_type": "bioconda_recipe",
        "fields_hash": verify_gold.canonical_fields_hash(tool_row, "tool"),
        "expert_role": "lead_bioinformatician",
        "scientific_justification": "verified",
        "negative_purity_verified": False,
    }
    rec.update(overrides)
    return rec


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
            tool_row = make_tool_row()
            evidence_row = make_evidence_row(tool_row)
            write_gold_dir(tmp, [tool_row], [evidence_row])

            # Tamper a canonical gold field after the ledger was minted
            tampered_row = dict(tool_row)
            tampered_row["expected_tool"] = "tampered_tool"
            with open(Path(tmp) / "eval" / "gold" / "tool.csv", "w", encoding="utf-8", newline="") as fh:
                writer = csv.writer(fh)
                writer.writerow(tampered_row.keys())
                writer.writerow(tampered_row.values())

            res = verify_gold.audit_gold_set(tmp)
            self.assertFalse(res["ok"])
            self.assertIn("cryptographic fields_hash mismatch", res["findings"][0])

    def test_automated_reviewer_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            tool_row = make_tool_row(reviewer="wangshx-automated", review_comment="automated sweep")
            evidence_row = make_evidence_row(tool_row)
            write_gold_dir(tmp, [tool_row], [evidence_row])

            res = verify_gold.audit_gold_set(tmp)
            self.assertFalse(res["ok"])
            self.assertTrue(any("invalid reviewer handle" in f for f in res["findings"]))

    def test_duplicate_gold_id_is_flagged(self):
        with tempfile.TemporaryDirectory() as tmp:
            row_a = make_tool_row()
            row_b = make_tool_row(query="same id, different canonical content")
            evidence = make_evidence_row(row_a)
            write_gold_dir(tmp, [row_a, row_b], [evidence])

            res = verify_gold.audit_gold_set(tmp)
            self.assertFalse(res["ok"])
            self.assertTrue(any("duplicate gold id" in f for f in res["findings"]))
            # The duplicate row is skipped before hashing, so only the first
            # occurrence can contribute a hash match.
            self.assertEqual(res["stats"]["hash_matched"], 1)

    def test_orphan_ledger_record_is_flagged(self):
        with tempfile.TemporaryDirectory() as tmp:
            tool_row = make_tool_row()
            evidence = make_evidence_row(tool_row)
            orphan = make_evidence_row(make_tool_row(id="tool-999"))
            write_gold_dir(tmp, [tool_row], [evidence, orphan])

            res = verify_gold.audit_gold_set(tmp)
            self.assertFalse(res["ok"])
            self.assertTrue(
                any("orphan evidence record" in f and "tool-999" in f for f in res["findings"])
            )


if __name__ == "__main__":
    unittest.main()
