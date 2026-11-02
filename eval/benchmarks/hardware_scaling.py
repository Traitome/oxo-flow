#!/usr/bin/env python3
"""Hardware-scaling model projections and measured-benchmark ingestion.

The built-in model projects three co-design regimes. Projections are not
measurements and are generated only with ``--allow-simulation``. Measured data
must be supplied through ``--measurements-json`` using raw per-run telemetry so
confidence intervals can be computed from run IDs and seeds.

No output from the projection path may be described as empirical evidence.

Usage:
  python3 eval/benchmarks/hardware_scaling.py --allow-simulation [--cohort-size 1000]
  python3 eval/benchmarks/hardware_scaling.py --measurements-json measurements.json
"""

from __future__ import annotations

import argparse
import json
import math
import os
import platform
import random
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple


MODELLED_TARGET = (
    "Dual-socket AMD EPYC 9654 model (128 physical Zen4 cores, "
    "256 threads, 768 MB L3 cache, 12-channel DDR5-4800, "
    "four PCIe 5.0 NVMe scratch devices)"
)


@dataclass
class HardwareBenchmarkResult:
    config_name: str
    description: str
    cohort_size: int
    wall_time_hours: float
    samples_per_hour: float
    l3_cache_miss_rate_pct: float
    remote_numa_access_pct: float
    interconnect_bandwidth_gbps: float
    io_wait_time_pct: float
    peak_memory_gb: float
    energy_kwh: float
    n_runs: int = 1
    wall_time_ci_hours: Optional[Tuple[float, float]] = None
    throughput_ci_samples_per_hour: Optional[Tuple[float, float]] = None
    run_ids: Optional[List[str]] = None
    seeds: Optional[List[int]] = None


def detect_system_topology() -> Dict[str, Any]:
    """Detects physical CPU sockets, NUMA nodes, and memory architecture."""
    os_name = platform.system()
    nodes = 1
    total_cores = os.cpu_count() or 1
    total_mem_gb = 16.0

    if os_name == "Linux":
        numa_path = Path("/sys/devices/system/node")
        if numa_path.exists():
            nodes = max(1, len(list(numa_path.glob("node[0-9]*"))))
        try:
            mem_info = Path("/proc/meminfo").read_text()
            for line in mem_info.splitlines():
                if line.startswith("MemTotal:"):
                    kb = int(line.split()[1])
                    total_mem_gb = kb / (1024 * 1024)
                    break
        except Exception:
            pass
    elif os_name == "Darwin":
        try:
            res = subprocess.run(["sysctl", "-n", "hw.memsize"], capture_output=True, text=True)
            bytes_val = int(res.stdout.strip())
            total_mem_gb = bytes_val / (1024**3)
        except Exception:
            pass

    return {
        "os": os_name,
        "cores": total_cores,
        "numa_nodes": nodes,
        "total_memory_gb": round(total_mem_gb, 1),
    }


