# `-j` Suggestion Evaluation: Global vs Per-Parallel-Group (issue #361)

Real-hardware evaluation backing the per-parallel-group `-j` suggestion
shipped in PR #369. Everything here is reproducible on any SLURM cluster
with a `bash` shell — no datasets, no external tools. The `.sbatch` files
pin our box's absolute paths (`/data/home/wsx/oxo-bench-360`) — adjust the
`cd` and `--output` lines for your site.

## Question

`oxo-flow run` suggests a `-j` (max concurrent jobs) value. The old formula

```
min(cores / max_threads_per_rule, DAG width).clamp(1, 16)
```

divides by the **global** heaviest rule, so one heavy wave caps every light
wide wave in the DAG. The proposal evaluates each parallel group against its
own heaviest rule and takes the maximum.

## Method

1. `python3 gen_dags.py` generates four wave-structured DAGs (barrier between
   stages). Every rule burns CPU for ~1 s with `{threads}` concurrent busy
   subshells sized to its declared threads — thread over-subscription has
   real makespan cost, so contention is measured, not simulated.
2. `sbatch j361.sbatch` runs each DAG under three strategies:
   - **cur** — the old suggestion (recomputed per DAG),
   - **grp** — the per-group proposal,
   - **unbounded** — `-j 64` (the engine's resource pool self-limits).
   Each run starts from a clean workdir; wall-clock and output count are
   recorded per combination.
3. `sbatch verify-suggest.sbatch` re-checks the dry-run suggestion under a
   full-node allocation and (separately) under a small one — demonstrating
   cgroup-awareness.

## Recorded results (workq node1: Xeon Gold 5218, 64 CPUs, 2026-09-14)

| DAG (waves: width×threads) | old -j | new -j | makespan old / new / unbounded |
|---|---|---|---|
| d1 uniform: 3 × (20×t1) | 16 | 20 | 5.2 s / **3.0 s** / 3.0 s |
| d2 heavy middle: 20×t1 / 2×t16 / 20×t1 | 4 | 20 | 11.0 s / **3.0 s** / 3.0 s |
| d3 heavy first: 2×t16 / 20×t1 / 20×t1 | 4 | 20 | 11.0 s / **3.0 s** / 3.0 s |
| d4 mixed: 20×t1 / 5×t4 / 2×t16 / 20×t1 | 4 | 20 | 13.0 s / **4.0 s** / 4.0 s |

Engine dry-run suggestions reproduced the old formula exactly (16/4/4/4)
before the change and the group formula exactly (20/20/20/20) after it,
under a full-node allocation. Under an 8-CPU SLURM allocation the same DAGs
suggest 8/8/8/8 — the suggestion is cgroup-aware.

## Findings

1. The global formula cost **1.7–3.7× makespan** on heterogeneous DAGs.
2. `clamp(1, 16)` was independently harmful: it truncated even uniform wide
   DAGs on big machines (d1: 5.2 s vs 3.0 s).
3. The group suggestion matches unbounded `-j` everywhere — the resource
   pool already enforces declared threads, so a generous suggestion is safe
   by construction (confirmed: unbounded never beat the group suggestion).

## Limitations

- One machine, one run per cell (wall quantizes to ~1 s via `SECONDS`
  granularity — all reported gaps exceed the noise floor by >2×).
- Wave shapes are synthetic barriers; real DAGs with partial overlap
  between waves should show the same direction (the global formula's cap
  applies whenever any wave is heavy) but unquantified magnitudes.
