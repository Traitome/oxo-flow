"""Hermetic tests for the execution-equivalence runner (#1300).

Covers the two load-bearing pieces of scripts/import_execution_equivalence_run.py
without any network or real runtime:

  * EXPECT-table parsing -- supported fixtures come from the shared truth table.
  * skip-vs-drift verdict logic -- a fixture whose runtime/backend is absent is
    SKIPPED, never MATCHED; only two genuinely-executed arms can MATCH or DRIFT.

No network, no container, no nextflow/snakemake: every dependency is faked.
"""
import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / "scripts" / "import_execution_equivalence_run.py"


def load_runner():
    spec = importlib.util.spec_from_file_location("exec_equiv", str(RUNNER))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


runner = load_runner()


class SupportedFixtureParsingTests(unittest.TestCase):
    def test_enumerates_only_supported_status(self):
        fake = mock.Mock()
        fake.EXPECTED = {
            "nextflow/rnaseq.ok.nf": {"status": "supported"},
            "nextflow/nfcore_module.ok.nf": {"status": "supported"},
            "nextflow/foo.bad.nf": {"status": "rejected"},
            "snakemake/expand.ok.smk": {"status": "supported"},
            "snakemake/rnaseq.ok.smk": {"status": "supported"},
        }
        self.assertEqual(runner.supported_fixtures(fake), [
            "nextflow/nfcore_module.ok.nf",
            "nextflow/rnaseq.ok.nf",
            "snakemake/expand.ok.smk",
            "snakemake/rnaseq.ok.smk",
        ])

    def test_supported_fixtures_are_sorted(self):
        fake = mock.Mock()
        fake.EXPECTED = {"b/x.nf": {"status": "supported"},
                         "a/y.smk": {"status": "supported"}}
        self.assertEqual(runner.supported_fixtures(fake), ["a/y.smk", "b/x.nf"])


class SourceRuntimeTests(unittest.TestCase):
    def test_known_extensions(self):
        self.assertEqual(runner.source_runtime("nextflow/x.nf")[0], "nextflow")
        self.assertEqual(runner.source_runtime("snakemake/x.smk")[0], "snakemake")

    def test_unknown_extension_has_no_runner(self):
        binary, template = runner.source_runtime("weird/x.txt")
        self.assertIsNone(binary)
        self.assertIsNone(template)


class RequiredBackendTests(unittest.TestCase):
    def test_union_over_rules(self):
        doc = {"rules": [
            {"environment": {"docker": "img"}},
            {"environment": {"conda": "env.yaml"}},
            {"environment": {"modules": ["salmon"]}},
            {"name": "noenv"},
        ]}
        self.assertEqual(runner.required_backends(doc), {"docker", "conda", "modules"})


class MissingReasonTests(unittest.TestCase):
    def test_all_present_returns_none(self):
        self.assertIsNone(runner.missing_reason("nextflow", {"docker"}, lambda n: True))

    def test_missing_runtime_is_reported(self):
        reason = runner.missing_reason("nextflow", set(), lambda n: False)
        self.assertIn("no execution environment", reason)
        self.assertIn("nextflow", reason)

    def test_modules_backend_always_reported_when_declared(self):
        reason = runner.missing_reason(None, {"modules"}, lambda n: True)
        self.assertIn("modules", reason)


class ClassifyVerdictTests(unittest.TestCase):
    def _arm(self, ran, artifacts=None, reason=""):
        return {"ran": ran, "artifacts": artifacts or {}, "reason": reason}

    def test_absent_runtime_is_skipped_never_matched(self):
        # The load-bearing honesty rule: a non-running arm cannot MATCH, even
        # when the would-be artifacts are equal.
        same = {"out/a.txt": "abc"}
        verdict, reason = runner.classify({
            "source": self._arm(False, reason="source arm: nextflow missing"),
            "imported": self._arm(True, same),
        })
        self.assertEqual(verdict, "SKIPPED")
        self.assertIn("nextflow", reason)

    def test_both_arms_run_and_agree_is_matched(self):
        same = {"out/a.txt": "abc"}
        verdict, _ = runner.classify({
            "source": self._arm(True, same),
            "imported": self._arm(True, same),
        })
        self.assertEqual(verdict, "MATCHED")

    def test_both_arms_run_and_differ_is_drifted(self):
        verdict, detail = runner.classify({
            "source": self._arm(True, {"out/a.txt": "abc"}),
            "imported": self._arm(True, {"out/a.txt": "xyz"}),
        })
        self.assertEqual(verdict, "DRIFTED")
        self.assertIn("checksum-differs", detail)

    def test_missing_file_in_one_arm_is_drifted(self):
        verdict, detail = runner.classify({
            "source": self._arm(True, {"out/a.txt": "abc", "out/b.txt": "d"}),
            "imported": self._arm(True, {"out/a.txt": "abc"}),
        })
        self.assertEqual(verdict, "DRIFTED")
        self.assertIn("only-source", detail)

    def test_imported_arm_not_run_is_skipped(self):
        verdict, _ = runner.classify({
            "source": self._arm(True, {"out/a.txt": "abc"}),
            "imported": self._arm(False, reason="imported arm: conda missing"),
        })
        self.assertEqual(verdict, "SKIPPED")