def model_hardware_scaling_projection(
    cohort_size: int = 1000,
) -> List[HardwareBenchmarkResult]:
    """Project three regimes from fixed assumptions, not measured telemetry."""
    # Baseline: Default OS scheduler on monolithic NFS (incumbent Nextflow/Snakemake behavior)
    baseline_time = (cohort_size * 18.5) / 128.0  # hours
    baseline_samples_hr = cohort_size / baseline_time

    res_baseline = HardwareBenchmarkResult(
        config_name="Monolithic NFS + OS Scheduler",
        description="Default Linux CFS scheduler without NUMA affinity; all intermediate I/O directed to shared NFS volume",
        cohort_size=cohort_size,
        wall_time_hours=round(baseline_time, 2),
        samples_per_hour=round(baseline_samples_hr, 2),
        l3_cache_miss_rate_pct=18.4,
        remote_numa_access_pct=48.2,
        interconnect_bandwidth_gbps=112.5,
        io_wait_time_pct=34.8,
        peak_memory_gb=384.0,
        energy_kwh=round(baseline_time * 0.650, 1),
    )

    # Configuration 2: oxo-flow NUMA Locality Pinning on Monolithic NFS
    numa_time = baseline_time * 0.76  # ~24% speedup from eliminating cross-socket memory stalls
    numa_samples_hr = cohort_size / numa_time

    res_numa = HardwareBenchmarkResult(
        config_name="oxo-flow NUMA-Pinned + Monolithic NFS",
        description="Engine dynamically discovers NUMA sockets; memory-bound alignment tasks bound to local socket DDR channels",
        cohort_size=cohort_size,
        wall_time_hours=round(numa_time, 2),
        samples_per_hour=round(numa_samples_hr, 2),
        l3_cache_miss_rate_pct=6.1,
        remote_numa_access_pct=4.3,
        interconnect_bandwidth_gbps=28.7,
        io_wait_time_pct=31.2,
        peak_memory_gb=340.0,
        energy_kwh=round(numa_time * 0.580, 1),
    )

    # Configuration 3: oxo-flow Full Quad-Pillar (NUMA Pinning + 4-Tier Scratch Ladder)
    full_time = baseline_time * 0.47  # ~53% speedup from NVMe scratch + RAM tmpfs FIFO streaming + NUMA pinning
    full_samples_hr = cohort_size / full_time

    res_full = HardwareBenchmarkResult(
        config_name="oxo-flow NUMA-Pinned + 4-Tier NVMe/RAM Scratch",
        description="NUMA core pinning combined with Tier 0 tmpfs FIFO piping and local high-speed NVMe scratch tiering",
        cohort_size=cohort_size,
        wall_time_hours=round(full_time, 2),
        samples_per_hour=round(full_samples_hr, 2),
        l3_cache_miss_rate_pct=5.4,
        remote_numa_access_pct=3.8,
        interconnect_bandwidth_gbps=22.4,
        io_wait_time_pct=7.6,
        peak_memory_gb=285.0,
        energy_kwh=round(full_time * 0.520, 1),
    )

    return [res_baseline, res_numa, res_full]


def _bootstrap_mean_ci(
    data: List[float], seed: int = 20261102, rounds: int = 4000
) -> Tuple[float, float]:
    if len(data) < 2:
        raise ValueError("confidence intervals require at least two measured runs")
    rng = random.Random(seed)
    means = []
    for _ in range(rounds):
        means.append(sum(rng.choice(data) for _ in data) / len(data))
    means.sort()
    lower = means[int(0.025 * rounds)]
    upper = means[min(rounds - 1, int(0.975 * rounds))]
    return lower, upper


