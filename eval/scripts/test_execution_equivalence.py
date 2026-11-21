"""Hermetic tests for the execution-equivalence harness (#1304).

No network, no container, no Nextflow, no Snakemake: every engine dependency
is stubbed or forced unreachable. Covers the load-bearing honesty rules of
scripts/execution_equivalence_run.py:

  * a fixture is MATCHED only when ALL THREE arms ran to exit 0 with identical
    declared-output content -- a non-running arm is SKIPPED, never MATCHED;
  * main() returns non-zero only when a fixture DRIFTED, and 0 when everything
    was SKIPPED (a host without the toolchain must not read as a failure);
  * the ENTRY/ARMS tables agree with the arm names the runner drives (the
    KeyError that broke the first end-to-end run);
  * the fixture set reuses the import corpus truth table without drift.
"""
import contextlib
import importlib.util
import io
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / "scripts" / "execution_equivalence_run.py"
EXPECT = ROOT / "scripts" / "execution_equivalence_expectations.py"


def _load(path, name):
    spec = importlib.util.spec_from_file_location(name, str(path))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


runner = _load(RUNNER, "execution_equivalence_run")
expect = _load(EXPECT, "execution_equivalence_expectations")


FIXTURE = "rnaseq_chain"


def _arm(ran, outputs=None, why=None, exit_=0):
    return {"arm": "x", "ran": ran, "exit": exit_, "seconds": 1.0,
            "outputs": outputs or {}, "missing": [], "restart_exit": 0,
            "restart_same": True, "notes": [], "command": None, "why": why}


def _full(fixture, tag="0"):
    # outputs covering the COMPLETE declared set: classify() rejects a
    # partial set as an output-set mismatch before comparing hashes.
    return {rel: tag for rel in expect.get(fixture)["declared_outputs"]}


class EntryArmsConsistencyTests(unittest.TestCase):
    def test_entry_covers_every_arm(self):
        # Regression: the runner drives arms by name ("oxo-flow"); a mismatched
        # ENTRY key raised KeyError on the first end-to-end run.
        for arm in expect.ARMS:
            self.assertIn(arm, expect.ENTRY)

    def test_arms_are_exactly_three_engines(self):
        self.assertEqual(sorted(expect.ARMS),
                         ["nextflow", "oxo-flow", "snakemake"])


class FixtureSetTests(unittest.TestCase):
    def test_each_fixture_has_an_engine_entry_and_outputs(self):
        for fx in expect.FIXTURES:
            self.assertTrue(fx["declared_outputs"], fx["name"])
            self.assertTrue(fx["import_reference"], fx["name"])

    def test_fixture_directories_exist(self):
        base = ROOT / "tests" / "fixtures" / "execution-equivalence"
        for fx in expect.FIXTURES:
            for fname in expect.ENTRY.values():
                self.assertTrue((base / fx["name"] / fname).is_file(),
                                "%s/%s missing" % (fx["name"], fname))

    def test_import_references_are_all_supported(self):
        # The fixture selectors must come from the import truth table's
        # supported set; drift here is a hard error, not a warning.
        self.assertEqual(expect.unknown_import_refs(), [])
        self.assertTrue(expect.supported_import_fixtures())


class ClassifyVerdictTests(unittest.TestCase):
    def test_absent_arm_is_skipped_never_matched(self):
        # The load-bearing honesty rule: a non-running arm cannot MATCH, even
        # when the would-be outputs are identical.
        same = _full(FIXTURE)
        recs = {FIXTURE: {
            "oxo-flow": _arm(True, same),
            "nextflow": _arm(True, same),
            "snakemake": _arm(False, why="engine binary not reachable"),
        }}
        verdict, why = runner.classify([FIXTURE], recs)
        self.assertEqual(verdict, "SKIPPED")
        self.assertIn("snakemake", why)

    def test_all_arms_run_and_agree_is_matched(self):
        same = _full(FIXTURE)
        recs = {a: _arm(True, same) for a in expect.ARMS}
        verdict, _ = runner.classify([FIXTURE], {FIXTURE: recs})
        self.assertEqual(verdict, "MATCHED")

    def test_all_arms_run_but_differ_is_drifted(self):
        recs = {FIXTURE: {
            "oxo-flow": _arm(True, _full(FIXTURE, "abc")),
            "nextflow": _arm(True, _full(FIXTURE, "xyz")),
            "snakemake": _arm(True, _full(FIXTURE, "abc")),
        }}
        verdict, why = runner.classify([FIXTURE], recs)
        self.assertEqual(verdict, "DRIFTED")
        self.assertIn(expect.get(FIXTURE)["declared_outputs"][0], why)

    def test_output_set_mismatch_is_drifted(self):
        short = dict(_full(FIXTURE))
        dropped = expect.get(FIXTURE)["declared_outputs"][-1]
        del short[dropped]
        recs = {FIXTURE: {
            "oxo-flow": _arm(True, _full(FIXTURE)),
            "nextflow": _arm(True, _full(FIXTURE)),
            "snakemake": _arm(True, short),
        }}
        verdict, why = runner.classify([FIXTURE], recs)
        self.assertEqual(verdict, "DRIFTED")
        self.assertIn("output set mismatch", why)


