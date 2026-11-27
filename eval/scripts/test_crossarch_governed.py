#!/usr/bin/env python3
"""Unit tests for the governed #1397 cross-architecture analysis.

Run by `make eval-tests` (stdlib unittest discovery). The analysis module is
loaded by path because its filename is a script, not an importable module.
"""

import importlib.util
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
ANALYZE_PATH = REPO / "scripts" / "cross-arch-provenance-governed-analyze.py"
CORPUS_PATH = REPO / "scripts" / "cross-arch-provenance-governed-corpus.json"
EVIDENCE = REPO / "docs/superpowers/audits/data/crossarch-1397"
PREREGISTRATION = EVIDENCE / "preregistration.json"
EXPECTED_COMMIT = "56b109c17b95e4c610a874aaf63fc0ae583a57a6"

_spec = importlib.util.spec_from_file_location("crossarch_governed_analyze", ANALYZE_PATH)
analyze = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(analyze)


def class_rows(count, total, differing=0):
    return [
        {
            "total": total,
            "identical": total - differing,
            "eligible": total >= count,
        }
    ]


class ClopperPearsonTest(unittest.TestCase):
    def test_known_values_match_scipy_binomtest(self):
        lo, hi = analyze.clopper_pearson(2, 9)
        self.assertAlmostEqual(lo, 0.0281, places=4)
        self.assertAlmostEqual(hi, 0.6001, places=4)
        lo, hi = analyze.clopper_pearson(1, 9)
        self.assertAlmostEqual(lo, 0.0028, places=4)
        self.assertAlmostEqual(hi, 0.4825, places=4)

    def test_all_identical_lower_bound_is_alpha_half_root(self):
        for n in (8, 12, 20, 24):
            lo, hi = analyze.clopper_pearson(n, n)
            self.assertAlmostEqual(lo, 0.025 ** (1.0 / n), places=10)
            self.assertEqual(hi, 1.0)

    def test_zero_successes_lower_bound_is_zero(self):
        lo, hi = analyze.clopper_pearson(0, 10)
        self.assertEqual(lo, 0.0)
        self.assertGreater(hi, 0.0)
        self.assertLess(hi, 0.4)


class GovernRuleTest(unittest.TestCase):
    def test_all_identical_is_positive(self):
        rows = class_rows(3, 3)
        category, step, interval, lo, hi = analyze.govern(24, 24, rows * 8, 0.80, 3, [])
        self.assertEqual(category, "POSITIVE")
        self.assertEqual(interval, "POSITIVE")
        self.assertAlmostEqual(lo, 0.025 ** (1.0 / 24), places=10)
        self.assertEqual(hi, 1.0)
        self.assertIn("step 3", step)

    def test_eligible_class_difference_forces_negative_even_with_perfect_proportion(self):
        rows = class_rows(3, 3) * 7 + class_rows(3, 3, differing=1)
        # The campaign-level proportion is 23/24; force it to 24/24 to prove
        # the class override does not depend on the interval.
        category, step, interval, lo, hi = analyze.govern(24, 24, rows, 0.80, 3, [])
        self.assertEqual(category, "NEGATIVE")
        self.assertEqual(interval, "POSITIVE")
        self.assertIn("step 2", step)

    def test_ineligible_class_difference_does_not_trigger_precedence(self):
        rows = class_rows(4, 3, differing=1)
        category, step, interval, lo, hi = analyze.govern(23, 24, rows, 0.70, 4, [])
        self.assertEqual(category, "POSITIVE")
        self.assertIn("step 3", step)

    def test_interval_containing_threshold_is_inconclusive(self):
        category, step, interval, lo, hi = analyze.govern(19, 24, class_rows(3, 3), 0.80, 3, [])
        self.assertEqual(category, "INCONCLUSIVE")
        self.assertLess(lo, 0.80)
        self.assertGreaterEqual(hi, 0.80)

    def test_invalid_precedes_everything(self):
        category, step, interval, lo, hi = analyze.govern(
            24, 24, class_rows(3, 3), 0.80, 3, [{"code": "ARM_EVIDENCE_MISSING", "detail": "x"}]
        )
        self.assertEqual(category, "INVALID")
        self.assertIn("step 1", step)
        self.assertIsNone(interval)