def load_measured_benchmark(path: Path) -> Tuple[
    List[HardwareBenchmarkResult], Dict[str, Any], Dict[str, Any]
]:
    """Load raw run-level measurements and aggregate them by configuration.

    Required run fields: ``run_id``, ``config_name``, ``cohort_size``, ``seed``,
    ``wall_time_hours``, ``samples_per_hour``, ``l3_cache_miss_rate_pct``,
    ``remote_numa_access_pct``, ``interconnect_bandwidth_gbps``,
    ``io_wait_time_pct``, ``peak_memory_gb``, and ``energy_kwh``.
    """
    payload = json.loads(path.read_text())
    host_inventory = payload.get("host_inventory")
    target_topology = payload.get("target_topology")
    runs = payload.get("runs")
    if not isinstance(host_inventory, dict) or not isinstance(target_topology, dict):
        raise ValueError("measured JSON requires host_inventory and target_topology objects")
    if not isinstance(runs, list) or not runs:
        raise ValueError("measured JSON requires a non-empty runs array")

    numeric_fields = [
        "cohort_size",
        "wall_time_hours",
        "samples_per_hour",
        "l3_cache_miss_rate_pct",
        "remote_numa_access_pct",
        "interconnect_bandwidth_gbps",
        "io_wait_time_pct",
        "peak_memory_gb",
        "energy_kwh",
    ]
    grouped: Dict[str, List[Dict[str, Any]]] = {}
    seen_run_ids: set[str] = set()
    for run in runs:
        if not isinstance(run, dict):
            raise ValueError("every measured run must be an object")
        run_id = run.get("run_id")
        if not isinstance(run_id, str) or not run_id:
            raise ValueError("every measured run requires a non-empty run_id")
        if run_id in seen_run_ids:
            raise ValueError(f"duplicate measured run_id: {run_id}")
        seen_run_ids.add(run_id)
        if not isinstance(run.get("seed"), int):
            raise ValueError(f"run {run_id} requires an integer seed")
        config_name = run.get("config_name")
        if not isinstance(config_name, str) or not config_name:
            raise ValueError(f"run {run_id} requires config_name")
        for field in numeric_fields:
            value = run.get(field)
            if not isinstance(value, (int, float)) or value < 0:
                raise ValueError(f"run {run_id} has invalid {field}: {value!r}")
        grouped.setdefault(config_name, []).append(run)

    aggregated: List[HardwareBenchmarkResult] = []
    for config_name, config_runs in sorted(grouped.items()):
        if len(config_runs) < 2:
            raise ValueError(
                f"configuration '{config_name}' needs at least two measured runs"
            )
        means = {
            field: sum(float(run[field]) for run in config_runs) / len(config_runs)
            for field in numeric_fields
        }
        wall_ci = _bootstrap_mean_ci(
            [float(run["wall_time_hours"]) for run in config_runs]
        )
        throughput_ci = _bootstrap_mean_ci(
            [float(run["samples_per_hour"]) for run in config_runs]
        )
        aggregated.append(
            HardwareBenchmarkResult(
                config_name=config_name,
                description=str(config_runs[0].get("description", "")),
                cohort_size=int(means["cohort_size"]),
                wall_time_hours=means["wall_time_hours"],
                samples_per_hour=means["samples_per_hour"],
                l3_cache_miss_rate_pct=means["l3_cache_miss_rate_pct"],
                remote_numa_access_pct=means["remote_numa_access_pct"],
                interconnect_bandwidth_gbps=means["interconnect_bandwidth_gbps"],
                io_wait_time_pct=means["io_wait_time_pct"],
                peak_memory_gb=means["peak_memory_gb"],
                energy_kwh=means["energy_kwh"],
                n_runs=len(config_runs),
                wall_time_ci_hours=wall_ci,
                throughput_ci_samples_per_hour=throughput_ci,
                run_ids=sorted(str(run["run_id"]) for run in config_runs),
                seeds=sorted(int(run["seed"]) for run in config_runs),
            )
        )
    if len(aggregated) < 2:
        raise ValueError("measured benchmark requires at least two configurations")
    return aggregated, host_inventory, target_topology


def generate_svg_chart(
    results: List[HardwareBenchmarkResult], evidence_mode: str
) -> str:
    """Generate a standalone vector chart with an explicit evidence label."""
    width = 720
    height = 420
    pad_left = 220
    pad_right = 50
    pad_top = 70
    pad_bottom = 70

    plot_width = width - pad_left - pad_right
    bar_height = 45
    gap = 45

    max_val = max(r.samples_per_hour for r in results) * 1.25
    base_throughput = results[0].samples_per_hour if results and results[0].samples_per_hour else 1.0
    best_throughput = max(r.samples_per_hour for r in results) if results else 0.0
    gain_pct = (best_throughput / base_throughput - 1.0) * 100.0

    svg = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" '
        f'width="{width}" height="{height}" style="background-color: #0f172a; font-family: ui-sans-serif, system-ui, sans-serif;">',
        f'<text x="{width/2}" y="35" text-anchor="middle" font-size="18" font-weight="700" fill="#f8fafc">'
        f'{"Measured" if evidence_mode == "measured" else "Model Projection (Not Measurement)"}: '
        f'Hardware Scaling</text>',
        f'<text x="{width/2}" y="55" text-anchor="middle" font-size="12" fill="#94a3b8">'
        "Genomic Pipeline Throughput (Samples per Hour; Higher is Better)</text>",
    ]

    colors = ["#64748b", "#06b6d4", "#10b981"]

    for idx, r in enumerate(results):
        y = pad_top + idx * (bar_height + gap)
        bar_w = (r.samples_per_hour / max_val) * plot_width
        col = colors[idx % len(colors)]

        svg.append(f'<text x="{pad_left - 15}" y="{y + bar_height/2 + 5}" text-anchor="end" font-size="12" font-weight="600" fill="#e2e8f0">{r.config_name}</text>')
        svg.append(f'<rect x="{pad_left}" y="{y}" width="{bar_w}" height="{bar_height}" rx="6" fill="{col}" opacity="0.9" />')
        svg.append(f'<text x="{pad_left + bar_w + 12}" y="{y + bar_height/2 + 5}" font-size="13" font-weight="700" fill="{col}">{r.samples_per_hour:.1f} smp/hr ({r.wall_time_hours:.1f}h total)</text>')

    # Axis and footer notes
    svg.append(f'<line x1="{pad_left}" y1="{pad_top}" x2="{pad_left}" y2="{height - pad_bottom}" stroke="#334155" stroke-width="2" />')
    suffix = (
        "Measured run-level aggregation."
        if evidence_mode == "measured"
        else "Deterministic model projection, not hardware measurement."
    )
    svg.append(
        f'<text x="{width/2}" y="{height - 25}" text-anchor="middle" '
        f'font-size="11" fill="#64748b">{suffix} '
        f"Projected throughput difference: {gain_pct:+.1f}%.</text>"
    )
    svg.append('</svg>')

    return "\n".join(svg)


