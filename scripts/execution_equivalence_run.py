#!/usr/bin/env python3
"""Execution-equivalence runner (#1304, Bet-3 / Gate-3 measurement).

For each fixture in scripts/execution_equivalence_expectations.py, run the SAME
intent natively in three engines -- oxo-flow, Nextflow DSL2, Snakemake -- over
the same inputs and the same pinned container digest, then compare the
SHA-256 of the CONTENT of every declared output path.

HARD RULE: NEVER FABRICATE A MEASUREMENT.
  * An engine that did not run (missing binary, non-zero exit, absent output)
    makes its cell SKIPPED, never MATCHED.
  * A fixture is MATCHED only when all three arms ran to exit 0 AND every
    declared output exists with the same SHA-256 in every arm.
  * A fixture where some arm ran but the hashes differ is DRIFTED, with the
    per-cell why recorded.
  * If the two reference engines are unreachable the whole run is SKIPPED and
    the process still exits 0 -- a machine without the toolchain must not read
    as a failure, but it must also never read as a pass.

Exit status: 1 iff at least one fixture is DRIFTED, else 0.

Usage: execution_equivalence_run.py <repo_root> <oxo-flow-bin> <scratch-dir>
"""

import hashlib
import json
import os
import shlex
import shutil
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import execution_equivalence_expectations as EXP  # noqa: E402

FIXTURE_ROOT = os.path.join("tests", "fixtures", "execution-equivalence")

# Engine-specific scratch/state directories that must NOT be counted as
# declared outputs (and are not copied when staging an arm).
SKIP_DIRS = {".nextflow", "work", ".snakemake", ".oxo-flow", "logs",
             "__pycache__", ".pytest_cache"}

ARM_NOTES = {
    "oxo-flow": ["native engine; per-rule [rules.environment] docker directive"],
    "nextflow": ["container directive needs a bash-bearing image (python:3.12-slim)"],
    "snakemake": ["container supplied via --config container=<SIF|docker://...>"],
}


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def which(name):
    return shutil.which(name)


def stage_arm(fixture_dir, dest):
    if os.path.exists(dest):
        shutil.rmtree(dest)
    shutil.copytree(fixture_dir, dest)
    for junk in os.listdir(dest):
        if junk in SKIP_DIRS:
            shutil.rmtree(os.path.join(dest, junk), ignore_errors=True)


def run(cmd, cwd, env):
    start = time.monotonic()
    try:
        proc = subprocess.run(cmd, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                              stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                              text=True, timeout=1800)
        return proc.returncode, time.monotonic() - start, proc.stdout
    except subprocess.TimeoutExpired:
        return 124, time.monotonic() - start, "TIMEOUT"
    except FileNotFoundError as exc:
        return 127, time.monotonic() - start, str(exc)


def arm_plan(arm, fixture, dest, oxo_bin):
    """(cmd, cwd, env, restart_cmd) for one arm, or None if unreachable."""
    env = dict(os.environ)
    entry = EXP.ENTRY[arm]
    if arm == "oxo-flow":
        cmd = [oxo_bin, "run", entry, "-j", "2"]
        return cmd, dest, env, list(cmd)
    if arm == "nextflow":
        nxf = os.environ.get("EQ_NEXTFLOW_BIN", "nextflow")
        if not which(nxf):
            return None
        prof = shlex.split(os.environ.get("EQ_NEXTFLOW_PROFILE", "-with-docker"))
        work = os.path.join(dest, "..", fixture + "-nxf-work")
        env["NXF_HOME"] = os.path.join(dest, "..", "nxf-home")
        base = [nxf, "run", entry, "-work-dir", work, "-ansi-log", "false"] + prof
        return base, dest, env, base + ["-resume"]
    if arm == "snakemake":
        smk = os.environ.get("EQ_SNAKEMAKE_BIN", "snakemake")
        if not which(smk):
            return None
        cont = os.environ.get("EQ_SNAKEMAKE_CONTAINER",
                              "docker://" + EXP.CONTAINER)
        base = [smk, "--use-singularity", "--config", "container=" + cont,
                "-j", "2"]
        extra = shlex.split(os.environ.get("EQ_SNAKEMAKE_EXTRA", ""))
        base += extra
        return base, dest, env, list(base)
    raise KeyError(arm)


def collect_outputs(arm_dir, declared):
    out = {}
    missing = []
    for rel in declared:
        path = os.path.join(arm_dir, rel)
        if os.path.isfile(path):
            out[rel] = sha256(path)
        else:
            missing.append(rel)
    return out, missing


