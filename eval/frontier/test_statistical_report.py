import json
import math
import tempfile
import unittest
from pathlib import Path

import statistical_report
from statistical_report import (
    AnalysisError,
    analyze_arms,
    chi2_p_value_df1,
    erfc_approx,
    generate_synthetic_benchmark_dataset,
    holm_adjust_p_values,
    load_benchmark_results,
    mcnemar_test,
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
            for variant in ("unconstrained", "syntax-only", "full-contract"):
                runs.append(
                    {
                        "variant": variant,
                        "intent_id": "rna-seq",
                        "seed": 3,
                        "model": "test-model",
                        "success": variant != "unconstrained",
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
                                "variant": "unconstrained",
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


class FormulaOracleTests(unittest.TestCase):
    """Known-value oracles for the analysis math itself (textbook definitions).

    The pairing/plumbing tests above cannot catch a silently wrong formula;
    these assert parsed known values with justified tolerances.
    """

    def test_erfc_approx_tracks_math_erfc_within_abramowitz_bound(self):
        # Abramowitz & Stegun 7.1.26 guarantees |error| <= 1.5e-7 for x >= 0;
        # the module's negative branch is the exact identity erfc(-x) = 2 - erfc(x).
        for x in (0.0, 0.5, 1.0, 1.5, 2.0, 3.0, -1.0):
            self.assertLessEqual(abs(erfc_approx(x) - math.erfc(x)), 1.5e-7, f"x={x}")
        # Relative error on the p-value range that survives a 0.05 gate.
        for x in (0.5, 1.0, 1.5, 2.0, 3.0):
            self.assertLess(abs(erfc_approx(x) - math.erfc(x)) / math.erfc(x), 1e-3, f"x={x}")

    def test_mcnemar_textbook_small_sample_with_continuity_correction(self):
        # b=1, c=9 discordant pairs (plus 5/3 concordant). The module applies
        # the Edwards continuity correction documented in its docstring:
        # chi2 = ((|b-c|-1)^2)/(b+c) = (8-1)^2/10 = 4.9. (The UNcorrected
        # statistic would be 6.4 with p~0.011; that mapping is asserted on
        # chi2_p_value_df1 directly below.)
        arm1 = [True] * 9 + [False] * 1 + [True] * 5 + [False] * 3
        arm2 = [False] * 9 + [True] * 1 + [True] * 5 + [False] * 3
        res = mcnemar_test(arm1, arm2)
        self.assertEqual((res["a"], res["b"], res["c"], res["d"]), (5, 1, 9, 3))
        self.assertEqual(res["chi2"], 4.9)
        # p = erfc(sqrt(chi2/2)); the approximation may deviate from
        # math.erfc by at most the A&S 7.1.26 bound.
        self.assertAlmostEqual(res["p_value"], math.erfc(math.sqrt(4.9 / 2)), delta=1.5e-7)
        self.assertLess(res["p_value"], 0.05)
        self.assertTrue(res["significant"])
        self.assertEqual(res["odds_ratio"], round(1 / 9, 3))

    def test_chi2_to_p_matches_df1_survival_function(self):
        # chi2_p_value_df1(x) must be erfc(sqrt(x/2)); 6.4 -> ~0.0114 is the
        # classic uncorrected McNemar value for b=1/c=9.
        self.assertAlmostEqual(chi2_p_value_df1(6.4), math.erfc(math.sqrt(6.4 / 2)), delta=1.5e-7)
        self.assertAlmostEqual(chi2_p_value_df1(6.4), 0.0114, places=3)
        self.assertEqual(chi2_p_value_df1(0.0), 1.0)

    def test_pass_at_k_matches_hand_computed_unbiased_estimator(self):
        # Chen et al. (HumanEval) unbiased estimator, cross-checked against the
        # exact combinatorial form 1 - C(n-c, k)/C(n, k).
        self.assertEqual(pass_at_k(5, 2, 2), 0.7)  # 1 - (3/5)(2/4)
        self.assertAlmostEqual(
            pass_at_k(10, 3, 3), 1 - math.comb(7, 3) / math.comb(10, 3), places=12
        )
        self.assertAlmostEqual(pass_at_k(10, 3, 3), 17 / 24, places=12)
        # Degenerate shapes: all-correct is a certain pass, none-correct a
        # certain fail at any k.
        self.assertEqual(pass_at_k(10, 10, 4), 1.0)
        self.assertEqual(pass_at_k(7, 0, 3), 0.0)

    def test_holm_step_down_known_ordering_monotonicity_and_cap(self):
        comparisons = {
            "strong": {"p_value": 0.01},
            "moderate": {"p_value": 0.04},
            "null": {"p_value": 0.90},
        }
        holm_adjust_p_values(comparisons)
        # m - rank multipliers: 3*0.01, 2*0.04, 1*0.90.
        self.assertEqual(comparisons["strong"]["p_value_holm"], 0.03)
        self.assertEqual(comparisons["moderate"]["p_value_holm"], 0.08)
        self.assertEqual(comparisons["null"]["p_value_holm"], 0.9)
        self.assertTrue(comparisons["strong"]["significant"])
        self.assertFalse(comparisons["moderate"]["significant"])
        self.assertFalse(comparisons["null"]["significant"])
        # Raw p-values are kept alongside the adjusted ones.
        self.assertEqual(comparisons["strong"]["p_value"], 0.01)

        # 2*0.012 = 0.024 would undercut the first adjusted value (0.03);
        # the step-down running maximum must enforce monotonicity, and the
        # 1.0 cap must hold for multipliers that overflow.
        comparisons = {
            "a": {"p_value": 0.01},
            "b": {"p_value": 0.012},
            "c": {"p_value": 0.50},
        }
        holm_adjust_p_values(comparisons)
        self.assertEqual(comparisons["a"]["p_value_holm"], 0.03)
        self.assertEqual(comparisons["b"]["p_value_holm"], 0.03)
        self.assertEqual(comparisons["c"]["p_value_holm"], 0.5)


class ArmsAblationHarnessTests(unittest.TestCase):
    """Issue #1315: the harness must emit exactly the three pre-registered arms
    and the analysis must pair them (plan:109-111). Today's regressions were a
    two-variant taxonomy and a ``--profile full`` knob that silently recorded
    the cli variant, so arm_c was never populated.
    """

    def test_harness_emits_exactly_three_arm_variants(self):
        import run_eval

        self.assertEqual(
            list(run_eval.ARMS),
            ["unconstrained", "syntax-only", "full-contract"],
        )
        # The variants the loader recognises must equal the harness's arms:
        # derived, not hand-copied (reuse-never-retype).
        from statistical_report import load_benchmark_results as _load

        source = Path(statistical_report.__file__).read_text()
        for variant in run_eval.ARMS:
            self.assertIn(f'"{variant}"', source)
        # The retired two-variant names must be gone from the mapping.
        for retired in ("naive-prompt", "cli", "full"):
            self.assertNotIn(f'"{retired}":', source)

    def test_analyze_arms_pairs_abc_on_synthetic_matched_set(self):
        report = analyze_arms(generate_synthetic_benchmark_dataset(6))
        for arm in ("arm_a", "arm_b", "arm_c"):
            self.assertIn(arm, report["metrics"])
            self.assertEqual(report["metrics"][arm]["total_samples"], 6)

    def test_loader_fails_loudly_on_unknown_arm_variant(self):
        # Guard: ``--profile full`` used to leave the recorded variant as the
        # cli name, silently starving arm_c. A variant outside the three arms
        # must now abort, never skip.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "results.json").write_text(
                json.dumps(
                    {
                        "runs": [
                            {
                                "variant": "full",
                                "intent_id": "rna-seq",
                                "seed": 1,
                                "model": "m",
                                "success": True,
                            }
                        ]
                    }
                )
            )
            with self.assertRaisesRegex(AnalysisError, "unknown variant"):
                load_benchmark_results(root)

    def test_loader_fails_loudly_when_an_arm_is_empty(self):
        # A matched file that carries only Arm A must not silently yield an
        # empty arm_b/arm_c and a fail-closed report from the wrong layer.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "results.json").write_text(
                json.dumps(
                    {
                        "runs": [
                            {
                                "variant": "unconstrained",
                                "intent_id": "rna-seq",
                                "seed": 1,
                                "model": "m",
                                "success": True,
                            }
                        ]
                    }
                )
            )
            dataset = load_benchmark_results(root)
            with self.assertRaisesRegex(AnalysisError, "arm_b has no benchmark runs"):
                validate_paired_dataset(dataset)


class ArmFeedbackToggleTests(unittest.TestCase):
    """Golden check: the Arm B / Arm C split must be an ACTUAL feedback toggle.

    Arm B (``syntax-only``) omits ``--ai-contract-feedback``; Arm C
    (``full-contract``) enables it. The engine flag is the only difference,
    so the harness source must wire it by variant, not by label.
    """

    def test_full_contract_enables_contract_feedback_flag(self):
        import run_eval

        source = Path(run_eval.__file__).read_text()
        self.assertIn('"--ai-contract-feedback"', source)
        # The flag is added only for the full-contract (Arm C) variant.
        self.assertIn('variant == "full-contract"', source)

    def test_contract_feedback_flag_is_opt_in_default_off(self):
        # The engine default (no flag) is Arm B; the flag must be added only
        # for Arm C, else Arm B and Arm C collapse into one measurement.
        import run_eval

        source = Path(run_eval.__file__).read_text()
        # The base command (shared by both CLI arms) does NOT carry the flag.
        shared = next(
            line for line in source.splitlines()
            if line.strip().startswith('cmd = [binary, "template"')
        )
        self.assertNotIn("--ai-contract-feedback", shared)
        # The flag appears exactly once, inside the full-contract branch.
        self.assertEqual(source.count('"--ai-contract-feedback"'), 1)


if __name__ == "__main__":
    unittest.main()