class ArmPlanTests(unittest.TestCase):
    def test_absent_engine_binary_yields_no_plan(self):
        env = {"EQ_NEXTFLOW_BIN": "definitely-not-on-path-1304",
               "EQ_SNAKEMAKE_BIN": "also-absent-1304"}
        with mock.patch.dict(os.environ, env, clear=False), \
                mock.patch.object(runner, "which", return_value=None):
            self.assertIsNone(
                runner.arm_plan("nextflow", "rnaseq_chain", "/tmp/d", "/bin/true"))
            self.assertIsNone(
                runner.arm_plan("snakemake", "rnaseq_chain", "/tmp/d", "/bin/true"))

    def test_oxo_flow_arm_always_plans(self):
        plan = runner.arm_plan("oxo-flow", "rnaseq_chain", "/tmp/d", "/bin/oxo")
        self.assertIsNotNone(plan)
        self.assertIn("run", plan[0])


class MainExitCodeTests(unittest.TestCase):
    def _root_with_fixtures(self, d):
        base = Path(d) / "tests" / "fixtures" / "execution-equivalence"
        for fx in expect.fixture_names():
            (base / fx).mkdir(parents=True)
        return d

    def _main_rc(self, root, d, make_rec):
        argv = ["prog", root, "/bin/true", d]
        # run_arm(fixture_dir, fixture, arm, scratch, oxo_bin)
        side = lambda fd, fx, arm, *a, **k: dict(make_rec(fx, arm))
        with mock.patch.object(runner, "run_arm", side_effect=side), \
                mock.patch.object(sys, "argv", argv), \
                contextlib.redirect_stdout(io.StringIO()):
            return runner.main()

    def test_main_returns_nonzero_when_drifted(self):
        with tempfile.TemporaryDirectory() as d:
            root = self._root_with_fixtures(d)
            make = lambda fx, arm: _arm(
                True, _full(fx, "b" if arm == "nextflow" else "a"))
            self.assertEqual(self._main_rc(root, d, make), 1)

    def test_main_returns_zero_when_all_skipped(self):
        with tempfile.TemporaryDirectory() as d:
            root = self._root_with_fixtures(d)
            make = lambda fx, arm: _arm(False, why="unreachable")
            self.assertEqual(self._main_rc(root, d, make), 0)

    def test_main_returns_zero_when_all_matched(self):
        with tempfile.TemporaryDirectory() as d:
            root = self._root_with_fixtures(d)
            make = lambda fx, arm: _arm(True, _full(fx))
            self.assertEqual(self._main_rc(root, d, make), 0)


class RunnerProcessStubTests(unittest.TestCase):
    def test_runner_process_exits_zero_when_nothing_executed(self):
        # End-to-end on this host with all engines forced unreachable and a
        # stub binary that fails every run: every arm SKIPPED, exit 0.
        with tempfile.TemporaryDirectory() as d:
            stub = Path(d) / "oxo-flow-stub"
            stub.write_text("#!/bin/sh\nexit 1\n")
            stub.chmod(0o755)
            env = dict(os.environ)
            env["EQ_NEXTFLOW_BIN"] = "absent-1304"
            env["EQ_SNAKEMAKE_BIN"] = "absent-1304"
            p = subprocess.run(
                [sys.executable, str(RUNNER), str(ROOT), str(stub), d],
                capture_output=True, text=True, env=env)
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn("SKIPPED=3", p.stdout)


if __name__ == "__main__":
    unittest.main()
