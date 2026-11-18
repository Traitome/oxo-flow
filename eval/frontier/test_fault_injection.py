import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from fault_injection import (
    AI_BLOCK,
    FAULT_CASES,
    _code_in_text,
    assess_case,
    assess_repair_stages,
    build_stub_dir,
    find_repair_record,
    provider_from_env,
    run_test_case,
    summarize_results,
    workflow_with_ai,
)


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


class StaticCodeMatchingTests(unittest.TestCase):
    """The text fallback for static detection must match diagnostic codes with
    one looseness: case-insensitive and word-bounded. The old bare substring
    was case-sensitive ("OOM" missed lowercase output) and unbounded ("disk"
    matched inside the rule name `spill_to_disk`)."""

    def _case(self, code):
        return next(c for c in FAULT_CASES if c.expected_diagnostic_code == code)

    def _assess(self, case, dry_output):
        return assess_case(
            case=case,
            lint_stdout="",
            dry_output=dry_output,
            run_output="",
            run_exit_code=None,
            workflow_before_sha256="abc",
            workflow_after_sha256="abc",
        )

    def test_lowercase_code_in_rule_name_is_not_a_detection(self):
        result = self._assess(
            self._case("disk"),
            "planning rule spill_to_disk",
        )
        self.assertFalse(result["static_detected"])

    def test_lowercase_engine_output_still_matches(self):
        result = self._assess(
            self._case("OOM"),
            "warning: oom risk for rule high_memory_step",
        )
        self.assertTrue(result["static_detected"])

    def test_hyphenated_code_matches_case_insensitively(self):
        result = self._assess(
            self._case("TYPE-002"),
            "note: type-002 sort-order conflict",
        )
        self.assertTrue(result["static_detected"])

    def test_code_inside_a_longer_word_is_not_a_detection(self):
        # "timeouts" contains the bare substring "timeout" but is not the
        # code token; a boundary-checked match must refuse it.
        result = self._assess(
            self._case("timeout"),
            "several timeouts were logged during warmup",
        )
        self.assertFalse(result["static_detected"])


