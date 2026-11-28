"""Tests for the committed post-hoc analysis utilities (issue #1395).

No network and no real campaign data: a tiny synthetic results directory
exercises subset selection, transport classification, session outcomes,
and the matched session-level comparison.
"""

import json
import tempfile
import unittest
from pathlib import Path

import mechanism_analysis
import posthoc_analysis


class SubsetSelectionTests(unittest.TestCase):
    def test_four_populations_are_nested_as_audited(self):
        intents = {"qc-fastqc", "spatial-visium", "qc-vague"}
        self.assertEqual(posthoc_analysis.subset_intents("all36", intents),
                         {"qc-fastqc", "spatial-visium", "qc-vague"})
        self.assertEqual(posthoc_analysis.subset_intents("specified33", intents),
                         {"qc-fastqc", "spatial-visium"})
        self.assertEqual(posthoc_analysis.subset_intents("sound15", intents),
                         {"qc-fastqc", "spatial-visium"})
        self.assertEqual(posthoc_analysis.subset_intents("vague3", intents),
                         {"qc-vague"})
        with self.assertRaises(ValueError):
            posthoc_analysis.subset_intents("unknown", intents)


class SessionClassificationTests(unittest.TestCase):
    def test_urls_are_sanitized(self):
        text = mechanism_analysis.sanitize_text(
            "error sending request for url (http://private.example/v1/chat)")
        self.assertNotIn("private.example", text)
        self.assertIn("<provider-endpoint>", text)

    def test_transport_failure_requires_zero_tokens_and_no_artifact(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / "session.json").write_text(json.dumps({
                "error": "provider error: AI request failed after 3 attempts: "
                         "error sending request for url (http://x.example/v1)",
                "outcome": "Failed",
            }))
            run = {"generated": False, "input_tokens": 0, "output_tokens": 0,
                   "error": "template exited 1"}
            self.assertTrue(mechanism_analysis.is_transport_failure(run, path))
            self.assertFalse(mechanism_analysis.is_transport_failure(
                {**run, "generated": True}, path))
            self.assertTrue(mechanism_analysis.is_transport_failure(
                {**run, "error": "Remote end closed connection without response"}, path))


class SyntheticCampaignTests(unittest.TestCase):
    def _write_campaign(self, root: Path) -> None:
        runs = []
        for intent in ("qc-fastqc", "qc-vague"):
            for seed in (1, 2):
                for variant in posthoc_analysis.ARMS:
                    success = not (variant == "unconstrained")
                    generated = success
                    run = {
                        "intent_id": intent, "tier": "easy", "domain": "qc",
                        "vague": intent == "qc-vague", "seed": seed,
                        "variant": variant, "model": "test-model",
                        "success": success, "generated": generated,
                        "input_tokens": 10 if generated else 0,
                        "output_tokens": 5 if generated else 0,
                        "est_cost_usd": 0.001 if generated else 0.0,
                        "wall_s": 1.0, "gates": {}, "all_gates_pass": success,
                        "missing_required_tools": [], "fidelity_pass": True,
                        "error": None if generated else "template exited 1",
                        "generation_command": None, "workflow_file": None,
                    }
                    runs.append(run)
                    cell = root / f"test-model__{variant}__{intent}__s{seed}"
                    cell.mkdir(parents=True)
                    outcome = "Success"
                    error = None
                    if variant == "full-contract" and intent == "qc-fastqc" and seed == 1:
                        outcome = "Failed"
                        error = "exceeded max rounds (10) without valid output"
                    (cell / "session.json").write_text(json.dumps({
                        "started_at": f"2026-01-01T00:00:0{seed}Z",
                        "ended_at": f"2026-01-01T00:00:1{seed}Z",
                        "outcome": outcome,
                        "error": error,
                        "messages": [{"role": "user", "content": "x"}],
                        "tool_calls": [],
                    }))
        (root / "results.json").write_text(json.dumps({
            "stamp": "synthetic", "seeds": 2, "runs": runs,
        }))

    def test_session_outcomes_and_matched_comparison(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self._write_campaign(root)
            report = mechanism_analysis.analyze(root)
            c = report["session_outcomes"]["full-contract"]
            self.assertEqual(c["gate_successes"], {"Success": 3, "Failed": 1})
            failed = c["gate_success_session_failed"]
            self.assertEqual(len(failed), 1)
            self.assertEqual(failed[0]["intent_id"], "qc-fastqc")
            matched = report["per_protocol_matched"]["session"]
            self.assertEqual((matched["b_success"], matched["c_success"]), (4, 3))
            self.assertEqual((matched["b_only_pass"], matched["c_only_pass"]), (1, 0))

            posthoc = posthoc_analysis.analyze(root)
            self.assertEqual(posthoc["subsets"]["all36"]["n_intents"], 2)
            self.assertEqual(posthoc["subsets"]["specified33"]["n_intents"], 1)
            self.assertEqual(posthoc["subsets"]["sound15"]["n_intents"], 1)
            self.assertEqual(posthoc["subsets"]["vague3"]["n_intents"], 1)


if __name__ == "__main__":
    unittest.main()
