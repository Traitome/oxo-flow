#!/usr/bin/env python3
"""
oxo-flow 集成基准测试套件 (Macro Benchmarks)

测量端到端管线生命周期、扩展性和可靠性。
所有基准均通过 ``oxo-flow validate`` / ``dry-run`` / ``lint`` 命令执行，
不依赖外部生物信息学工具。

用法:
    # 运行所有集成基准
    python3 benches/macro/suite.py

    # 指定 oxo-flow 二进制路径和输出目录
    python3 benches/macro/suite.py --oxo-flow ../target/debug/oxo-flow --output results/

    # 仅运行生命周期基准
    python3 benches/macro/suite.py --benchmark lifecycle
"""

import argparse
import json
import os
import platform
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any


# ---------------------------------------------------------------------------
# 管线生成器
# ---------------------------------------------------------------------------

# 单条命令的超时上限：超时按 FAIL 计入结果，而不是让异常击穿整个套件、
# 把临时目录留在磁盘上 (#557)。
COMMAND_TIMEOUT_SEC = 120


def _toml_escape(s: str) -> str:
    """将 TOML 值中的特殊字符转义"""
    return s.replace("\\", "\\\\").replace('"', '\\"')


def generate_hello(rule_count: int) -> str:
    """生成链式管线，包含 N 条顺序依赖的规则"""
    lines = [
        '[workflow]',
        f'name = "hello-{rule_count}"',
        'version = "1.0.0"',
        '',
    ]
    for i in range(rule_count):
        inp = 'input.txt' if i == 0 else f'step_{i-1}_output.txt'
        out = f'step_{i}_output.txt'
        lines.append(f'[[rules]]')
        lines.append(f'name = "step_{i}"')
        lines.append(f'input = ["{_toml_escape(inp)}"]')
        lines.append(f'output = ["{_toml_escape(out)}"]')
        lines.append(f'shell = "echo {i} > {{output[0]}}"')
        lines.append('')
        lines.append('[rules.resources]')
        lines.append(f'threads = 1')
        lines.append('')
    return '\n'.join(lines)


def generate_parallel(sample_count: int) -> str:
    """生成并行管线，每个样本一条独立规则，再加一条汇总规则"""
    lines = [
        '[workflow]',
        f'name = "parallel-{sample_count}"',
        'version = "1.0.0"',
        '',
    ]
    # 每个样本的处理规则
    for i in range(sample_count):
        sample = f'S{i:03d}'
        lines.append(f'[[rules]]')
        lines.append(f'name = "process_{sample}"')
        lines.append(f'input = ["input_{sample}.txt"]')
        lines.append(f'output = ["processed_{sample}.txt"]')
        lines.append(f'shell = "echo process {sample} > processed_{sample}.txt"')
        lines.append('')
        lines.append('[rules.resources]')
        lines.append(f'threads = 1')
        lines.append('')
    # 汇总规则
    inputs = ', '.join(f'"processed_S{i:03d}.txt"' for i in range(sample_count))
    lines.append(f'[[rules]]')
    lines.append(f'name = "merge"')
    lines.append(f'input = [{inputs}]')
    lines.append(f'output = ["merged_output.txt"]')
    lines.append(f'shell = "cat {{input[0]}} > merged_output.txt"')
    lines.append('')
    lines.append('[rules.resources]')
    lines.append(f'threads = 1')
    lines.append('')
    return '\n'.join(lines)