def generate_markdown_report(
    results: List[HardwareBenchmarkResult],
    host_inventory: Dict[str, Any],
    target_topology: Dict[str, Any],
    evidence_mode: str,
) -> str:
    """Render projections or measurements without changing their evidence class."""
    simulated = evidence_mode != "measured"
    md = [
        (
            "# Hardware Scaling Model Projection (Not Measurement)"
            if simulated
            else "# Measured Hardware Scaling Benchmark"
        ),
        "",
    ]
    if simulated:
        md.extend([
            "> **MODEL PROJECTION.** Values below come from fixed assumptions in",
            "> `model_hardware_scaling_projection`; they are not experimental data.",
            "",
        ])
    md.extend([
        "## 1. Host and Target Topology",
        "",
        f"- **Execution host inventory**: {json.dumps(host_inventory, sort_keys=True)}.",
        f"- **Target topology**: {json.dumps(target_topology, sort_keys=True)}.",
        f"- **Evidence mode**: `{evidence_mode}`.",
        f"- **Cohort Dimension**: $N = {results[0].cohort_size:,}$ whole-genome sequencing (WGS) variant calling pipelines.",
        "",
        (
            "## 2. Projected Performance"
            if simulated
            else "## 2. Measured Performance"
        ),
        "",
        "| Architecture Configuration | Runs | Wall Clock (Hours) | Wall-Time 95% CI | Throughput (Samples/Hr) | Throughput 95% CI | Speedup | L3 Miss (%) | Remote NUMA (%) | I/O Wait (%) | Energy (kWh) |",
        "| :--- | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: |",
    ])

    base_time = results[0].wall_time_hours
    for r in results:
        speedup = f"{base_time / r.wall_time_hours:.2f}x" if r.wall_time_hours > 0 else "1.00x"
        wall_ci = (
            f"[{r.wall_time_ci_hours[0]:.2f}, {r.wall_time_ci_hours[1]:.2f}]"
            if r.wall_time_ci_hours
            else "n/a"
        )
        throughput_ci = (
            f"[{r.throughput_ci_samples_per_hour[0]:.1f}, "
            f"{r.throughput_ci_samples_per_hour[1]:.1f}]"
            if r.throughput_ci_samples_per_hour
            else "n/a"
        )
        md.append(
            f"| **{r.config_name}** | {r.n_runs} | {r.wall_time_hours:.2f} | {wall_ci} | "
            f"**{r.samples_per_hour:.2f}** | {throughput_ci} | **{speedup}** | "
            f"{r.l3_cache_miss_rate_pct:.1f}% | {r.remote_numa_access_pct:.1f}% | "
            f"{r.io_wait_time_pct:.1f}% | {r.energy_kwh:.1f} |"
        )

    md.extend([
        "",
        "## 3. Interpretation",
        "",
        (
            "- The table is a deterministic projection under stated assumptions, "
            "not a measurement of oxo-flow on the target topology."
            if simulated
            else "- Results aggregate raw run IDs and seeds by configuration; "
            "confidence intervals are percentile bootstrap intervals over runs."
        ),
        "- Cross-host deployment evidence remains required before making a general performance claim.",
        "- Projected or measured speedup does not by itself establish scientific correctness.",
    ])

    return "\n".join(md)


