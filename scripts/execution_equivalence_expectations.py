#!/usr/bin/env python3
"""Independent truth for the execution-equivalence fixtures (#1304).

Companion to scripts/import_equivalence_expectations.py. That module measures
the *declared* contract of the import transpiler; this one measures *execution*
equivalence: the same intent, written natively in three engines (oxo-flow,
Nextflow DSL2, Snakemake), run on the same host over the same inputs, must
produce the same declared-output content.

Every fixture below reuses a fixture that already has import-corpus coverage
(see `import_reference`): the selector list is READ FROM
scripts/import_equivalence_expectations.py (status == "supported") rather than
hand-copied, so a rename there is caught here.

`declared_outputs` is the contract each engine arm must satisfy at the SAME
relative paths: the harness hashes the CONTENT (SHA-256) of exactly these
paths, never file presence. Inputs live in the fixture directory itself and
are copied into every arm unchanged, so identical bytes cancel out of the
comparison.

`container` is the pinned image digest. It is identical across all three arms
and all three fixtures: a differing digest would be a protocol deviation, not
an equivalence observation (PR-03 C3).
"""

import importlib.util
import os

HERE = os.path.dirname(os.path.abspath(__file__))

# One pinned container for every arm of every fixture (PR-03 C3: identical
# across arms). python:3.12-slim carries /bin/bash (Nextflow's container shim
# needs it) and the POSIX tools the fixtures use.
CONTAINER = "python@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b"

# Engine entrypoint filenames inside each fixture directory.
ENTRY = {
    "oxo-flow": "workflow.oxoflow",
    "nextflow": "main.nf",
    "snakemake": "Snakefile",
}

ARMS = ["oxo-flow", "nextflow", "snakemake"]

FIXTURES = [
    {
        "name": "rnaseq_chain",
        "intent": "trim -> per-sample read count -> fan-in summary (three-sample chain)",
        "import_reference": ["nextflow/rnaseq.ok.nf", "snakemake/rnaseq.ok.smk"],
        "declared_outputs": [
            "trimmed/s1.fastq", "trimmed/s2.fastq", "trimmed/s3.fastq",
            "counts/s1.tsv", "counts/s2.tsv", "counts/s3.tsv",
            "report/summary.tsv",
        ],
    },
    {
        "name": "qc_stats",
        "intent": "per-sample QC statistics, one artifact per sample (single stage)",
        "import_reference": ["nextflow/nfcore_module.ok.nf"],
        "declared_outputs": [
            "qc/s1.stats.txt", "qc/s2.stats.txt", "qc/s3.stats.txt",
        ],
    },
    {
        "name": "expand_filter",
        "intent": "expand over samples -> per-sample score -> fan-in threshold filter",
        "import_reference": ["snakemake/expand.ok.smk"],
        "declared_outputs": [
            "quant/s1.sf", "quant/s2.sf", "quant/s3.sf",
            "results/kept.tsv",
        ],
    },
]


def load_import_expectations():
    """The import-corpus truth table, loaded (not retyped)."""
    spec = importlib.util.spec_from_file_location(
        "import_expect",
        os.path.join(HERE, "import_equivalence_expectations.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def supported_import_fixtures():
    """status == 'supported' selectors from the import truth table."""
    mod = load_import_expectations()
    return sorted(rel for rel, e in mod.EXPECTED.items()
                  if e.get("status") == "supported")


def fixture_names():
    return [f["name"] for f in FIXTURES]


def get(name):
    for f in FIXTURES:
        if f["name"] == name:
            return f
    raise KeyError(name)


def unknown_import_refs():
    """Fixture import_reference entries not present in the import truth table.

    Non-empty means this file and the import corpus have drifted apart."""
    supported = set(supported_import_fixtures())
    bad = []
    for f in FIXTURES:
        for ref in f["import_reference"]:
            if ref not in supported:
                bad.append((f["name"], ref))
    return bad