def write_arm(root, stem, *, hashes, control_hash, git_sha="a" * 40,
              content_hash="sha256:" + "b" * 64, dirty=False,
              tools=None, platform="linux/amd64", container_arch="x86_64"):
    root = Path(root)
    root.mkdir(parents=True, exist_ok=True)
    tools = tools or {
        "python3": "Python 3.12.16",
        "sort": "sort (GNU coreutils) 9.7",
        "wc": "wc (GNU coreutils) 9.7",
        "gzip": "gzip 1.13",
        "sha256sum": "sha256sum (GNU coreutils) 9.7",
        "libc": "ldd (Debian GLIBC 2.36-9+deb12u10) 2.36",
    }
    (root / "identity.txt").write_text(
        "\n".join(
            [
                "hostname: testhost",
                "host-arch: %s" % ("arm64" if container_arch == "aarch64" else "x86_64"),
                "engine-commit: %s" % git_sha,
                "platform: %s" % platform,
                "binary-sha256: %s" % ("c" * 64),
            ]
        )
        + "\n"
    )
    container = {
        "image-src": "python@sha256:" + "d" * 64,
        "manifest-list-digest": "sha256:" + "d" * 64,
        "staged-tag": "crossarch1397:pin",
        "platform": platform,
        "image-arch": "amd64" if platform.endswith("amd64") else "arm64",
        "image-id": "sha256:" + "e" * 64,
        "container-arch": container_arch,
    }
    container.update(tools)
    (root / "container.txt").write_text(
        "\n".join("%s: %s" % (k, v) for k, v in container.items()) + "\n"
    )
    all_hashes = dict(hashes)
    all_hashes["w/control/os_arch.txt"] = control_hash
    (root / ("%s.checkpoint.json" % stem)).write_text(
        json.dumps(
            {
                "checksums": {"%s" % k.split("/", 1)[1]: "sha256:" + v for k, v in all_hashes.items()},
                "workflow_git_sha": git_sha,
                "workflow_git_dirty": dirty,
                "workflow_content_hash": content_hash,
                "completed_rules": ["prepare"],
                "failed_rules": [],
            }
        )
    )
    (root / ("%s.verify.json" % stem)).write_text(
        json.dumps(
            {
                "verify": {
                    "matched": len(all_hashes),
                    "mismatched": 0,
                    "missing": 0,
                    "entries": [
                        {"file": k.split("/", 1)[1], "status": "matched"}
                        for k in all_hashes
                    ],
                }
            }
        )
    )
    (root / ("%s.artifacts.sha256" % stem)).write_text(
        "\n".join("%s  %s" % (v, k) for k, v in sorted(all_hashes.items())) + "\n"
    )


def write_corpus(root):
    corpus = {
        "format": "oxo-flow.crossarch-corpus/v1",
        "image": {
            "ref": "python@sha256:" + "d" * 64,
            "manifest_list_digest": "sha256:" + "d" * 64,
            "staged_tag": "crossarch1397:pin",
        },
        "workflows": [{"stem": "w", "file": "w.oxoflow"}],
        "classes": {"pure-text": ["w/a.txt", "w/b.txt", "w/c.txt"]},
        "negative_controls": ["w/control/os_arch.txt"],
        "primary_artifact_count": 3,
        "analysis": {"tau_ARCH": 0.2, "n_min": 3, "alpha": 0.05},
    }
    path = Path(root) / "corpus.json"
    path.write_text(json.dumps(corpus))
    return path


class EndToEndSyntheticTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.corpus = write_corpus(self.root)
        self.hashes = {"w/a.txt": "1" * 64, "w/b.txt": "2" * 64, "w/c.txt": "3" * 64}

    def tearDown(self):
        self.tmp.cleanup()

    def run_result(self, *, arm_a_kwargs=None, arm_b_kwargs=None, make_arm_b=True):
        arm_a = self.root / "arm-a"
        arm_b = self.root / "arm-b"
        defaults_a = {
            "hashes": self.hashes,
            "control_hash": "9" * 64,
            "git_sha": "a" * 40,
            "platform": "linux/amd64",
            "container_arch": "x86_64",
        }
        defaults_b = {
            "hashes": self.hashes,
            "control_hash": "8" * 64,
            "git_sha": "a" * 40,
            "platform": "linux/arm64",
            "container_arch": "aarch64",
        }
        kw_a = dict(defaults_a)
        kw_a.update(arm_a_kwargs or {})
        kw_b = dict(defaults_b)
        kw_b.update(arm_b_kwargs or {})
        write_arm(arm_a, "w", **kw_a)
        if make_arm_b:
            write_arm(arm_b, "w", **kw_b)
        corpus, _tau, _n_min, _alpha, _primary, _controls = analyze.load_corpus(self.corpus)
        loaded_a = analyze.load_arm(arm_a, corpus)
        loaded_b = analyze.load_arm(arm_b, corpus)
        return analyze.build_result(corpus, loaded_a, loaded_b, "a" * 40, self.corpus)

    def test_valid_identical_arms_with_differing_control_is_positive(self):
        result = self.run_result()
        self.assertEqual(result["decision"]["category"], "POSITIVE")
        self.assertEqual(result["primary_endpoint"]["identical"], 3)
        self.assertTrue(result["negative_control"][0]["differs"])

    def test_identical_negative_control_is_invalid(self):
        result = self.run_result(arm_b_kwargs={"control_hash": "9" * 64})
        self.assertEqual(result["decision"]["category"], "INVALID")
        codes = {r["code"] for r in result["validity"]["reasons"]}
        self.assertIn("NEGATIVE_CONTROL_IDENTICAL", codes)

    def test_missing_arm_is_invalid_not_stable(self):
        result = self.run_result(make_arm_b=False)
        self.assertEqual(result["decision"]["category"], "INVALID")
        codes = {r["code"] for r in result["validity"]["reasons"]}
        self.assertIn("ARM_EVIDENCE_MISSING", codes)
        self.assertIn("MISSING_ARTIFACT", codes)

    def test_dirty_tree_is_invalid(self):
        result = self.run_result(arm_b_kwargs={"dirty": True})
        self.assertEqual(result["decision"]["category"], "INVALID")
        codes = {r["code"] for r in result["validity"]["reasons"]}
        self.assertIn("WORKFLOW_TREE_DIRTY", codes)

    def test_tool_version_mismatch_is_invalid(self):
        tools = {
            "python3": "Python 3.12.15",
            "sort": "sort (GNU coreutils) 9.7",
            "gzip": "gzip 1.13",
            "sha256sum": "sha256sum (GNU coreutils) 9.7",
            "libc": "ldd (Debian GLIBC 2.36-9+deb12u10) 2.36",
        }
        result = self.run_result(arm_b_kwargs={"tools": tools})
        self.assertEqual(result["decision"]["category"], "INVALID")
        codes = {r["code"] for r in result["validity"]["reasons"]}
        self.assertIn("ENV_LOCK_TOOL_MISMATCH", codes)


class FrozenCorpusManifestTest(unittest.TestCase):
    def test_corpus_shape_matches_the_frozen_amendment(self):
        corpus = json.loads(CORPUS_PATH.read_text())
        primary = [key for keys in corpus["classes"].values() for key in keys]
        self.assertEqual(len(primary), 24)
        self.assertEqual(len(set(primary)), 24)
        self.assertEqual(len(corpus["classes"]), 8)
        for name, keys in corpus["classes"].items():
            self.assertEqual(len(keys), 3, "%s must have three artifacts" % name)
        self.assertEqual(len(corpus["negative_controls"]), 1)
        self.assertEqual(corpus["analysis"]["tau_ARCH"], 0.8)
        self.assertEqual(corpus["analysis"]["n_min"], 3)
        self.assertEqual(corpus["primary_artifact_count"], 24)
        for wf in corpus["workflows"]:
            self.assertTrue((REPO / wf["file"]).is_file(), wf["file"])

    def test_all_identical_lower_bound_clears_tau(self):
        lower = analyze.clopper_pearson(24, 24)[0]
        self.assertAlmostEqual(lower, 0.025 ** (1.0 / 24), places=10)
        self.assertGreater(lower, 0.80)

    def test_manifest_records_freeze_and_final_run_hashes(self):
        manifest = json.loads(PREREGISTRATION.read_text())
        self.assertEqual(
            manifest["freeze_manifest"]["sha256"],
            "0eaca24be6d00f5ab369f1fb6769131bfeed336107128e76b26ccb6c4ff63908",
        )
        self.assertEqual(
            manifest["final_run_manifest"]["sha256"],
            "770ae8f2769f112cd308249ea94882dee2b8c6e7a7e383550d790c229c2a75bf",
        )
        self.assertEqual(manifest["freeze_manifest"]["commit"], "dd5dcd44ee91a2c93bfd24c29f189d8de32e87d5")
        self.assertEqual(
            manifest["final_run_manifest"]["commit"],
            "56b109c17b95e4c610a874aaf63fc0ae583a57a6",
        )
        self.assertIn("post_run_hardening", manifest)