def run_arm(fixture_dir, fixture, arm, scratch, oxo_bin):
    dest = os.path.join(scratch, fixture, arm.replace("-", ""))
    stage_arm(fixture_dir, dest)
    plan = arm_plan(arm, fixture, dest, oxo_bin)
    rec = {"arm": arm, "ran": False, "exit": None, "seconds": None,
           "outputs": {}, "missing": list(EXP.get(fixture)["declared_outputs"]),
           "restart_exit": None, "restart_same": None,
           "notes": list(ARM_NOTES.get(arm, [])), "command": None,
           "why": None}
    if plan is None:
        rec["why"] = "engine binary not reachable on PATH"
        return rec
    cmd, cwd, env, restart_cmd = plan
    rec["command"] = " ".join(cmd)
    rc, secs, tail = run(cmd, cwd, env)
    rec["exit"] = rc
    rec["seconds"] = round(secs, 2)
    outputs, missing = collect_outputs(dest, EXP.get(fixture)["declared_outputs"])
    rec["outputs"] = outputs
    rec["missing"] = missing
    if rc != 0:
        rec["why"] = "engine exit %d" % rc
        rec["tail"] = tail[-400:]
        return rec
    if missing:
        rec["why"] = "declared output(s) absent: " + ", ".join(missing)
        return rec
    rec["ran"] = True
    # restart-into-cache: run again, expect exit 0 with unchanged outputs.
    rrc, _rsecs, _tail = run(restart_cmd, cwd, env)
    rec["restart_exit"] = rrc
    again, _m = collect_outputs(dest, EXP.get(fixture)["declared_outputs"])
    rec["restart_same"] = (again == outputs)
    return rec


def classify(fixtures, records):
    """records: {fixture: {arm: rec}} -> (verdict, why)."""
    arms = ["oxo-flow", "nextflow", "snakemake"]
    if not all(records[f][a]["ran"] for f in fixtures for a in arms):
        absent = sorted({a for f in fixtures for a in arms
                         if not records[f][a]["ran"]})
        return "SKIPPED", "arm(s) did not run: " + ", ".join(absent)
    for f in fixtures:
        declared = EXP.get(f)["declared_outputs"]
        ref = records[f]["oxo-flow"]["outputs"]
        for a in arms:
            got = records[f][a]["outputs"]
            if set(got) != set(declared):
                return "DRIFTED", "%s/%s output set mismatch" % (f, a)
            for rel in declared:
                if got[rel] != ref[rel]:
                    return "DRIFTED", "%s/%s differs on %s" % (f, a, rel)
    return "MATCHED", "all declared outputs byte-identical across three arms"


def main():
    if len(sys.argv) < 4:
        sys.stderr.write(__doc__)
        return 2
    root, oxo_bin, scratch = sys.argv[1], sys.argv[2], sys.argv[3]
    if not os.path.isfile(oxo_bin):
        found = which("oxo-flow") or which("oxo_flow")
        if found:
            oxo_bin = found
    os.makedirs(scratch, exist_ok=True)

    fixtures = EXP.fixture_names()
    bad_refs = EXP.unknown_import_refs()
    if bad_refs:
        print("WARNING: fixture import references not in the import corpus:")
        for name, ref in bad_refs:
            print("  - %s -> %s" % (name, ref))

    records = {}
    tally = {"MATCHED": 0, "DRIFTED": 0, "SKIPPED": 0}
    print("%-18s %-8s %-4s %-5s %-7s %-6s %s" %
          ("FIXTURE", "ARM", "RAN", "EXIT", "SECS", "OUTS", "VERDICT"))
    print("-" * 78)
    verdicts = {}
    for fixture in fixtures:
        fx_dir = os.path.join(root, FIXTURE_ROOT, fixture)
        records[fixture] = {}
        if not os.path.isdir(fx_dir):
            for arm in ("oxo-flow", "nextflow", "snakemake"):
                records[fixture][arm] = {
                    "arm": arm, "ran": False, "outputs": {}, "missing": [],
                    "why": "fixture directory absent: " + fx_dir}
        else:
            for arm in ("oxo-flow", "nextflow", "snakemake"):
                records[fixture][arm] = run_arm(
                    fx_dir, fixture, arm, scratch, oxo_bin)
        verdict, why = classify([fixture], records)
        verdicts[fixture] = (verdict, why)
        tally[verdict] += 1
        for arm in ("oxo-flow", "nextflow", "snakemake"):
            rec = records[fixture][arm]
            declared = EXP.get(fixture)["declared_outputs"]
            have = len(rec["outputs"])
            print("%-18s %-8s %-4s %-5s %-7s %-6s %s" % (
                fixture if arm == "oxo-flow" else "", arm,
                "yes" if rec["ran"] else "no",
                "-" if rec["exit"] is None else rec["exit"],
                "-" if rec["seconds"] is None else rec["seconds"],
                "%d/%d" % (have, len(declared)) if rec["ran"] else "-",
                ("" if rec["ran"] else rec["why"] or "")))
        print("  -> %s: %s -- %s" % (fixture, verdict, why))

    print("-" * 78)
    print("TALLY  MATCHED=%d  DRIFTED=%d  SKIPPED=%d" %
          (tally["MATCHED"], tally["DRIFTED"], tally["SKIPPED"]))

    report = {
        "container": EXP.CONTAINER,
        "import_corpus_supported": EXP.supported_import_fixtures(),
        "verdicts": {k: list(v) for k, v in verdicts.items()},
        "tally": tally,
        "fixtures": records,
    }
    out_path = os.path.join(scratch, "execution-equivalence-records.json")
    with open(out_path, "w") as fh:
        json.dump(report, fh, indent=2, sort_keys=True)
    print("raw records: %s" % out_path)

    return 1 if tally["DRIFTED"] else 0


if __name__ == "__main__":
    sys.exit(main())