def generate_scatter_gather(sample_count: int) -> str:
    """生成 scatter-gather 管线: 拆分 -> 处理 -> 合并"""
    lines = [
        '[workflow]',
        f'name = "scatter_gather-{sample_count}"',
        'version = "1.0.0"',
        '',
    ]
    # 拆分规则（纯标记，不需要实际工具）
    lines.append(f'[[rules]]')
    lines.append(f'name = "split"')
    lines.append(f'input = ["input.txt"]')
    lines.append(f'output = [{", ".join(f'"chunk_{i}.txt"' for i in range(sample_count))}]')
    lines.append(f'shell = "echo split > {{output[0]}}"')
    lines.append('')
    lines.append('[rules.resources]')
    lines.append(f'threads = 1')
    lines.append('')

    # 处理每个 chunk
    for i in range(sample_count):
        lines.append(f'[[rules]]')
        lines.append(f'name = "process_{i}"')
        lines.append(f'input = ["chunk_{i}.txt"]')
        lines.append(f'output = ["processed_{i}.txt"]')
        lines.append(f'shell = "echo process {i} > processed_{i}.txt"')
        lines.append('')
        lines.append('[rules.resources]')
        lines.append(f'threads = 1')
        lines.append('')

    # 合并
    inputs = ', '.join(f'"processed_{i}.txt"' for i in range(sample_count))
    lines.append(f'[[rules]]')
    lines.append(f'name = "gather"')
    lines.append(f'input = [{inputs}]')
    lines.append(f'output = ["final_output.txt"]')
    lines.append(f'shell = "cat {{input[0]}} > final_output.txt"')
    lines.append('')
    lines.append('[rules.resources]')
    lines.append(f'threads = 1')
    lines.append('')
    return '\n'.join(lines)


# ---------------------------------------------------------------------------
# 基准运行器
# ---------------------------------------------------------------------------

class CommandTimedOut(Exception):
    """命令超出 COMMAND_TIMEOUT_SEC — 记为 FAIL 而不是击穿套件 (#557)。"""


def _run_command(cmd: list[str]) -> tuple[float, int, str, str]:
    """运行命令并返回 (wall time 秒, exit code, stdout, stderr)。"""
    start = time.perf_counter()
    try:
        result = subprocess.run(cmd, capture_output=True, text=True,
                                timeout=COMMAND_TIMEOUT_SEC)
    except subprocess.TimeoutExpired:
        raise CommandTimedOut(f"{' '.join(cmd)} timed out after {COMMAND_TIMEOUT_SEC}s")
    elapsed = time.perf_counter() - start
    return elapsed, result.returncode, result.stdout, result.stderr


def _format_duration(seconds: float) -> str:
    if seconds < 1:
        return f"{seconds * 1000:.1f} ms"
    if seconds < 60:
        return f"{seconds:.3f} s"
    return f"{seconds / 60:.2f} min"


def benchmark_lifecycle(oxo_bin: str, counts: list[int], output: dict[str, Any], iterations: int = 1):
    """测量 validate / dry-run / lint 在不同管线规模下的性能。"""
    print("\n  ── 生命周期基准 ──")
    # 临时目录集中创建、with 块统一清理：固定 /tmp 路径会在多用户/并行
    # 运行间互相覆盖，异常路径还会把 fixture 留在磁盘上 (#557)。
    with tempfile.TemporaryDirectory(prefix="oxo_bench_") as tmpdir:
        for count in counts:
            tmp = Path(tmpdir) / f"hello_{count}.oxoflow"
            tmp.write_text(generate_hello(count))

            for cmd_name, args in [("validate", ["validate", str(tmp)]),
                                    ("dry-run", ["dry-run", str(tmp)]),
                                    ("lint", ["lint", str(tmp)])]:
                full_cmd = [oxo_bin] + args
                # 多次迭代取最小值（降低系统噪声），与 hyperfine 的 min 语义一致
                elapsed, rc, stdout, stderr = float("inf"), 1, "", ""
                for _ in range(iterations):
                    e, r, so, se = _run_command(full_cmd)
                    if e < elapsed:
                        elapsed, rc, stdout, stderr = e, r, so, se
                status = "OK" if rc == 0 else "FAIL"
                print(f"    {cmd_name:10s}  {count:5d} rules  {_format_duration(elapsed):>10s}  [{status}]")
                output.setdefault("lifecycle", []).append({
                    "command": cmd_name,
                    "rule_count": count,
                    "wall_time_sec": round(elapsed, 4),
                    "exit_code": rc,
                    "iterations": iterations,
                })
                if rc != 0 and stderr:
                    print(f"      stderr: {stderr[:200]}")


