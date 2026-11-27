#!/usr/bin/env python3
"""Execution-equivalence runner for scripts/import-execution-equivalence.sh (#1300).

The honest EXECUTION half of the #1165 transpiler ask: does an IMPORTED
workflow, actually RUN, produce the same output artifacts as its SOURCE
workflow? This is the companion to scripts/import_equivalence_run.py, which
measures only the *declared* contract.

HARD RULE: NEVER FABRICATE A MEASUREMENT. A fixture's verdict is MATCHED only
when BOTH arms (source + imported) genuinely executed AND each produced at
least one artifact. If either arm did not run -- missing runtime, missing
container/conda backend, missing toolchain, or the arm errored -- the verdict
is SKIPPED with the reason, never MATCHED. If both arms ran but produced zero
artifacts, the verdict is NO_ARTIFACTS, never MATCHED: a runtime that exits 0
while writing nothing is not evidence of equivalence (issue #1349). A green
tally must mean runs happened.

The comparison endpoint is the COMPLETE produced-artifact path->SHA-256 map
(planted inputs, the seeded workflow files, and engine metadata excluded), so
an incidental file can never be the only agreement. When the imported
workflow declares literal outputs (no wildcard syntax), each must also be
present in both arms or the fixture is SKIPPED as "declared output missing".
A fixture whose `execution` block pins a container must import to that same
container (the `docker://` scheme prefix is normalized before comparison),
else it is SKIPPED as a container mismatch. Wildcard declared outputs are not
expanded for the membership check; the artifact-set comparison still covers
whatever the arms produced.

Requires Python >= 3.11 (tomllib). On an older interpreter the harness exits 2
with an explicit message instead of a silent-green empty result.

Supported fixtures are enumerated from import_equivalence_expectations.py
(status == "supported"). A fixture may carry an `execution` block naming the
exact SOURCE-arm argv (`source`, with {src} for the fixture basename), the
backend the source needs, and the pinned container; otherwise the source run
command and required runtime are derived from the fixture extension and its
imported environment block. Set EQ_RECORDS_PATH to persist the per-arm raw
records (command, exit state, artifact SHA-256 map) as JSON.

Usage:  import_execution_equivalence_run.py <ROOT> <BIN> <TMP>
Exit 0 when nothing executed (or every executed fixture matched); exit 1 when
any executed fixture drifted; exit 2 on an interpreter below the 3.11 floor.
The bash wrapper turns 1 into a --strict failure and is report-only otherwise
(with a stderr warning when the runner failed).
"""
import hashlib
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys

try:
    import tomllib  # Python >= 3.11; guarded so old interpreters fail loudly (#1349)
except ModuleNotFoundError:  # pragma: no cover - exercised via the tomllib=None test
    tomllib = None

HERE = os.path.dirname(os.path.abspath(__file__))
# Inputs planted before each arm so the bioinformatics tools have something to
# read; identical bytes go to both arms, so they cancel out of the comparison.
# The two pinned real-world fixtures (#1398) consume one FASTA each; both files
# are planted for every fixture and excluded from the artifact set, so neither
# is ever mistaken for a produced output.
_PLANTED_FASTA = ">chr1\nACGTACGTACGTACGTACGTACGTACGTACGTACGT\n>chr2\nTTTTGGGGCCCCAAAA\n"
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
    "GENOME_PATH/genome.fna": _PLANTED_FASTA,
    "GENOME_PATH/GCF_000001215.4_Release_6_plus_ISO1_MT_genomic.fna": _PLANTED_FASTA,
}
# Tool/engine metadata directories that are not scientific outputs.
SKIP_DIRS = {".nextflow", "work", ".snakemake", ".oxo-flow", "logs",
             "__pycache__", ".pytest_cache"}
# Nextflow writes its own run log into the launch directory; it is engine
# bookkeeping, not a declared output.
SKIP_FILE_PREFIXES = (".nextflow.log",)