class CommittedEvidenceTest(unittest.TestCase):
    """Regression tests over the committed campaign evidence + manifest."""

    def run_cli(self, manifest, arm_a, arm_b, out):
        cmd = [
            sys.executable,
            "-I",
            str(ANALYZE_PATH),
            "--corpus",
            str(CORPUS_PATH),
            "--preregistration",
            str(manifest),
            "--repo-root",
            str(REPO),
            "--arm-a",
            str(arm_a),
            "--arm-b",
            str(arm_b),
            "--expected-commit",
            EXPECTED_COMMIT,
            "--out",
            str(out),
        ]
        proc = subprocess.run(cmd, cwd=str(REPO), capture_output=True, text=True)
        if proc.returncode != 0:
            self.fail("analysis failed: %s%s" % (proc.stdout, proc.stderr))
        return json.loads(Path(out).read_text())

    def test_committed_evidence_with_manifest_is_positive(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = self.run_cli(
                PREREGISTRATION,
                EVIDENCE / "arm-local-arm64",
                EVIDENCE / "arm-remote-amd64",
                Path(tmp) / "results.json",
            )
        self.assertEqual(result["decision"]["category"], "POSITIVE")
        self.assertTrue(result["manifest_checked"])
        self.assertTrue(result["validity"]["valid"], result["validity"]["reasons"])
        self.assertEqual(result["primary_endpoint"]["total"], 24)
        self.assertEqual(result["primary_endpoint"]["identical"], 24)
        self.assertEqual(result["interval"]["lower"], analyze.clopper_pearson(24, 24)[0])
        self.assertEqual(result["arms"]["arm_a"]["host_arch"], "aarch64")
        self.assertEqual(result["arms"]["arm_b"]["host_arch"], "x86_64")
        for arm in ("arm_a", "arm_b"):
            self.assertEqual(
                result["arms"][arm]["tools"]["wc"]["value"],
                "wc (GNU coreutils) 9.7",
            )
            self.assertEqual(
                result["arms"][arm]["tools"]["wc"]["source"],
                "container-supplement.txt",
            )
        self.assertTrue(result["env_lock_identity"])
        for row in result["env_lock_identity"]:
            self.assertTrue(row["normalized_equal"], row)
        self.assertNotEqual(
            result["env_lock_identity"][0]["arm_a_raw"],
            result["env_lock_identity"][0]["arm_b_raw"],
        )

    def test_tampered_sealed_hash_is_invalid(self):
        manifest = json.loads(PREREGISTRATION.read_text())
        manifest["files"]["scripts/cross-arch-provenance-governed-corpus.json"] = "0" * 64
        with tempfile.TemporaryDirectory() as tmp:
            manifest_path = Path(tmp) / "preregistration.json"
            manifest_path.write_text(json.dumps(manifest))
            result = self.run_cli(
                manifest_path,
                EVIDENCE / "arm-local-arm64",
                EVIDENCE / "arm-remote-amd64",
                Path(tmp) / "results.json",
            )
        self.assertEqual(result["decision"]["category"], "INVALID")
        codes = {reason["code"] for reason in result["validity"]["reasons"]}
        self.assertIn("SEALED_HASH_MISMATCH", codes)

    def test_forged_host_arch_is_invalid(self):
        with tempfile.TemporaryDirectory() as tmp:
            arm = Path(tmp) / "arm-local"
            shutil.copytree(EVIDENCE / "arm-local-arm64", arm)
            lines = []
            for line in (arm / "identity.txt").read_text().splitlines():
                if line.startswith("host-arch:"):
                    lines.append("host-arch:       x86_64")
                else:
                    lines.append(line)
            (arm / "identity.txt").write_text("\n".join(lines) + "\n")
            result = self.run_cli(
                PREREGISTRATION,
                arm,
                EVIDENCE / "arm-remote-amd64",
                Path(tmp) / "results.json",
            )
        self.assertEqual(result["decision"]["category"], "INVALID")
        codes = {reason["code"] for reason in result["validity"]["reasons"]}
        self.assertIn("HOST_ARCH_MISMATCH", codes)

    def test_tampered_checkpoint_hash_is_invalid(self):
        with tempfile.TemporaryDirectory() as tmp:
            arm = Path(tmp) / "arm-local"
            shutil.copytree(EVIDENCE / "arm-local-arm64", arm)
            checkpoint_path = arm / "crossarch-governed-a.checkpoint.json"
            checkpoint = json.loads(checkpoint_path.read_text())
            first = sorted(checkpoint["checksums"])[0]
            checkpoint["checksums"][first] = "sha256:" + "0" * 64
            checkpoint_path.write_text(json.dumps(checkpoint))
            result = self.run_cli(
                PREREGISTRATION,
                arm,
                EVIDENCE / "arm-remote-amd64",
                Path(tmp) / "results.json",
            )
        self.assertEqual(result["decision"]["category"], "INVALID")
        codes = {reason["code"] for reason in result["validity"]["reasons"]}
        self.assertIn("CHECKPOINT_INDEPENDENT_HASH_MISMATCH", codes)


if __name__ == "__main__":
    unittest.main()