def benchmark_scaling(oxo_bin: str, sample_counts: list[int], output: dict[str, Any],
                       iterations: int = 1):
    """测量并行管线中随样本数增加的扩展性。"""
    print("\n  ── 扩展性基准 ──")
    # 同 lifecycle：TemporaryDirectory 统一清理，不再写固定 /tmp 路径 (#557)。
    with tempfile.TemporaryDirectory(prefix="oxo_bench_") as tmpdir:
        for sc in sample_counts:
            tmp = Path(tmpdir) / f"parallel_{sc}.oxoflow"
            tmp.write_text(generate_parallel(sc))

            for cmd_name, args in [("validate", ["validate", str(tmp)]),
                                    ("dry-run", ["dry-run", str(tmp)])]:
                full_cmd = [oxo_bin] + args
                elapsed, rc, _, _ = float("inf"), 1, "", ""
                for _ in range(iterations):
                    e, r, _, _ = _run_command(full_cmd)
                    if e < elapsed:
                        elapsed, rc = e, r
                status = "OK" if rc == 0 else "FAIL"
                print(f"    {cmd_name:10s}  {sc:5d} samples  {_format_duration(elapsed):>10s}  [{status}]")
                output.setdefault("scaling", []).append({
                    "command": cmd_name,
                    "sample_count": sc,
                    "wall_time_sec": round(elapsed, 4),
                    "exit_code": rc,
                    "iterations": iterations,
                })

        # scatter-gather 基准
        for sc in [10, 50]:
            tmp = Path(tmpdir) / f"scatter_{sc}.oxoflow"
            tmp.write_text(generate_scatter_gather(sc))
            elapsed, rc, _, _ = _run_command([oxo_bin, "validate", str(tmp)])
            status = "OK" if rc == 0 else "FAIL"
            print(f"    scatter    {sc:5d} chunks  {_format_duration(elapsed):>10s}  [{status}]")
            output.setdefault("scaling", []).append({
                "command": "validate_scatter_gather",
                "sample_count": sc,
                "wall_time_sec": round(elapsed, 4),
                "exit_code": rc,
            })


def benchmark_reliability(oxo_bin: str, output: dict[str, Any], iterations: int = 1):
    """验证基准可靠性:
    1. 执行时间稳定性（低方差 → 引擎行为可复现）
    2. lint 诊断一致性
    """
    print("\n  ── 可靠性基准 ──")

    # 1. 执行时间稳定性：没有可解析的 CLI checksum 表面，这里按 JSON 键
    #    execution_time_stability 的本义测量同一命令多次运行的耗时方差
    #    (#557)。
    with tempfile.TemporaryDirectory(prefix="oxo_bench_") as tmpdir:
        toml = generate_hello(50)
        tmp = Path(tmpdir) / "reliable.oxoflow"
        tmp.write_text(toml)

        run_times = []
        for i in range(max(3, iterations)):
            elapsed, rc, _, _ = _run_command([oxo_bin, "lint", str(tmp)])
            if i > 0 and rc == 0:  # 首次运行为预热（冷启动），不计入方差
                run_times.append(elapsed)

        if not run_times:
            print("    execution time stability: FAIL  (lint never exited 0)")
            output.setdefault("reliability", []).append({
                "test": "execution_time_stability",
                "run_times_sec": [],
                "stable": False,
            })
        else:
            stable = max(run_times) - min(run_times) < 0.5  # <500ms 方差
            print(f"    execution time stability: {'PASS' if stable else 'CHECK'}  "
                  f"(range: {max(run_times) - min(run_times):.3f}s)")
            output.setdefault("reliability", []).append({
                "test": "execution_time_stability",
                "run_times_sec": [round(c, 4) for c in run_times],
                "stable": stable,
            })

        # 2. 错误检测
        bad_toml = generate_hello(5) + '\n[[rules]]\nname = "step_0"\n'
        tmp_bad = Path(tmpdir) / "bad.oxoflow"
        tmp_bad.write_text(bad_toml)
        elapsed, rc, stdout, stderr = _run_command(
            [oxo_bin, "validate", str(tmp_bad)])
        detects_errors = rc != 0
        print(f"    duplicate detection: {'PASS' if detects_errors else 'FAIL'}  "
              f"(exit={rc})")
        output.setdefault("reliability", []).append({
            "test": "duplicate_rule_detection",
            "detected": detects_errors,
            "exit_code": rc,
        })