class MeasureEndToEndStubTests(unittest.TestCase):
    """Drive measure()/collect_artifacts end-to-end with stub arms (no real
    toolchain). This is the falsifiable regression for the P1 finding: the two
    arms must be comparable, so two genuinely-running arms that emit an
    identical output set report MATCHED -- not DRIFTED from a one-sided seed."""

    def _write_stub(self, path, out_content):
        # argv[1] == 'import' -> write a minimal (rule-less) TOML document;
        # otherwise (a 'run' invocation) -> write out.txt with out_content.
        path.write_text(
            "#!/usr/bin/env python3\n"
            "import sys\n"
            "a = sys.argv[1:]\n"
            "if a and a[0] == 'import':\n"
            "    open(a[a.index('-o') + 1], 'w').write('# imported\\n')\n"
            "else:\n"
            "    open('out.txt', 'w').write(%r)\n" % out_content
        )
        path.chmod(0o755)

    def _measure(self, src_content, imp_content):
        with tempfile.TemporaryDirectory() as d:
            tmp = Path(d)
            fx = tmp / "ROOT" / "tests" / "fixtures" / "import" / "nextflow"
            fx.mkdir(parents=True)
            (fx / "x.nf").write_text("process STUB {}\n")
            bindir = tmp / "bin"
            bindir.mkdir()
            src_stub = bindir / "nextflow"
            self._write_stub(src_stub, src_content)
            oxo_stub = bindir / "oxo-flow"
            self._write_stub(oxo_stub, imp_content)
            expect = mock.Mock()
            expect.ROOT = str(tmp / "ROOT")
            with mock.patch.dict(
                runner.SOURCE_RUNNERS,
                {".nf": ("nextflow", [str(src_stub), "run", "{src}"])},
            ):
                return runner.measure("nextflow/x.nf", expect, str(oxo_stub),
                                      str(tmp / "work"), lambda n: True, 30)

    def test_identical_stub_output_is_matched(self):
        # Both arms exit 0 and emit the SAME out.txt (their seeds differ in name
        # but are excluded), so the verdict must be MATCHED.
        verdict, _ = self._measure(src_content="hello\n", imp_content="hello\n")
        self.assertEqual(verdict, "MATCHED")

    def test_differing_stub_output_is_drifted(self):
        verdict, detail = self._measure(src_content="hello\n", imp_content="bye\n")
        self.assertEqual(verdict, "DRIFTED")
        self.assertIn("checksum-differs", detail)

    def test_seed_files_are_not_counted_as_artifacts(self):
        # collect_artifacts sees the natural seeds (x.nf / W.oxoflow); measure()
        # must drop both, so identical OUT artifacts still MATCH.
        verdict, _ = self._measure(src_content="same\n", imp_content="same\n")
        self.assertEqual(verdict, "MATCHED")


class StrictExitCodeTests(unittest.TestCase):
    """The P2 finding: main() must return non-zero on drift so the wrapper's
    --strict branch can actually fire, and 0 when nothing executed."""

    def test_main_returns_nonzero_when_drifted(self):
        import contextlib
        import io
        buf = io.StringIO()
        with mock.patch.object(runner, "measure", return_value=("DRIFTED", "x")), \
                contextlib.redirect_stdout(buf):
            rc = runner.main(["prog", "/tmp/ROOT", "/bin/true", "/tmp/t"])
        self.assertEqual(rc, 1)

    def test_main_returns_zero_when_all_skipped(self):
        import contextlib
        import io
        buf = io.StringIO()
        with mock.patch.object(runner, "measure",
                               return_value=("SKIPPED", "no runtime")), \
                contextlib.redirect_stdout(buf):
            rc = runner.main(["prog", "/tmp/ROOT", "/bin/true", "/tmp/t"])
        self.assertEqual(rc, 0)

    def test_runner_process_exits_zero_when_nothing_executed(self):
        # End-to-end on this host: a stub binary that fails every import leaves
        # all fixtures SKIPPED, so the real runner must exit 0.
        with tempfile.TemporaryDirectory() as d:
            stub = Path(d) / "oxo-flow-stub"
            stub.write_text("#!/bin/sh\nexit 1\n")
            stub.chmod(0o755)
            p = subprocess.run(
                [sys.executable, str(RUNNER), str(ROOT), str(stub), d],
                capture_output=True, text=True)
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)


if __name__ == "__main__":
    unittest.main()
