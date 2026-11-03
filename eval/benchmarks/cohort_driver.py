#!/usr/bin/env python3
"""Cohort DAG-expansion latency benchmark driver for oxo-flow.

Generates reproducible synthetic cohorts (N=10 to N=10,000 samples), renders a
static three-rule `.oxoflow` workflow (align_bwa -> qc_check -> aggregate_cohort)
fanning out over a [[values]] list, and measures how long `oxo-flow dry-run
--json` takes to expand and plan the DAG at each cohort size.

What is actually measured:
1. `dry-run --json` wall-clock latency per cohort size (no pipeline execution).
2. The length of the emitted `plan` array (no node count is inferred).
3. Derived throughput in expanded plan nodes per second.

Not measured: engine execution, environment resolution, or any re-sequencing /
re-entry behavior — the rendered workflow is static.

Usage:
  python3 eval/benchmarks/cohort_driver.py [--samples 100] [--out-dir cohorts/] [--run-dry]
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import random
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "scripts"))
import common  # noqa: E402


class BenchmarkError(RuntimeError):
    """Raised when a measured benchmark field cannot be trusted."""


@dataclass
class CohortSample:
    sample_id: str
    patient_id: str
    tissue_type: str  # "tumor" | "normal" | "blood"
    fastq_r1: str
    fastq_r2: str
    read_depth: float
    library_prep: str


def generate_cohort(num_samples: int, seed: int = 42) -> List[CohortSample]:
    """Generates synthetic cohort metadata adhering to clinical genomics standards."""
    rng = random.Random(seed)
    cohort = []
    for i in range(1, num_samples + 1):
        pid = f"PATIENT_{((i - 1) // 2) + 1:05d}"
        tissue = "tumor" if i % 2 == 1 else "normal"
        sid = f"SAMPLE_{i:06d}_{tissue.upper()}"
        depth = rng.gauss(42.0, 6.0)
        cohort.append(
            CohortSample(
                sample_id=sid,
                patient_id=pid,
                tissue_type=tissue,
                fastq_r1=f"data/{sid}_R1.fastq.gz",
                fastq_r2=f"data/{sid}_R2.fastq.gz",
                read_depth=round(max(15.0, depth), 2),
                library_prep="TruSeq_Stranded_mRNA" if i % 3 == 0 else "Illumina_DNA_PCR_Free",
            )
        )
    return cohort


def export_samplesheet(cohort: List[CohortSample], output_path: Path) -> None:
    """Writes standard TSV samplesheet for oxo-flow input ingestion."""
    output_path.parent.mkdir(parents=True, exist_ok=True)
    with output_path.open("w", newline="") as f:
        writer = csv.writer(f, delimiter="\t")
        writer.writerow(["sample", "patient", "tissue", "r1", "r2", "depth", "library"])
        for s in cohort:
            writer.writerow([s.sample_id, s.patient_id, s.tissue_type, s.fastq_r1, s.fastq_r2, s.read_depth, s.library_prep])


def render_cohort_workflow(num_samples: int, cohort: List[CohortSample]) -> str:
    """Renders a static three-rule .oxoflow workflow fanning out over the cohort."""
    sample_ids = [s.sample_id for s in cohort]
    formatted_samples = ", ".join(f'"{sid}"' for sid in sample_ids)

    return f"""[workflow]
name = "cohort_scale_{num_samples}"
version = "1.0.0"
description = "Large-scale cohort processing benchmark workflow"

[[values]]
name = "cohort_sample"
values = [{formatted_samples}]

[types.bam_coord]
format = "bam"
sort_order = "coordinate"
index = "bai"

[[rules]]
name = "align_bwa"
output = ["aligned/{{cohort_sample}}.bam"]
shell = "echo 'bwa mem -R @RG ref.fa {{cohort_sample}}_R1.fq {{cohort_sample}}_R2.fq' > aligned/{{cohort_sample}}.bam"
[rules.types]
outputs = {{ default = "bam_coord" }}

[rules.resources]
threads = 4
memory = "8G"

[[rules]]
name = "qc_check"
input = ["aligned/{{cohort_sample}}.bam"]
output = ["qc/{{cohort_sample}}_metrics.json"]
shell = "echo '{{\\"coverage\\": 35.2}}' > qc/{{cohort_sample}}_metrics.json"

[rules.resources]
threads = 1
memory = "2G"