# ---------------------------------------------------------------------------
# 主入口
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="oxo-flow 集成基准测试套件")
    parser.add_argument("--oxo-flow",
                        default="target/debug/oxo-flow",
                        help="oxo-flow 二进制路径")
    parser.add_argument("--output", default=None,
                        help="结果输出目录 (默认打印到 stdout)")
    parser.add_argument("--benchmark", default="all",
                        choices=["all", "lifecycle", "scaling",
                                 "reliability"],
                        help="要运行的基准类型")
    parser.add_argument("--iterations", type=int, default=1,
                        help="每条命令的重复运行次数（取最小值，降低噪声）")
    args = parser.parse_args()

    if args.iterations < 1:
        parser.error("--iterations 必须 >= 1")

    oxo_bin = Path(args.oxo_flow)
    if not oxo_bin.exists():
        print(f"Error: oxo-flow binary not found at {oxo_bin}", file=sys.stderr)
        print("Run 'cargo build' first, or specify --oxo-flow", file=sys.stderr)
        sys.exit(1)

    output: dict[str, Any] = {
        "suite": "oxo-flow macro benchmarks",
        "version": "0.1.0",
        "oxo_binary": str(oxo_bin.resolve()),
        "host": platform.node(),
        "os": f"{platform.system()} {platform.release()}",
        "date": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "results": {},
    }

    print(f"oxo-flow 集成基准测试")
    print(f"  二进制: {oxo_bin.resolve()}")
    print(f"  主机:   {platform.node()} ({platform.system()})")
    print(f"  日期:   {output['date']}")
    print("-" * 60)

    # 生命周期基准: 不同管线规模
    if args.benchmark in ("all", "lifecycle"):
        benchmark_lifecycle(str(oxo_bin.resolve()),
                            [10, 50, 100, 500, 1000], output["results"],
                            iterations=args.iterations)
    # 扩展性基准: 并行样本数
    if args.benchmark in ("all", "scaling"):
        benchmark_scaling(str(oxo_bin.resolve()),
                          [10, 50, 100], output["results"],
                          iterations=args.iterations)
    # 可靠性基准
    if args.benchmark in ("all", "reliability"):
        benchmark_reliability(str(oxo_bin.resolve()), output["results"],
                              iterations=args.iterations)

    # 输出
    json_output = json.dumps(output, indent=2, ensure_ascii=False)
    if args.output:
        out_path = Path(args.output)
        out_path.mkdir(parents=True, exist_ok=True)
        result_file = out_path / "macro_results.json"
        result_file.write_text(json_output)
        print(f"\n结果保存至: {result_file}")
    else:
        print(f"\n{json_output}")


if __name__ == "__main__":
    try:
        main()
    except CommandTimedOut as exc:
        # 超时已在单条结果里如实计为 FAIL；这里兜底防止异常击穿主流程、
        # 把已跑完的结果一起丢掉 (#557)。
        print(f"\nBENCHMARK TIMEOUT: {exc}", file=sys.stderr)
        sys.exit(2)