class StubToolTests(unittest.TestCase):
    """The executable-repair arm depends on stub binaries that create every
    file a real tool would write (the engine validates declared outputs
    post-rule) and on simtool parameterizing each INFRA fault by a declared
    [config] key so a resource-change repair genuinely flips it to success."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="oxo_stub_test_")
        self.workdir = Path(self.tmp.name)
        self.stub_dir = build_stub_dir(self.workdir)
        self.addCleanup(self.tmp.cleanup)

    def _run(self, script, *args):
        return subprocess.run(
            [str(self.stub_dir / script), *args],
            capture_output=True,
            text=True,
            cwd=self.workdir,
        )

    def test_stub_dir_is_on_disk_and_executable(self):
        for name in ("gatk", "bcftools", "samtools", "tabix", "simtool", "bwa", "vep", "featureCounts"):
            path = self.stub_dir / name
            self.assertTrue(path.is_file(), name)
            self.assertTrue(os.access(path, os.X_OK), name)

    def test_gatk_dash_capital_o_creates_output(self):
        res = self._run("gatk", "HaplotypeCaller", "-I", "align.bam", "-O", "raw.vcf")
        self.assertEqual(res.returncode, 0)
        self.assertTrue((self.workdir / "raw.vcf").is_file())

    def test_bcftools_format_selector_is_not_a_file(self):
        res = self._run("bcftools", "view", "-O", "z", "-o", "u.vcf.gz", "u.vcf")
        self.assertEqual(res.returncode, 0)
        self.assertTrue((self.workdir / "u.vcf.gz").is_file())
        self.assertFalse((self.workdir / "z").exists())

    def test_samtools_index_creates_companion_bai(self):
        (self.workdir / "in.bam").touch()
        res = self._run("samtools", "index", "in.bam")
        self.assertEqual(res.returncode, 0)
        self.assertTrue((self.workdir / "in.bam.bai").is_file())

    def test_samtools_dash_o_creates_output(self):
        res = self._run("samtools", "sort", "-o", "sorted.bam", "in.bam")
        self.assertEqual(res.returncode, 0)
        self.assertTrue((self.workdir / "sorted.bam").is_file())

    def test_tabix_creates_tbi(self):
        res = self._run("tabix", "-p", "vcf", "u.vcf.gz")
        self.assertEqual(res.returncode, 0)
        self.assertTrue((self.workdir / "u.vcf.gz.tbi").is_file())

    def test_simtool_oom_fails_below_requirement_without_output(self):
        res = self._run(
            "simtool", "memhog", "--need-mb", "8192", "--have-mb", "2048", "-o", "out.txt"
        )
        self.assertEqual(res.returncode, 137)
        self.assertIn("OOM", res.stderr)
        self.assertFalse((self.workdir / "out.txt").is_file())

    def test_simtool_oom_succeeds_when_repair_raises_allocation(self):
        res = self._run(
            "simtool", "memhog", "--need-mb", "8192", "--have-mb", "8192", "-o", "out.txt"
        )
        self.assertEqual(res.returncode, 0)
        self.assertTrue((self.workdir / "out.txt").is_file())

    def test_simtool_timeout_and_enospc_exit_codes(self):
        res = self._run(
            "simtool", "longrun", "--need-min", "30", "--have-min", "10", "-o", "s.txt"
        )
        self.assertEqual(res.returncode, 152)
        self.assertIn("timeout", res.stderr)
        res = self._run(
            "simtool", "bigscratch", "--need-gb", "10", "--free-gb", "1", "-o", "d.dat"
        )
        self.assertEqual(res.returncode, 1)
        self.assertIn("disk", res.stderr)

    def test_seeded_command_strings_do_not_carry_code_tokens(self):
        # Stage-1 static detection must be engine-grounded, not a coincidence
        # of the fault-seed text: the simtool subcommand names deliberately
        # avoid the diagnostic code tokens ("OOM"/"timeout"/"disk"). Checked
        # with the same word-boundary matcher the harness's static stage
        # uses — a token buried in an identifier ("fault_test_timeout") is
        # not a match.
        for case in FAULT_CASES:
            if case.category != "infrastructure":
                continue
            self.assertFalse(_code_in_text("OOM", case.toml_template), case.fault_id)
            self.assertFalse(_code_in_text("timeout", case.toml_template), case.fault_id)
            self.assertFalse(_code_in_text("disk", case.toml_template), case.fault_id)

    def test_every_case_has_a_reference_repair(self):
        for case in FAULT_CASES:
            self.assertIsNotNone(case.repaired_toml, case.fault_id)
            self.assertTrue(case.executability_note, case.fault_id)


class RepairStageTests(unittest.TestCase):
    """Stages 4-6 classification: narration/checkpoint evidence, exact WHY
    per failed stage, and honest dry-mode degradation."""

    def setUp(self):
        self.case = FAULT_CASES[0]

    def _assess(self, **kwargs):
        defaults = dict(
            case=self.case,
            run_output="",
            run_exit_code=1,
            checkpoint=None,
            rerun_exit_code=1,
            rerun_output="",
            provider_configured=True,
            workflow_sha_before="a",
            workflow_sha_after="a",
            workflow_after_text="[workflow]\nname = 'x'\n",
        )
        defaults.update(kwargs)
        return assess_repair_stages(**defaults)

    def test_dry_mode_reports_provider_not_configured(self):
        result = self._assess(provider_configured=False, rerun_exit_code=137)
        self.assertFalse(result["repair_applied"])
        self.assertIn("provider not configured", result["repair_stage_why"])
        self.assertTrue(result["rerun_attempted"])
        self.assertFalse(result["rerun_succeeded"])

    def test_narration_prefix_is_primary_applied_evidence(self):
        result = self._assess(
            run_output="  REPAIR_APPLIED patch written\n  REPAIR_VERIFIED patch parses\n",
            rerun_exit_code=0,
        )
        self.assertTrue(result["repair_applied"])
        self.assertIn("narration:REPAIR_APPLIED", result["applied_evidence"])
        self.assertTrue(result["repair_verified"])
        self.assertTrue(result["rerun_succeeded"])
        self.assertIsNone(result["repair_stage_why"])
        self.assertEqual(result["narration_repair_events"], ["REPAIR_APPLIED", "REPAIR_VERIFIED"])

    def test_checkpoint_repair_record_is_secondary_evidence(self):
        result = self._assess(
            checkpoint={"repair_record": {"applied": True, "verified": True}},
            rerun_exit_code=0,
        )
        self.assertTrue(result["repair_applied"])
        self.assertIn("checkpoint:repair_record.applied", result["applied_evidence"])
        self.assertTrue(result["repair_verified"])

    def test_engine_self_rerun_success_contract(self):
        # The #1164 engine self-reruns after a verified patch and exits 0:
        # fault manifestation, verification, and rerun success must all
        # classify from the engine narration alone.
        result = self._assess(
            run_output=(
                "  REPAIR_APPLIED session_id=20261118-000000-recover-ab12\n"
                "  REPAIR_RERUN_SUCCEEDED verification rerun completed\n"
            ),
            run_exit_code=0,
            checkpoint={"repair_record": {
                "applied": True,
                "session_id": "20261118-000000-recover-ab12",
                "rerun_attempted": True,
                "rerun_succeeded": True,
                "detail": "verification rerun completed",
            }},
            rerun_exit_code=None,
        )
        self.assertTrue(result["repair_applied"])
        self.assertTrue(result["fault_manifested"])
        self.assertTrue(result["repair_verified"])
        self.assertIn("narration:REPAIR_RERUN_SUCCEEDED", result["verified_evidence"])
        self.assertIn(
            "checkpoint:repair_record.rerun_succeeded", result["verified_evidence"]
        )
        self.assertTrue(result["rerun_attempted"])
        self.assertTrue(result["rerun_succeeded"])
        self.assertIsNone(result["repair_stage_why"])

    def test_engine_rerun_failure_reports_the_narrated_why(self):
        result = self._assess(
            run_output=(
                "  REPAIR_APPLIED session_id=x\n"
                "  REPAIR_RERUN_FAILED verification rerun exited 2\n"
            ),
            run_exit_code=2,
            rerun_exit_code=None,
        )
        self.assertTrue(result["repair_applied"])
        self.assertFalse(result["repair_verified"])
        self.assertFalse(result["rerun_succeeded"])
        self.assertIn("REPAIR_RERUN_FAILED", result["repair_stage_why"])

    def test_self_rerun_engine_disables_the_toml_parses_fallback(self):
        # Under the self-rerunning engine (proven by repair_record with
        # rerun_attempted), parses-valid says nothing about the rerun
        # outcome — the legacy fallback must stay off.
        result = self._assess(
            run_output="  REPAIR_APPLIED session_id=x\n",
            run_exit_code=0,
            checkpoint={"repair_record": {
                "applied": True,
                "rerun_attempted": True,
                "rerun_succeeded": False,
            }},
            workflow_sha_after="b",
            workflow_after_text="[workflow]\nname = 'x'\n",
            rerun_exit_code=None,
        )
        self.assertTrue(result["repair_applied"])
        self.assertFalse(result["repair_verified"])
        self.assertNotIn(
            "fallback:patched-toml-parses", result["verified_evidence"]
        )

    def test_legacy_fix_applied_narration_falls_back(self):
        result = self._assess(
            run_output="  ✓ Fix applied to workflow_ai.oxoflow\n",
            rerun_exit_code=0,
        )
        self.assertTrue(result["repair_applied"])
        self.assertEqual(result["applied_evidence"], ["narration:fix-applied-legacy"])

    def test_validation_rejected_patch_why_is_captured(self):
        result = self._assess(
            run_output=(
                "  AI fix not auto-applied: generated TOML failed validation "
                "(missing field `workflow`). Review the suggestion manually."
            ),
            rerun_exit_code=1,
        )
        self.assertFalse(result["repair_applied"])
        self.assertIn("generated TOML failed validation", result["repair_stage_why"])

    def test_advisory_case_has_no_failure_to_repair(self):
        case = next(c for c in FAULT_CASES if c.runtime_class == "advisory_warning")
        result = assess_repair_stages(
            case=case,
            run_output="Done: 1 succeeded, 0 failed",
            run_exit_code=0,
            checkpoint=None,
            rerun_exit_code=None,
            rerun_output="",
            provider_configured=True,
            workflow_sha_before="a",
            workflow_sha_after="a",
            workflow_after_text="[workflow]\nname = 'x'\n",
        )
        self.assertFalse(result["fault_manifested"])
        self.assertFalse(result["rerun_attempted"])
        self.assertIn("advisory", result["repair_stage_why"])

    def test_patched_toml_parsing_verifies_as_fallback(self):
        valid = "[workflow]\nname = \"patched\"\n"
        result = self._assess(
            run_output="  ✓ Fix applied to workflow_ai.oxoflow\n",
            workflow_sha_before="a",
            workflow_sha_after="b",
            workflow_after_text=valid,
            rerun_exit_code=0,
        )
        self.assertTrue(result["repair_verified"])
        self.assertEqual(result["verified_evidence"], ["fallback:patched-toml-parses"])

    def test_unparseable_patch_is_not_verified(self):
        result = self._assess(
            run_output="  ✓ Fix applied to workflow_ai.oxoflow\n",
            workflow_sha_before="a",
            workflow_sha_after="b",
            workflow_after_text="not [ toml",
            rerun_exit_code=1,
        )
        self.assertTrue(result["repair_applied"])
        self.assertFalse(result["repair_verified"])
        self.assertIn("not verified", result["repair_stage_why"])

    def test_rerun_failure_why_names_exit_code(self):
        result = self._assess(
            run_output="  REPAIR_APPLIED patch written\n  REPAIR_VERIFIED ok\n",
            rerun_exit_code=137,
            rerun_output="Error: workflow execution failed",
        )
        self.assertTrue(result["repair_applied"])
        self.assertIn("exit code 137", result["repair_stage_why"])


class ProviderEnvTests(unittest.TestCase):
    def test_unset_provider_is_dry(self):
        env = {"PATH": "/usr/bin"}
        self.assertIsNone(provider_from_env(env))

    def test_disabled_provider_is_dry(self):
        self.assertIsNone(provider_from_env({"OXO_FLOW_AI_PROVIDER": "disabled"}))

    def test_live_provider_records_names_only(self):
        env = {
            "OXO_FLOW_AI_PROVIDER": "openai",
            "OPENAI_API_KEY": "sk-secret-do-not-leak",
            "OPENAI_BASE_URL": "https://example.invalid/v1",
        }
        provider = provider_from_env(env)
        self.assertEqual(provider["provider"], "openai")
        self.assertIn("OPENAI_API_KEY", provider["provider_env_vars_present"])
        # Secret values must never leak into the record (the provider name
        # itself is recordable; the key and endpoint are not).
        serialized = json.dumps(provider)
        self.assertNotIn("sk-secret-do-not-leak", serialized)
        self.assertNotIn("https://example.invalid/v1", serialized)


class RepairRecordLookupTests(unittest.TestCase):
    def test_top_level_record(self):
        self.assertEqual(find_repair_record({"repair_record": {"applied": True}}), {"applied": True})

    def test_nested_record(self):
        checkpoint = {"run": {"repair_record": {"applied": True}}}
        self.assertEqual(find_repair_record(checkpoint), {"applied": True})

    def test_absent_record(self):
        self.assertIsNone(find_repair_record({"completed_rules": []}))
        self.assertIsNone(find_repair_record(None))


class WorkflowWithAiTests(unittest.TestCase):
    def test_ai_block_is_appended_not_injected(self):
        case = FAULT_CASES[0]
        text = workflow_with_ai(case)
        self.assertTrue(text.startswith(case.toml_template.rstrip("\n")))
        self.assertIn('auto_fix = "always"', text)
        self.assertIn("[ai]", text)
        # The seeded template itself must stay clean for deterministic
        # stage 1-3 measurement.
        self.assertNotIn("[ai]", case.toml_template)
        self.assertEqual(AI_BLOCK.count("enabled"), 1)


FAKE_CLI = r"""#!/bin/sh
# Fake oxo-flow for harness plumbing tests (no engine, no provider).
cmd="$1"
case "$cmd" in
  lint)
    echo '{"diagnostics":[{"code":"TYPE-002","rule":"call_variants","suggestion":"insert a coordinate-sort bridge rule"}]}'
    exit 0;;
  dry-run)
    echo "planned 2 rules"
    exit 0;;
  run)
    shift
    wf="$1"
    if [ "$(basename "$wf")" = "workflow_ai.oxoflow" ] && [ ! -f .patched ]; then
        # First repair-arm run: the rule fails. Recovery only engages under
        # a live provider, mirroring the real engine's behavior.
        echo "  ✗ rule 'call_variants' failed"
        if [ "$OXO_FLOW_AI_PROVIDER" != "" ] && [ "$OXO_FLOW_AI_PROVIDER" != "disabled" ]; then
            touch .patched
            echo "  REPAIR_APPLIED patch written to $wf"
            echo "  REPAIR_VERIFIED patch parses as workflow TOML"
            mkdir -p .oxo-flow
            echo '{"repair_record":{"applied":true,"verified":true}}' > .oxo-flow/checkpoint.json
            printf '\n# patched by fake engine\n' >> "$wf"
        else
            echo "  AI recovery skipped: provider not configured"
        fi
        exit 1
    fi
    exit 0;;
  *)
    echo "unknown command: $cmd" >&2
    exit 2;;
