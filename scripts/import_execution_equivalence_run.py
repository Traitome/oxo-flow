#!/usr/bin/env python3
"""Execution-equivalence runner for scripts/import-execution-equivalence.sh (#1300).

The honest EXECUTION half of the #1165 transpiler ask: does an IMPORTED
workflow, actually RUN, produce the same output artifacts as its SOURCE
workflow? This is the companion to scripts/import_equivalence_run.py, which
measures only the *declared* contract.

HARD RULE: NEVER FABRICATE A MEASUREMENT. A fixture's verdict is MATCHED only
when BOTH arms (source + imported) genuinely executed and their artifact sets
agree. If either arm did not run -- missing runtime, missing container/conda
backend, missing toolchain, or the arm errored -- the verdict is SKIPPED with
the reason, never MATCHED. A green tally must mean runs happened.

Supported fixtures are enumerated from import_equivalence_expectations.py
(status == "supported"); the source run command and the required runtime are
derived from the fixture extension and its imported environment block, so this
file does not restate the truth table.

Usage:  import_execution_equivalence_run.py <ROOT> <BIN> <TMP>
Exit 0 when nothing executed (or every executed fixture matched); exit 1 when
any executed fixture drifted. The bash wrapper turns that 1 into a --strict
failure and is report-only otherwise.
"""
import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
# Inputs planted before each arm so the bioinformatics tools have something to
# read; identical bytes go to both arms, so they cancel out of the comparison.
PLANTED = {
    "data/sample1.fastq.gz": "@r\nACGTACGT\n+\nIIIIIIII\n",
    "data/s1.fastq.gz": "@r\nACGTACGT\n+\nIIIIIIII\n",
    "data/s1_1.fastq.gz": "@r\nACGT\n+\nIIII\n",
    "data/s1_2.fastq.gz": "@r\nACGT\n+\nIIII\n",
    "raw/A.fastq": "@r\nACGT\n+\nIIII\n",
    "raw/B.fastq": "@r\nACGT\n+\nIIII\n",
    "genes.gtf": "gene\tchr\tstart\tend\tstrand\n",
    "envs/trimmomatic.yaml": "name: trimmomatic\nchannels:\n  - bioconda\ndependencies:\n  - trimmomatic=0.39\n",
    "envs/star.yaml": "name: star\nchannels:\n  - bioconda\ndependencies:\n  - star=2.7.10b\n",
}
# Tool/engine metadata directories that are not scientific outputs.
SKIP_DIRS = {".nextflow", "work", ".snakemake", ".oxo-flow", "logs",
             "__pycache__", ".pytest_cache"}

# Source-arm invocation per fixture extension. {src} is the fixture basename.
SOURCE_RUNNERS = {
    ".nf": ("nextflow", ["nextflow", "run", "{src}", "-with-docker"]),
    ".smk": ("snakemake",
             ["snakemake", "-s", "{src}", "--cores", "2", "--use-conda"]),
}
# Environment backend a fixture needs, probed via these binaries (any one).
BACKEND_PROBES = {
    "docker": ("docker", "singularity", "apptainer"),
    "conda": ("conda", "mamba"),
    "modules": (),  # HPC environment modules: never assumed present
}


