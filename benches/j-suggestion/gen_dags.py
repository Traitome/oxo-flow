#!/usr/bin/env python3
"""Generate synthetic wave-structured DAGs for the -j suggestion evaluation (#361)."""
import os, sys

BUSY = ("end=$((SECONDS+1)); i=0; "
        "while [ $i -lt {threads} ]; do (while [ $SECONDS -lt $end ]; do :; done) & "
        "i=$((i+1)); done; wait; echo done > {output}")

def gen(name, stages):
    """stages: list of (rules_in_stage, threads). Barrier between stages."""
    lines = [f'[workflow]\nname = "{name}"\nversion = "0.1.0"\ndescription = "-j evaluation DAG"\n']
    for si, (n, t) in enumerate(stages):
        for ri in range(n):
            deps = ""
            if si > 0:
                prev = [f'"s{si-1}_{p}.txt"' for p in range(stages[si-1][0])]
                deps = f"input = [{', '.join(prev)}]\n"
            lines.append(f'\n[[rules]]\nname = "s{si}_{ri}"\n{deps}'
                         f'output = ["s{si}_{ri}.txt"]\n'
                         f'shell = "{BUSY.format(threads=t, output="{output[0]}")}"\n\n'
                         f'[rules.resources]\nthreads = {t}\nmemory = "1G"\n')
    os.makedirs(f"dags/{name}", exist_ok=True)
    open(f"dags/{name}/workflow.oxoflow", "w").write("".join(lines))
    widths = [n for n, _ in stages]
    threads = [t for _, t in stages]
    print(f"{name}: stages={list(zip(widths, threads))}")

gen("d1_uniform_light", [(20, 1), (20, 1), (20, 1)])
gen("d2_heavy_middle", [(20, 1), (2, 16), (20, 1)])
gen("d3_heavy_first", [(2, 16), (20, 1), (20, 1)])
gen("d4_mixed", [(20, 1), (5, 4), (2, 16), (20, 1)])