def main() -> None:
    parser = argparse.ArgumentParser(description="Hardware-aware scaling benchmark harness")
    parser.add_argument("--cohort-size", type=int, default=10000, help="Cohort size for simulated scaling")
    parser.add_argument(
        "--allow-simulation",
        action="store_true",
        help="Explicitly allow deterministic model projections (never empirical evidence)",
    )
    parser.add_argument(
        "--measurements-json",
        default=None,
        help="Raw measured run-level telemetry JSON",
    )
    parser.add_argument("--out-json", default=None, help="Export path for JSON benchmark results")
    parser.add_argument("--out-md", default=None, help="Export path for Markdown report")
    parser.add_argument("--out-svg", default=None, help="Export path for SVG vector visualization")
    args = parser.parse_args()

    if bool(args.allow_simulation) == bool(args.measurements_json):
        parser.error("choose exactly one of --allow-simulation or --measurements-json")

    host_inventory = detect_system_topology()
    if args.measurements_json:
        results, supplied_host, target_topology = load_measured_benchmark(
            Path(args.measurements_json)
        )
        host_inventory = supplied_host
        evidence_mode = "measured"
    else:
        results = model_hardware_scaling_projection(cohort_size=args.cohort_size)
        target_topology = {
            "description": MODELLED_TARGET,
            "provenance": "fixed model assumption, not inventory",
        }
        evidence_mode = "model_projection"

    print("=" * 80)
    print(
        "oxo-flow Measured Hardware Scaling Benchmark"
        if evidence_mode == "measured"
        else "oxo-flow Hardware Scaling Model Projection (NOT MEASUREMENT)"
    )
    print(f"Evidence mode: {evidence_mode}")
    print(f"Cohort: {results[0].cohort_size:,} Genomes")
    print("=" * 80)

    for r in results:
        speedup = results[0].wall_time_hours / r.wall_time_hours
        print(
            f"{r.config_name:<40} | Time: {r.wall_time_hours:6.1f}h | "
            f"Throughput: {r.samples_per_hour:6.2f} smp/h | Speedup: {speedup:4.2f}x | "
            f"I/O Wait: {r.io_wait_time_pct:4.1f}%"
        )
    print("=" * 80)

    if args.out_json:
        out_j = Path(args.out_json)
        out_j.parent.mkdir(parents=True, exist_ok=True)
        data = {
            "evidence_mode": evidence_mode,
            "inferential": evidence_mode == "measured",
            "host_inventory": host_inventory,
            "target_topology": target_topology,
            "results": [r.__dict__ for r in results],
            "interpretation_warning": (
                None
                if evidence_mode == "measured"
                else "Deterministic model projection; not empirical evidence."
            ),
        }
        out_j.write_text(json.dumps(data, indent=2))
        print(f"Hardware scaling JSON exported to: {out_j}")

    if args.out_md:
        out_m = Path(args.out_md)
        out_m.parent.mkdir(parents=True, exist_ok=True)
        out_m.write_text(
            generate_markdown_report(
                results, host_inventory, target_topology, evidence_mode
            )
        )
        print(f"Hardware scaling Markdown report exported to: {out_m}")

    if args.out_svg:
        out_s = Path(args.out_svg)
        out_s.parent.mkdir(parents=True, exist_ok=True)
        out_s.write_text(generate_svg_chart(results, evidence_mode))
        print(f"Hardware scaling SVG chart exported to: {out_s}")


if __name__ == "__main__":
    main()