esac
"""


class HarnessPlumbingTests(unittest.TestCase):
    """End-to-end run_test_case against a fake CLI: proves the repair arm's
    seeding, narration/checkpoint parsing, rerun, executability check, and
    log persistence work without a real engine."""

    def _run(self, live: bool):
        tmp = tempfile.TemporaryDirectory(prefix="oxo_plumb_test_")
        self.addCleanup(tmp.cleanup)
        base = Path(tmp.name)
        fake = base / "oxo-flow"
        fake.write_text(FAKE_CLI)
        fake.chmod(0o755)
        logs = base / "logs"

        if live:
            os.environ["OXO_FLOW_AI_PROVIDER"] = "openai"
            try:
                provider = provider_from_env()
                result = run_test_case(
                    str(fake), FAULT_CASES[0], provider=provider, logs_dir=logs
                )
            finally:
                os.environ.pop("OXO_FLOW_AI_PROVIDER", None)
        else:
            result = run_test_case(str(fake), FAULT_CASES[0], provider=None, logs_dir=logs)
        return result, logs

    def test_live_fake_cli_measures_repaired_rerun(self):
        result, logs = self._run(live=True)
        self.assertEqual(result["measurement_mode"], "live")
        self.assertEqual(result["repair_status"], "measured")
        # Stage 1-3 from the fake lint output.
        self.assertTrue(result["static_detected"])
        self.assertTrue(result["diagnostic_attributed"])
        self.assertTrue(result["repair_proposed"])
        # Stage 4-6 from narration + checkpoint + rerun exit code.
        self.assertTrue(result["repair_applied"])
        self.assertTrue(result["repair_verified"])
        self.assertTrue(result["rerun_attempted"])
        self.assertTrue(result["workflow_success_on_rerun"])
        self.assertTrue(result["executability_verified"])
        self.assertTrue(result["engine_patched_workflow"])
        self.assertEqual(result["case_status"], "repaired_and_rerun")
        self.assertIsNone(result["repair_stage_why"])
        self.assertIn(
            "narration:REPAIR_APPLIED", result["applied_evidence"]
        )
        self.assertIn(
            "checkpoint:repair_record.applied", result["applied_evidence"]
        )
        # Raw logs persisted and traceable from the record.
        self.assertIn("fault_run.txt", result["raw_logs"])
        self.assertTrue((logs / FAULT_CASES[0].fault_id / "fault_run.txt").is_file())
        summary = summarize_results([result])
        self.assertEqual(summary["workflow_success_on_rerun"], 1)
        self.assertEqual(summary["measurement_mode"], "live")

    def test_dry_fake_cli_reports_provider_not_configured(self):
        result, _logs = self._run(live=False)
        self.assertEqual(result["measurement_mode"], "dry")
        self.assertFalse(result["repair_applied"])
        self.assertIn("provider not configured", result["repair_stage_why"])
        # The fake CLI only narrates REPAIR_APPLIED under a live provider;
        # dry mode must not fabricate the evidence.
        self.assertEqual(result["narration_repair_events"], [])
        self.assertTrue(result["rerun_attempted"])
        self.assertFalse(result["workflow_success_on_rerun"])


if __name__ == "__main__":
    unittest.main()