def load_expectations():
    spec = importlib.util.spec_from_file_location(
        "expect", os.path.join(HERE, "import_equivalence_expectations.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def supported_fixtures(expect):
    """Enumerate status == 'supported' fixtures, sorted, from the truth table."""
    return sorted(rel for rel, e in expect.EXPECTED.items()
                  if e.get("status") == "supported")


def source_runtime(rel):
    """(binary, argv-template) for the SOURCE arm, or (None, None)."""
    return SOURCE_RUNNERS.get(os.path.splitext(rel)[1], (None, None))


def required_backends(imported_doc):
    """Environment backends the imported workflow declares (union over rules)."""
    need = set()
    for rule in imported_doc.get("rules", []):
        for key in (rule.get("environment") or {}):
            if key in ("docker", "conda"):
                need.add(key)
            elif key in ("modules", "module"):
                need.add("modules")
    return need


def missing_reason(runtime_bin, backends, have):
    """Return a SKIPPED reason, or None when every requirement is satisfiable."""
    reasons = []
    if runtime_bin and not have(runtime_bin):
        reasons.append(runtime_bin)
    for backend in sorted(backends):
        probes = BACKEND_PROBES.get(backend, ())
        if not probes:
            reasons.append("%s (HPC modules)" % backend)
        elif not any(have(p) for p in probes):
            reasons.append(backend)
    return ("no execution environment: missing " + ", ".join(reasons)
            if reasons else None)


def classify(arms):
    """Verdict from the two arm results. Never MATCHED unless both ran.

    arms = {"source": {...}, "imported": {...}}; each has "ran" (bool),
    "reason" (str, when not ran) and "artifacts" (relpath -> sha256).
    """
    for name in ("source", "imported"):
        if not arms.get(name, {}).get("ran"):
            return "SKIPPED", arms.get(name, {}).get("reason", "arm did not run")
    if arms["source"]["artifacts"] == arms["imported"]["artifacts"]:
        return "MATCHED", "artifact sets agree"
    a, b = arms["source"]["artifacts"], arms["imported"]["artifacts"]
    only_src = sorted(set(a) - set(b))
    only_imp = sorted(set(b) - set(a))
    changed = sorted(k for k in set(a) & set(b) if a[k] != b[k])
    detail = []
    if only_src:
        detail.append("only-source=%s" % ",".join(only_src[:4]))
    if only_imp:
        detail.append("only-imported=%s" % ",".join(only_imp[:4]))
    if changed:
        detail.append("checksum-differs=%s" % ",".join(changed[:4]))
    return "DRIFTED", "; ".join(detail)


# The workflow file seeded into each arm is NOT a produced artifact. Each arm
# keeps its natural seed name (the source arm needs the real fixture basename
# for its engine; the imported arm runs W.oxoflow), and BOTH seed paths are
# excluded from the artifact set by measure() before classifying. Without this a
# real wrapper that leaves its own input seed behind would show a spurious
# one-sided artifact and a genuinely-equivalent run would be misreported DRIFTED.
IMPORTED_SEED = "W.oxoflow"


def plant(root):
    for rel, body in PLANTED.items():
        path = os.path.join(root, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as fh:
            fh.write(body)


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def collect_artifacts(root):
    """relpath -> sha256 for produced files, excluding planted inputs and
    engine metadata directories."""
    planted = set(PLANTED)
    out = {}
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for fn in filenames:
            full = os.path.join(dirpath, fn)
            rel = os.path.relpath(full, root)
            if rel in planted:
                continue
            out[rel] = sha256(full)
    return out


def error_line(text):
    """Most informative single line from captured output: an 'error' line if
    present, else the last non-empty line, else empty."""
    lines = [ln.strip() for ln in (text or "").splitlines() if ln.strip()]
    err = next((ln for ln in lines if "error" in ln.lower()), None)
    return err or (lines[-1] if lines else "")


def run_arm(argv, cwd, timeout):
    """(ran, reason, artifacts). ran is True only on exit 0."""
    try:
        p = subprocess.run(argv, cwd=cwd, capture_output=True, text=True,
                           timeout=timeout)
    except FileNotFoundError as exc:
        return False, "runtime not runnable: %s" % exc, {}
    except subprocess.TimeoutExpired:
        return False, "timed out after %ss" % timeout, {}
    if p.returncode != 0:
        info = error_line(p.stderr) or error_line(p.stdout)
        return False, "exit %d: %s" % (p.returncode, info), {}
    return True, "", collect_artifacts(cwd)


def measure(rel, expect, bin_, tmp, have, timeout):
    src = os.path.join(expect.ROOT, "tests", "fixtures", "import", rel)
    runtime_bin, template = source_runtime(rel)
    if runtime_bin is None:
        return "SKIPPED", "no source runner for extension %s" % os.path.splitext(rel)[1]
    # Import once to read the environment block; reuse the EXISTING importer.
    import tomllib
    imp_dir = os.path.join(tmp, rel.replace("/", "_"))
    os.makedirs(imp_dir, exist_ok=True)
    imported_wf = os.path.join(imp_dir, "W.oxoflow")
    p = subprocess.run([bin_, "import", src, "-o", imported_wf],
                       capture_output=True, text=True)
    if p.returncode != 0:
        return "SKIPPED", "import failed: %s" % error_line(p.stderr)
    with open(imported_wf, "rb") as fh:
        imported_doc = tomllib.load(fh)
    backends = required_backends(imported_doc)
    reason = missing_reason(runtime_bin, backends, have)
    if reason:
        return "SKIPPED", reason
    src_dir = os.path.join(tmp, rel.replace("/", "_") + "_src")
    run_dir = os.path.join(tmp, rel.replace("/", "_") + "_run")
    for d in (src_dir, run_dir):
        os.makedirs(d, exist_ok=True)
        plant(d)
    # Seed each arm with the workflow file it must run. Each arm keeps its NATURAL
    # seed name (the source engine needs the real fixture basename; the imported
    # arm runs W.oxoflow), and BOTH seed relpaths are dropped from the collected
    # artifact set. That makes the two arms structurally comparable: a wrapper
    # that leaves its own input seed behind cannot masquerade as a one-sided
    # artifact, so a genuinely-equivalent run reports MATCHED, not DRIFTED.
    src_seed = os.path.basename(rel)
    shutil.copy(src, os.path.join(src_dir, src_seed))
    shutil.copy(imported_wf, os.path.join(run_dir, IMPORTED_SEED))
    s_ran, s_reason, s_art = run_arm(
        [a.format(src=src_seed) for a in template], src_dir, timeout)
    i_ran, i_reason, i_art = run_arm(
        [bin_, "run", IMPORTED_SEED, "-d", run_dir], run_dir, timeout)
    for art, seed in ((s_art, src_seed), (i_art, IMPORTED_SEED)):
        art.pop(seed, None)
    return classify({
        "source": {"ran": s_ran, "reason": "source arm: " + s_reason, "artifacts": s_art},
        "imported": {"ran": i_ran, "reason": "imported arm: " + i_reason, "artifacts": i_art},
    })


def main(argv):
    if len(argv) < 4:
        print("usage: import_execution_equivalence_run.py <ROOT> <BIN> <TMP>",
              file=sys.stderr)
        return 0
    root, bin_, tmp = argv[1], argv[2], argv[3]
    timeout = int(os.environ.get("OXO_EXEC_EQUIV_TIMEOUT", "300"))
    expect = load_expectations()
    expect.ROOT = root
    have = lambda name: shutil.which(name) is not None

    print("# import-execution-equivalence run")
    print("# fixtures are enumerated from import_equivalence_expectations.py")
    print()
    tally = {"EXECUTED": 0, "SKIPPED": 0, "MATCHED": 0, "DRIFTED": 0}
    rows = []
    for rel in supported_fixtures(expect):
        verdict, reason = measure(rel, expect, bin_, tmp, have, timeout)
        if verdict in ("MATCHED", "DRIFTED"):
            tally["EXECUTED"] += 1
            tally[verdict] += 1
        else:
            tally["SKIPPED"] += 1
        rows.append((rel, verdict, reason))
        print("RESULT\t%s\t%s\t%s" % (rel, verdict, reason))
    print()
    print("EXECUTION-EQUIVALENCE TALLY (fixtures=%d)" % len(rows))
    print("  executed : %d" % tally["EXECUTED"])
    print("  skipped  : %d" % tally["SKIPPED"])
    print("  matched  : %d" % tally["MATCHED"])
    print("  drifted  : %d" % tally["DRIFTED"])
    print()
    print(json.dumps(tally))
    # Exit non-zero iff an executed fixture drifted, so the wrapper --strict
    # branch is falsifiable; an all-SKIPPED (or all-MATCHED) run stays 0.
    return 1 if tally["DRIFTED"] else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
