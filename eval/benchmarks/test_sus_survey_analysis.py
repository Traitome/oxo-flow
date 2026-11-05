import unittest
from contextlib import redirect_stdout
from io import StringIO
from unittest import mock

from sus_survey_analysis import (
    analyze_synthetic_fixture,
    generate_synthetic_fixture,
    main,
    render_markdown_report,
)


class SyntheticSusHarnessTests(unittest.TestCase):
    def test_report_is_explicitly_non_inferential(self):
        report = analyze_synthetic_fixture(generate_synthetic_fixture(8))
        markdown = render_markdown_report(report)

        self.assertIn("SIMULATION ONLY", markdown)
        self.assertIn("performs no hypothesis test", markdown)
        self.assertNotIn("p =", markdown)
        self.assertNotIn("p <", markdown)
        self.assertNotIn("t =", markdown)
        self.assertNotIn("significant", markdown.lower())
        self.assertNotIn("achieved", markdown.lower())
        self.assertNotIn("participants", markdown.lower())
        self.assertNotIn("superior", markdown.lower())
        self.assertNotIn("nextflow", markdown.lower())
        self.assertNotIn("snakemake", markdown.lower())
        self.assertNotIn("oxo-flow", markdown.lower())

    def test_analysis_omits_inferential_results(self):
        report = analyze_synthetic_fixture(generate_synthetic_fixture(6))

        self.assertEqual(report["evidence_status"], "simulation_only")
        self.assertFalse(report["inferential_analysis_performed"])
        self.assertNotIn("comparisons", report)
        self.assertNotIn("p_value", report["metrics"])
        self.assertNotIn("t_stat", report["metrics"])

    def test_renderer_rejects_a_report_that_claims_inference(self):
        report = analyze_synthetic_fixture(generate_synthetic_fixture(6))
        report["inferential_analysis_performed"] = True

        with self.assertRaisesRegex(ValueError, "only non-inferential synthetic reports"):
            render_markdown_report(report)

    def test_cli_uses_fixture_cases_and_emits_no_inferential_summary(self):
        output = StringIO()
        with (
            mock.patch("sys.argv", ["sus_survey_analysis.py", "--cases", "6"]),
            redirect_stdout(output),
        ):
            main()

        text = output.getvalue()
        self.assertIn("SYNTHETIC SUS HARNESS CHECK", text)
        self.assertIn("Synthetic fixture cases: 6", text)
        self.assertIn("No hypothesis test", text)
        self.assertNotIn("p =", text)
        self.assertNotIn("p <", text)


if __name__ == "__main__":
    unittest.main()