# Source-arm invocation per fixture extension. {src} is the fixture basename.
# A fixture may override the whole template in its `execution.source` entry
# (issue #1398) so the SOURCE arm honors the engine/backend/container the
# real-world source declares instead of one hardcoded flag choice per language.
SOURCE_RUNNERS = {
    ".nf": ("nextflow", ["nextflow", "run", "{src}", "-with-docker"]),
    ".smk": ("snakemake",
             ["snakemake", "-s", "{src}", "--cores", "2", "--use-conda"]),
}
# Binaries that can be overridden by environment (installed outside PATH on the
# qualification host).
ENGINE_ENV = {
    "nextflow": ("OXO_EXEC_NEXTFLOW_BIN", "nextflow"),
    "snakemake": ("OXO_EXEC_SNAKEMAKE_BIN", "snakemake"),
}
# Environment backend a fixture needs, probed via these binaries (any one).
BACKEND_PROBES = {
    "docker": ("docker", "singularity", "apptainer"),
    "singularity": ("singularity", "apptainer"),
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


def source_runtime(rel, entry=None):
    """(binary, argv-template) for the SOURCE arm, or (None, None).

    A truth-table entry may declare `execution.source` (the exact argv with the
    fixture basename as `{src}`) and `execution.container`; that declaration
    wins over the per-extension default so a real-world source keeps the
    engine/backend flags it actually needs.
    """
    execution = (entry or {}).get("execution") or {}
    template = execution.get("source")
    if template:
        engine = template[0]
        env_var, default = ENGINE_ENV.get(engine, (None, engine))
        binary = os.environ.get(env_var) or default
        return binary, [binary] + list(template[1:])
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


def normalize_container(spec):
    """Compare container specs independent of the transfer scheme prefix.

    A fixture declares e.g. `docker://registry/img:tag` while the importer
    emits `registry/img:tag` under `environment.docker`; the same image must
    compare equal or a future fixture could run a different container than the
    one its `execution` block pins.
    """
    spec = (spec or "").strip()
    for prefix in ("docker://", "shub://"):
        if spec.startswith(prefix):
            return spec[len(prefix):]
    return spec


def imported_containers(imported_doc):
    """Normalized container specs the imported workflow declares, in order."""
    out = []
    for rule in imported_doc.get("rules", []):
        environment = rule.get("environment") or {}
        for key in ("docker", "singularity"):
            spec = environment.get(key)
            if spec:
                out.append(normalize_container(spec))
    return out


def container_mismatch(imported_doc, execution):
    """Reason when `execution.container` is not what the imported workflow runs."""
    declared = execution.get("container")
    if not declared:
        return None
    want = normalize_container(declared)
    have = imported_containers(imported_doc)
    if want in have:
        return None
    return ("container mismatch: execution declares %s; imported environment has %s"
            % (declared, ", ".join(have) if have else "no container"))


# A path with any of these characters is not a single literal artifact and
# cannot be membership-checked without expanding the wildcard domain.
WILDCARD_CHARS = set("{}*[]?")


def declared_literal_outputs(imported_doc):
    """Declared output paths that carry no wildcard/glob syntax."""
    out = []
    for rule in imported_doc.get("rules", []):
        for path in rule.get("output") or []:
            if not any(ch in WILDCARD_CHARS for ch in path):
                out.append(path)
    return out


def missing_declared_outputs(artifacts, declared):
    """Declared literal outputs absent from a produced artifact map."""
    return [path for path in declared if path not in artifacts]


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
    """Verdict from the two arm results. Never MATCHED unless both ran AND
    produced at least one artifact.

    arms = {"source": {...}, "imported": {...}}; each has "ran" (bool),
    "reason" (str, when not ran) and "artifacts" (relpath -> sha256).
    """
    for name in ("source", "imported"):
        if not arms.get(name, {}).get("ran"):
            return "SKIPPED", arms.get(name, {}).get("reason", "arm did not run")
    a, b = arms["source"]["artifacts"], arms["imported"]["artifacts"]
    if not a and not b:
        # Two executed arms with equal EMPTY sets must not pass as MATCHED:
        # a runtime that exits 0 while writing nothing is not evidence of
        # equivalence (issue #1349).
        return "NO_ARTIFACTS", ("both arms ran but produced no artifacts "
                                "-- nothing to compare")
    if a == b:
        return "MATCHED", "artifact sets agree"
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
    engine metadata directories/files."""
    planted = set(PLANTED)
    out = {}
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for fn in filenames:
            if fn.startswith(SKIP_FILE_PREFIXES):
                continue
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


def measure(rel, expect, bin_, tmp, have, timeout, record_sink=None):
    expected = getattr(expect, "EXPECTED", None)
    entry = (expected.get(rel, {}) if isinstance(expected, dict) else {}) or {}
    execution = entry.get("execution") or {}
    src = os.path.join(expect.ROOT, "tests", "fixtures", "import", rel)
    runtime_bin, template = source_runtime(rel, entry)
    if runtime_bin is None:
        return "SKIPPED", "no source runner for extension %s" % os.path.splitext(rel)[1]
    # Read the imported workflow's environment block via the module-level
    # tomllib (guarded: main() refuses to run below the 3.11 floor).
    imp_dir = os.path.join(tmp, rel.replace("/", "_"))
    os.makedirs(imp_dir, exist_ok=True)
    imported_wf = os.path.join(imp_dir, "W.oxoflow")
    p = subprocess.run([bin_, "import", src, "-o", imported_wf],
                       capture_output=True, text=True)
    if p.returncode != 0:
        return "SKIPPED", "import failed: %s" % error_line(p.stderr)
    with open(imported_wf, "rb") as fh:
        imported_doc = tomllib.load(fh)
    mismatch = container_mismatch(imported_doc, execution)
    if mismatch:
        return "SKIPPED", mismatch
    backends = required_backends(imported_doc)
    source_backend = execution.get("source_backend")
    if source_backend:
        backends.add(source_backend)
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
    source_argv = [a.format(src=src_seed) for a in template]
    imported_argv = [bin_, "run", IMPORTED_SEED, "-d", run_dir]
    arm_timeout = int(execution.get("timeout", timeout))
    s_ran, s_reason, s_art = run_arm(source_argv, src_dir, arm_timeout)
    i_ran, i_reason, i_art = run_arm(imported_argv, run_dir, arm_timeout)
    for art, seed in ((s_art, src_seed), (i_art, IMPORTED_SEED)):
        art.pop(seed, None)
    # The comparison endpoint is the complete produced-artifact set, so an
    # incidental file can never be the only agreement. When the imported
    # workflow declares literal outputs, each must also be present in BOTH
    # arms; wildcard outputs are not expanded here and are skipped from this
    # membership check (the artifact-set comparison still covers whatever the
    # arms produced).
    declared = declared_literal_outputs(imported_doc)
    if s_ran and declared:
        missing = missing_declared_outputs(s_art, declared)
        if missing:
            return "SKIPPED", ("source arm: declared output(s) missing: "
                               + ", ".join(missing[:4]))
    if i_ran and declared:
        missing = missing_declared_outputs(i_art, declared)
        if missing:
            return "SKIPPED", ("imported arm: declared output(s) missing: "
                               + ", ".join(missing[:4]))
    verdict, detail = classify({
        "source": {"ran": s_ran, "reason": "source arm: " + s_reason, "artifacts": s_art},
        "imported": {"ran": i_ran, "reason": "imported arm: " + i_reason, "artifacts": i_art},
    })
    if record_sink is not None:
        record_sink.append({
            "fixture": rel,
            "verdict": verdict,
            "reason": detail,
            "source": {"command": source_argv, "ran": s_ran,
                       "reason": s_reason, "artifacts": s_art},
            "imported": {"command": imported_argv, "ran": i_ran,
                         "reason": i_reason, "artifacts": i_art},
            "execution": execution,
            "declared_literal_outputs": declared,
        })
    return verdict, detail


def main(argv):
    if tomllib is None:
        # Issue #1349: on an interpreter below the 3.11 floor the old code
        # died mid-run with a traceback that report-only mode swallowed into
        # a silent-green empty result. Fail loudly instead, before any row
        # is printed.
        print(
            "ERROR: import-execution-equivalence requires Python >= 3.11 "
            "(tomllib); this interpreter is Python %s. Without tomllib the "
            "imported workflow's environment block cannot be read, so no "
            "fixture can execute -- refusing to emit an empty green tally. "
            "Re-run under python3.11+." % sys.version.split()[0],
            file=sys.stderr,
        )
        return 2
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
    tally = {"EXECUTED": 0, "SKIPPED": 0, "MATCHED": 0, "DRIFTED": 0,
             "NO_ARTIFACTS": 0}
    rows = []
    records = []
    for rel in supported_fixtures(expect):
        verdict, reason = measure(rel, expect, bin_, tmp, have, timeout, records)
        if verdict == "SKIPPED":
            tally["SKIPPED"] += 1
        else:
            # MATCHED / DRIFTED / NO_ARTIFACTS: both arms genuinely ran.
            tally["EXECUTED"] += 1
            tally[verdict] += 1
        rows.append((rel, verdict, reason))
        print("RESULT\t%s\t%s\t%s" % (rel, verdict, reason))
    print()
    print("EXECUTION-EQUIVALENCE TALLY (fixtures=%d)" % len(rows))
    print("  executed    : %d" % tally["EXECUTED"])
    print("  skipped     : %d" % tally["SKIPPED"])
    print("  matched     : %d" % tally["MATCHED"])
    print("  drifted     : %d" % tally["DRIFTED"])
    print("  no_artifacts: %d" % tally["NO_ARTIFACTS"])
    print()
    print(json.dumps(tally))
    records_path = os.environ.get("EQ_RECORDS_PATH")
    if records_path:
        # Durable raw evidence: per-arm command, exit state, captured reason and
        # the full artifact SHA-256 map. Sorted keys keep the file stable.
        with open(records_path, "w") as fh:
            json.dump({"fixtures": records, "tally": tally}, fh,
                      indent=2, sort_keys=True)
        print("RECORDS\t%s" % records_path)
    # Exit non-zero iff an executed fixture drifted, so the wrapper --strict
    # branch is falsifiable; an all-SKIPPED (or all-MATCHED, or all
    # NO_ARTIFACTS) run stays 0.
    return 1 if tally["DRIFTED"] else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