[[rules]]
name = "aggregate_cohort"
input = []
expand_inputs = [{{
    pattern = "qc/{{cohort_sample}}_metrics.json"
}}]
output = ["report/cohort_summary.tsv"]
shell = "cat qc/*_metrics.json > report/cohort_summary.tsv"
"""


def benchmark_cohort_expansion(binary: str, num_samples: int, work_dir: Path) -> Dict[str, Any]:
    """Measures oxo-flow DAG compilation and dry-run preview latency at scale."""
    cohort = generate_cohort(num_samples)
    samplesheet_file = work_dir / "samplesheet.tsv"
    export_samplesheet(cohort, samplesheet_file)

    wf_file = work_dir / "cohort.oxoflow"
    wf_file.write_text(render_cohort_workflow(num_samples, cohort))

    t0 = time.perf_counter()
    res = subprocess.run(
        [binary, "dry-run", "--json", str(wf_file)],
        capture_output=True,
        text=True,
        cwd=work_dir,
    )
    t1 = time.perf_counter()
    latency_ms = (t1 - t0) * 1000.0

    if res.returncode != 0:
        detail = (res.stderr or res.stdout).strip()
        raise BenchmarkError(
            f"dry-run failed for N={num_samples} (exit {res.returncode}): {detail}"
        )
    try:
        j = json.loads(res.stdout)
    except json.JSONDecodeError as exc:
        raise BenchmarkError(
            f"dry-run for N={num_samples} did not emit valid JSON: {exc}"
        ) from exc
    plan = j.get("plan")
    if not isinstance(plan, list):
        raise BenchmarkError(
            f"dry-run for N={num_samples} has no measured 'plan' array"
        )
    dag_size = len(plan)
    if dag_size == 0:
        raise BenchmarkError(
            f"dry-run for N={num_samples} returned an empty plan"
        )
    elapsed_seconds = latency_ms / 1000.0
    throughput = dag_size / elapsed_seconds if elapsed_seconds > 0 else None

    return {
        "num_samples": num_samples,
        "dag_nodes": dag_size,
        "expansion_latency_ms": round(latency_ms, 2),
        "parse_success": True,
        "measurement_valid": True,
        "throughput_nodes_per_sec": round(throughput, 1) if throughput else None,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Cohort DAG-expansion latency benchmark driver")
    parser.add_argument("--samples", type=int, default=100, help="Target cohort size")
    parser.add_argument("--scale-sweep", action="store_true", help="Perform scaling sweep across cohort sizes")
    parser.add_argument("--out-json", default=None, help="Export path for JSON report")
    parser.add_argument("--out-md", default=None, help="Export path for Markdown report")
    args = parser.parse_args()

    binary = common.find_engine_binary()
    sweep_sizes = [10, 50, 100, 250, 500, 1000] if args.scale_sweep else [args.samples]

    print("=" * 80)
    print("oxo-flow Large-Scale Cohort Benchmark Driver")
    print(f"Target Binary: {binary}")
    print(f"Sweep Cohorts: {sweep_sizes}")
    print("=" * 80)

    work_base = Path(tempfile.mkdtemp(prefix="oxoflow_cohort_"))

    results = []
    try:
        for sz in sweep_sizes:
            wdir = work_base / f"cohort_{sz}"
            wdir.mkdir(parents=True, exist_ok=True)
            r = benchmark_cohort_expansion(binary, sz, wdir)
            results.append(r)
            throughput = (
                f"{r['throughput_nodes_per_sec']:8.1f}"
                if r["throughput_nodes_per_sec"] is not None
                else "     n/a"
            )
            print(
                f"Cohort N={sz:5d} | DAG Nodes: {r['dag_nodes']:6d} | "
                f"Expansion Latency: {r['expansion_latency_ms']:7.2f} ms | "
                f"Throughput: {throughput} nodes/sec"
            )
    except BenchmarkError as exc:
        print(f"error: {exc}", file=sys.stderr)
        raise SystemExit(2) from exc
    finally:
        shutil.rmtree(work_base, ignore_errors=True)

    print("=" * 80)

    summary = {
        "benchmark": "cohort_expansion_latency",
        "results": results,
    }

    if args.out_json:
        out_j = Path(args.out_json)
        out_j.parent.mkdir(parents=True, exist_ok=True)
        out_j.write_text(json.dumps(summary, indent=2))
        print(f"Cohort benchmark JSON exported to: {out_j}")

    if args.out_md:
        out_m = Path(args.out_md)
        out_m.parent.mkdir(parents=True, exist_ok=True)
        lines = [
            "# Large-Scale Cohort Expansion & DAG Compilation Benchmark",
            "",
            "Measured from successful `oxo-flow dry-run --json` invocations. "
            "DAG nodes equal the length of the emitted `plan` array; no node "
            "count is inferred from cohort size.",
            "",
            "| Cohort Size ($N$) | Concrete DAG Instances | Expansion Latency (ms) | Compilation Throughput (Nodes/Sec) |",
            "| :---: | :---: | :---: | :---: |",
        ]
        for r in results:
            throughput = (
                f"{r['throughput_nodes_per_sec']:,.1f}"
                if r["throughput_nodes_per_sec"] is not None
                else "n/a"
            )
            lines.append(
                f"| {r['num_samples']} | {r['dag_nodes']:,} | "
                f"{r['expansion_latency_ms']:.2f} ms | {throughput} |"
            )
        out_m.write_text("\n".join(lines) + "\n")
        print(f"Cohort benchmark Markdown exported to: {out_m}")


if __name__ == "__main__":
    main()
