"""Hermetic tests for the execution-equivalence runner (#1300).

Covers the load-bearing pieces of scripts/import_execution_equivalence_run.py
without any network or real runtime:

  * EXPECT-table parsing -- supported fixtures come from the shared truth table.
  * skip-vs-drift verdict logic -- a fixture whose runtime/backend is absent is
    SKIPPED, never MATCHED; only two genuinely-executed arms can MATCH or DRIFT.
  * #1349 hardening -- empty == empty is NO_ARTIFACTS, never MATCHED; an
    interpreter below the tomllib (3.11) floor refuses to run with exit 2.

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

    def test_entry_execution_source_overrides_the_extension_default(self):
        # #1398: the SOURCE arm must honor the backend/container the fixture
        # declares instead of a hardcoded -with-docker / --use-conda choice.
        entry = {"execution": {
            "source": ["snakemake", "-s", "{src}", "--use-singularity"],
            "source_backend": "singularity",
        }}
        binary, template = runner.source_runtime("snakemake/x.smk", entry)
        self.assertEqual(binary, "snakemake")
        self.assertEqual(template, [
            "snakemake", "-s", "{src}", "--use-singularity"])

    def test_engine_binary_env_override_is_honored(self):
        entry = {"execution": {"source": ["nextflow", "run", "{src}"]}}
        with mock.patch.dict("os.environ",
                             {"OXO_EXEC_NEXTFLOW_BIN": "/opt/nf/nextflow"}):
            binary, template = runner.source_runtime("nextflow/x.nf", entry)
        self.assertEqual(binary, "/opt/nf/nextflow")
        self.assertEqual(template, ["/opt/nf/nextflow", "run", "{src}"])


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

    def test_singularity_backend_probe_accepts_apptainer(self):
        # #1398: a Snakemake source arm run with --use-singularity must be
        # probed as singularity, not satisfied by docker alone.
        have = lambda name: name in ("snakemake", "apptainer")
        self.assertIsNone(
            runner.missing_reason("snakemake", {"singularity"}, have))
        self.assertIn(
            "singularity",
            runner.missing_reason("snakemake", {"singularity"},
                                  lambda n: n in ("snakemake", "docker")))


class ContainerValidationTests(unittest.TestCase):
    def test_docker_scheme_prefix_is_normalized(self):
        doc = {"rules": [{"environment": {"docker": "img:1"}}]}
        self.assertIsNone(
            runner.container_mismatch(doc, {"container": "docker://img:1"}))

    def test_singularity_environment_also_counts(self):
        doc = {"rules": [{"environment": {"singularity": "sif:1"}}]}
        self.assertIsNone(
            runner.container_mismatch(doc, {"container": "docker://sif:1"}))

    def test_mismatch_names_both_specs(self):
        doc = {"rules": [{"environment": {"docker": "img:2"}}]}
        reason = runner.container_mismatch(doc, {"container": "docker://img:1"})
        self.assertIn("container mismatch", reason)
        self.assertIn("img:1", reason)
        self.assertIn("img:2", reason)

    def test_missing_imported_container_is_a_mismatch(self):
        doc = {"rules": [{"name": "system-only"}]}
        reason = runner.container_mismatch(doc, {"container": "docker://img:1"})
        self.assertIn("no container", reason)

    def test_no_declared_container_is_not_checked(self):
        doc = {"rules": [{"environment": {"docker": "img:2"}}]}
        self.assertIsNone(runner.container_mismatch(doc, {}))


class DeclaredOutputTests(unittest.TestCase):
    def test_only_literal_paths_are_collected(self):
        doc = {"rules": [
            {"output": ["results/a.txt", "results/{sample}.bam"]},
            {"output": ["glob/*.txt"]},
            {"output": ["questions?.txt"]},
        ]}
        self.assertEqual(
            runner.declared_literal_outputs(doc), ["results/a.txt"])

    def test_missing_declared_output_is_reported(self):
        self.assertEqual(
            runner.missing_declared_outputs({"other.txt": "x"},
                                            ["declared.txt"]),
            ["declared.txt"])
        self.assertEqual(
            runner.missing_declared_outputs({"declared.txt": "x"},
                                            ["declared.txt"]), [])


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

    def test_both_arms_run_but_produce_nothing_is_no_artifacts_never_matched(self):
        # The #1349 false-PASS vector: both arms exited 0 yet wrote nothing,
        # so the empty dicts compare equal. That must be an explicit honest
        # verdict, never MATCHED.
        verdict, reason = runner.classify({
            "source": self._arm(True, {}),
            "imported": self._arm(True, {}),
        })
        self.assertEqual(verdict, "NO_ARTIFACTS")
        self.assertNotEqual(verdict, "MATCHED")
        self.assertIn("no artifacts", reason)

    def test_one_arm_empty_other_has_artifacts_is_drifted(self):
        # The guard must not over-trigger: one genuinely-produced artifact
        # against an empty set is still a real comparison -> DRIFTED.
        verdict, detail = runner.classify({
            "source": self._arm(True, {}),
            "imported": self._arm(True, {"out/a.txt": "abc"}),
        })
        self.assertEqual(verdict, "DRIFTED")
        self.assertIn("only-imported", detail)


class MeasureEndToEndStubTests(unittest.TestCase):
    """Drive measure()/collect_artifacts end-to-end with stub arms (no real
    toolchain). This is the falsifiable regression for the P1 finding: the two
    arms must be comparable, so two genuinely-running arms that emit an
    identical output set report MATCHED -- not DRIFTED from a one-sided seed."""

    def _write_stub(self, path, out_content, import_body="# imported\n"):
        # argv[1] == 'import' -> write the supplied TOML document;
        # otherwise (a 'run' invocation) -> write out.txt with out_content.
        # out_content=None writes nothing on run: a runtime that exits 0
        # while producing zero artifacts (#1349).
        body = (
            "#!/usr/bin/env python3\n"
            "import sys\n"
            "a = sys.argv[1:]\n"
            "if a and a[0] == 'import':\n"
            "    open(a[a.index('-o') + 1], 'w').write(%r)\n" % import_body
        )
        if out_content is not None:
            body += "else:\n    open('out.txt', 'w').write(%r)\n" % out_content
        path.write_text(body)
        path.chmod(0o755)

    def _measure(self, src_content, imp_content, record_sink=None,
                 import_body=None, expected=None):
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
            self._write_stub(
                oxo_stub, imp_content,
                "# imported\n" if import_body is None else import_body)
            expect = mock.Mock()
            expect.ROOT = str(tmp / "ROOT")
            expect.EXPECTED = expected or {}
            with mock.patch.dict(
                runner.SOURCE_RUNNERS,
                {".nf": ("nextflow", [str(src_stub), "run", "{src}"])},
            ):
                return runner.measure("nextflow/x.nf", expect, str(oxo_stub),
                                      str(tmp / "work"), lambda n: True, 30,
                                      record_sink)

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

    def test_zero_output_arms_are_no_artifacts_not_matched(self):
        # End-to-end regression of the #1349 false-PASS vector: both stub arms
        # exit 0 but write nothing, so the artifact sets are equal AND empty.
        # That must be an explicit NO_ARTIFACTS verdict, never MATCHED.
        verdict, reason = self._measure(src_content=None, imp_content=None)
        self.assertEqual(verdict, "NO_ARTIFACTS")
        self.assertIn("no artifacts", reason)

    def test_records_capture_both_arms_and_artifact_hashes(self):
        # #1398: raw evidence is a deliverable -- every executed fixture must
        # leave its exact per-arm command, exit state and SHA-256 map.
        records = []
        verdict, _ = self._measure("same\n", "same\n", records)
        self.assertEqual(verdict, "MATCHED")
        self.assertEqual(len(records), 1)
        rec = records[0]
        self.assertEqual(rec["fixture"], "nextflow/x.nf")
        self.assertEqual(rec["verdict"], "MATCHED")
        self.assertTrue(rec["source"]["ran"])
        self.assertTrue(rec["imported"]["ran"])
        self.assertIn("run", rec["source"]["command"])
        self.assertEqual(rec["imported"]["command"][1:3], ["run", "W.oxoflow"])
        self.assertIn("out.txt", rec["source"]["artifacts"])
        self.assertIn("out.txt", rec["imported"]["artifacts"])
        self.assertEqual(rec["source"]["artifacts"]["out.txt"],
                         rec["imported"]["artifacts"]["out.txt"])

    def test_incidental_file_agreement_without_declared_output_is_skipped(self):
        # #1398 review: an incidental file that both arms happen to write must
        # not be able to carry a MATCHED when the imported workflow declares a
        # different output. Both stub arms write out.txt; declared.txt is
        # missing from both -> SKIPPED, never MATCHED.
        import_body = ('[[rules]]\nname = "r"\n'
                       'output = ["declared.txt"]\nshell = "true"\n')
        verdict, reason = self._measure(
            "incidental\n", "incidental\n", import_body=import_body)
        self.assertEqual(verdict, "SKIPPED")
        self.assertIn("declared output", reason)

    def test_declared_output_present_keeps_match(self):
        import_body = ('[[rules]]\nname = "r"\n'
                       'output = ["out.txt"]\nshell = "true"\n')
        verdict, _ = self._measure(
            "same\n", "same\n", import_body=import_body)
        self.assertEqual(verdict, "MATCHED")

    def test_pinned_container_mismatch_is_skipped_before_running(self):
        # #1398 review: the fixture's declared container must be what the
        # imported workflow actually runs (scheme-insensitive).
        expected = {"nextflow/x.nf": {
            "execution": {"container": "docker://wanted:1"}}}
        import_body = ('[[rules]]\nname = "r"\n'
                       'environment = { docker = "other:2" }\n'
                       'shell = "true"\n')
        verdict, reason = self._measure(
            "same\n", "same\n", import_body=import_body, expected=expected)
        self.assertEqual(verdict, "SKIPPED")
        self.assertIn("container mismatch", reason)


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

    def test_main_refuses_below_tomllib_floor(self):
        # #1349 gap 1: an interpreter without tomllib (Python < 3.11) used to
        # die mid-run with a traceback that report-only mode swallowed into a
        # silent-green empty result. It must instead refuse to start with an
        # explicit message and a non-zero exit.
        import contextlib
        import io
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(runner, "tomllib", None), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            rc = runner.main(["prog", "/tmp/ROOT", "/bin/true", "/tmp/t"])
        self.assertEqual(rc, 2)
        self.assertIn("3.11", err.getvalue())
        self.assertIn("tomllib", err.getvalue())
        self.assertNotIn("RESULT", out.getvalue())

    def test_no_artifacts_counts_as_executed_and_exits_zero(self):
        # #1349 gap 2 at the tally level: NO_ARTIFACTS is an honest verdict --
        # counted under executed (both arms ran), never under matched, and it
        # does not flip the exit code (only DRIFTED does).
        import contextlib
        import io
        buf = io.StringIO()
        with mock.patch.object(
                runner, "measure",
                return_value=("NO_ARTIFACTS",
                              "both arms ran but produced no artifacts")), \
                contextlib.redirect_stdout(buf):
            rc = runner.main(["prog", "/tmp/ROOT", "/bin/true", "/tmp/t"])
        self.assertEqual(rc, 0)
        self.assertIn("  no_artifacts: ", buf.getvalue())
        self.assertNotIn('"NO_ARTIFACTS": 0', buf.getvalue())
        self.assertIn('"MATCHED": 0', buf.getvalue())

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
