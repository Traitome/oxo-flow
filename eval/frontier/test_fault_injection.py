import json
import unittest

from fault_injection import FAULT_CASES, assess_case, summarize_results


class FaultStageTests(unittest.TestCase):
    def setUp(self):
        self.case = FAULT_CASES[0]

    def test_stages_are_independent(self):
        lint = json.dumps(
            {
                "diagnostics": [
                    {
                        "code": self.case.expected_diagnostic_code,
                        "rule": self.case.target_rule,
                        "suggestion": "insert a coordinate-sort adapter",
                    }
                ]
            }
        )
        result = assess_case(
            case=self.case,
            lint_stdout=lint,
            dry_output="",
            run_output="",
            run_exit_code=None,
            workflow_before_sha256="abc",
            workflow_after_sha256="abc",
        )

        self.assertTrue(result["static_detected"])
        self.assertTrue(result["diagnostic_attributed"])
        self.assertTrue(result["repair_proposed"])
        self.assertFalse(result["repair_applied"])
        self.assertFalse(result["repair_verified"])
        self.assertFalse(result["workflow_success_on_rerun"])
        self.assertEqual(result["repair_status"], "not_implemented")

    def test_detection_without_suggestion_is_not_a_proposal(self):
        lint = json.dumps(
            {
                "diagnostics": [
                    {
                        "code": self.case.expected_diagnostic_code,
                        "rule": self.case.target_rule,
                        "suggestion": None,
                    }
                ]
            }
        )
        result = assess_case(
            case=self.case,
            lint_stdout=lint,
            dry_output="",
            run_output="",
            run_exit_code=None,
            workflow_before_sha256="abc",
            workflow_after_sha256="abc",
        )
        self.assertTrue(result["static_detected"])
        self.assertFalse(result["repair_proposed"])
        self.assertFalse(result["repair_applied"])

    def test_infrastructure_attribution_uses_runtime_root_cause(self):
        case = next(c for c in FAULT_CASES if c.category == "infrastructure")
        run_output = (
            f"rule {case.target_rule} failed\n"
            "Root Cause: out of memory\n"
            "Suggestion: increase memory\n"
            f"Remediation for {case.target_rule}\n"
        )
        result = assess_case(
            case=case,
            lint_stdout="{}",
            dry_output="",
            run_output=run_output,
            run_exit_code=1,
            workflow_before_sha256="abc",
            workflow_after_sha256="abc",
        )
        self.assertTrue(result["diagnostic_attributed"])
        self.assertTrue(result["repair_proposed"])
        self.assertFalse(result["workflow_success_on_rerun"])

    def test_summary_does_not_relabel_detection_as_healing(self):
        result = {
            "static_detected": True,
            "diagnostic_attributed": True,
            "repair_proposed": True,
            "repair_applied": False,
            "repair_verified": False,
            "workflow_success_on_rerun": False,
        }
        summary = summarize_results([result])
        self.assertEqual(summary["repair_application_rate"], 0.0)
        self.assertEqual(summary["repair_verification_rate"], 0.0)
        self.assertNotIn("success_rate", summary)


if __name__ == "__main__":
    unittest.main()
