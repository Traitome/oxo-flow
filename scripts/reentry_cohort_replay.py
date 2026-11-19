#!/usr/bin/env python3
"""Cohort-scale deterministic replay harness for checkpoint re-entry +
[[values]] expansion (issue #1245, ROADMAP Stage 12.2).

Generates a synthetic workflow with N samples x M [[values]] strata across
several rules deep, where a `checkpoint = true` producer contributes runtime
re-entry (one extra sample + one extra stratum). Measures COLD vs RESUME wall
clock and proves the instance-identity set is identical across resume.

Report-only: prints the measured table and always exits 0. The fast CI proof of
the same contract lives in tests/reentry_cohort_replay.rs.

Usage:
  python3 scripts/reentry_cohort_replay.py [N] [M]
Env:
  OXO_FLOW_BIN   path to the oxo-flow binary (default target/debug, then PATH)
"""
import json
import os
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def find_bin():
    b = os.environ.get("OXO_FLOW_BIN", "")
    if b:
        return b
    cand = os.path.join(ROOT, "target", "debug", "oxo-flow")
    if os.access(cand, os.X_OK):
        return cand
    from shutil import which
    return which("oxo-flow")


def gen_workflow(work, n, m):
    samples = ", ".join('"S%03d"' % i for i in range(1, n + 1))
    strata = ", ".join('"s%02d"' % k for k in range(m))
    new_strat = "s%02d" % m
    manifest = '[reentry]\\nsample = [\\"SNEW\\"]\\nvalues = { stratum = [\\"%s\\"] }\\n' % new_strat
    wf = """
[workflow]
name = "reentry-cohort"

[[sample_groups]]
name = "cohort"
samples = [{samples}]

[[values]]
name = "stratum"
values = [{strata}]

[[rules]]
name = "discover"
input = ["catalog.txt"]
output = ["discover.toml"]
shell = "printf '%s' '{manifest}' > discover.toml"
checkpoint = true
checkpoint_manifest = "discover.toml"

[[rules]]
name = "stage1"
input = ["discover.toml"]
output = ["s1/{{sample}}_{{stratum}}.txt"]
shell = "echo {{sample}}_{{stratum}} > s1/{{sample}}_{{stratum}}.txt"

[[rules]]
name = "stage2"
input = ["s1/{{sample}}_{{stratum}}.txt"]
output = ["s2/{{sample}}_{{stratum}}.txt"]
shell = "echo {{sample}}_{{stratum}} > s2/{{sample}}_{{stratum}}.txt"
""".format(samples=samples, strata=strata, manifest=manifest)
    with open(os.path.join(work, "wf.oxoflow"), "w") as fh:
        fh.write(wf)
    with open(os.path.join(work, "catalog.txt"), "w") as fh:
        fh.write("v1\n")


def run(binary, work, extra):
    t0 = time.perf_counter()
    p = subprocess.run([binary, "run", "wf.oxoflow"] + extra, cwd=work,
                       capture_output=True, text=True)
    return time.perf_counter() - t0, p


def plan_names(binary, work):
    p = subprocess.run([binary, "dry-run", "wf.oxoflow", "--json"], cwd=work,
                       capture_output=True, text=True)
    data = json.loads(p.stdout)
    return sorted(r["name"] for r in data["plan"])


def completed_names(work):
    with open(os.path.join(work, ".oxo-flow", "checkpoint.json")) as fh:
        data = json.load(fh)
    return sorted(data["completed_rules"])


def main():
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 200
    m = int(sys.argv[2]) if len(sys.argv) > 2 else 3
    binary = find_bin()
    if not binary:
        print("no oxo-flow binary (set OXO_FLOW_BIN)", file=sys.stderr)
        return 2
    import platform
    print("# reentry cohort replay")
    print("# binary:  %s" % binary)
    print("# host:    %s %s %s" % (platform.node(), platform.system(), platform.machine()))
    try:
        sha = subprocess.run(["git", "-C", ROOT, "rev-parse", "HEAD"],
                             capture_output=True, text=True).stdout.strip()
    except Exception:
        sha = "unknown"
    print("# commit:  %s" % (sha or "unknown"))
    print("# cohort:  N=%d samples x M=%d strata; re-entry adds 1 sample + 1 stratum" % (n, m))

    work = tempfile.mkdtemp(prefix="reentry-cohort-")
    gen_workflow(work, n, m)

    cold_s, cold = run(binary, work, ["-j", "4"])
    assert cold.returncode == 0, cold.stderr
    cold_set = completed_names(work)
    expected = (n + 1) * (m + 1) * 2 + 1
    print("\n== cold run ==")
    print("wall_clock=%.2fs instances=%d (expected %d)" % (cold_s, len(cold_set), expected))
    assert len(cold_set) == expected, "cold instance count mismatch"

    resume_s, resume = run(binary, work, ["-j", "4"])
    assert resume.returncode == 0, resume.stderr
    resume_set = completed_names(work)
    plan = plan_names(binary, work)
    print("\n== resume run ==")
    print("wall_clock=%.2fs" % resume_s)
    print("identity_set_equal=%s (cold==resume)" % (cold_set == resume_set))
    print("plan_set_equal=%s    (preview==resume)" % (plan == resume_set))
    assert cold_set == resume_set, "resume dropped/duplicated an instance"
    assert plan == resume_set, "resume preview drifted"

    resume2_s, resume2 = run(binary, work, ["-j", "4"])
    assert resume2.returncode == 0, resume2.stderr
    stable = completed_names(work) == cold_set
    print("resume_stable=%s    (2nd resume == 1st)" % stable)
    assert stable, "repeated resume not stable"

    print("\n== summary ==")
    print("instances=%d  cold=%.2fs  resume=%.2fs  speedup=%.1fx"
          % (len(cold_set), cold_s, resume_s, cold_s / resume_s if resume_s else 0.0))
    print("# done")
    return 0


if __name__ == "__main__":
    sys.exit(main())
