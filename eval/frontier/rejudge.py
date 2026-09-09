#!/usr/bin/env python3
"""Re-judge existing benchmark artifacts without re-calling the provider.

For every run directory under <results_dir>: locate the generated
*.oxoflow (wherever the variant left it), canonicalize it to
workflow.oxoflow, re-run the three static gates, and patch result.json.
Use after engine changes to re-score the same generations, or to repair
runs whose gate step missed a nested artifact.

    python3 rejudge.py results/ab-16384 [--binary /path/to/oxo-flow]
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from run_eval import find_binary, run_gates  # noqa: E402


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("results_dir", type=Path)
    ap.add_argument("--binary", default=None)
    args = ap.parse_args()

    binary = args.binary or find_binary()
    run_dirs = sorted(p for p in args.results_dir.iterdir() if p.is_dir())
    print(f"re-judging {len(run_dirs)} run(s) in {args.results_dir} with {binary}")

    for run_dir in run_dirs:
        result_file = run_dir / "result.json"
        if not result_file.exists():
            print(f"  skip {run_dir.name}: no result.json")
            continue
        result = json.loads(result_file.read_text())

        wf = run_dir / "workflow.oxoflow"
        if not wf.exists():
            candidates = [p for p in sorted(run_dir.rglob("*.oxoflow"))
                          if p != wf]
            if candidates:
                wf.write_text(candidates[0].read_text())
        if not wf.exists():
            print(f"  {run_dir.name}: no artifact — keeping generated={result['generated']}")
            continue

        gates = run_gates(binary, wf, run_dir)
        result["generated"] = True
        result["gates"] = gates
        result["all_gates_pass"] = all(g["pass"] for g in gates.values())
        result["workflow_file"] = str(wf)
        result_file.write_text(json.dumps(result, indent=2))
        verdict = "PASS" if result["all_gates_pass"] else "FAIL"
        detail = " ".join(f"{g}={'ok' if v['pass'] else v['exit']}"
                          for g, v in gates.items())
        print(f"  {run_dir.name}: {verdict} ({detail})")

    print("done — re-run the report aggregation in run_eval.py or read result.json files")


if __name__ == "__main__":
    main()
