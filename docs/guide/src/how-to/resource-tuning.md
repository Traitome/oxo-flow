# Resource Tuning Guide

This guide covers best practices for declaring CPU, memory, GPU, and disk resources in oxo-flow workflows.

## Thread Declaration

Match threads to the tool's actual parallelism capability. Oversubscribing wastes memory, undersubscribing wastes time.

| Tool | Recommended Threads | Notes |
|---|---|---|
| **BWA-MEM2** | 12-16 | Saturates ~12-16 cores; more doesn't help |
| **STAR** | 16-32 | Scales well up to available cores |
| **samtools sort** | 4-8 + 2G/thread | Memory-bound: threads × 2GB per thread |
| **samtools index** | 2-4 | Limited parallelism |
| **GATK HaplotypeCaller** | 4-8 | Java parallelism limited |
| **GATK MarkDuplicates** | 1-2 | Mostly single-threaded |
| **fastp** | 8-16 | Good parallelization |
| **FastQC** | 2-4 | Limited parallelism |

```toml
# Example: BWA alignment
[[rules]]
name = "bwa_align"

[rules.resources]
threads = 16
memory = "32G"  # 2× expected input size
```

!!! warning "Engine conventions"
    - Always declare resources in the `[rules.resources]` sub-table. The
      deprecated rule-level `threads`/`memory` shorthand still works, and
      **takes precedence** over the sub-table when both are present.
    - In the sub-table, `threads <= 1` means "unset" — the engine falls
      back to `[defaults].threads`. To give a rule fewer threads than the
      default, declare `threads = 2` or higher. Forcing exactly one thread
      is only possible via the deprecated rule-level shorthand
      `threads = 1` (lint flags it as [W025](../reference/glossary.md#diagnostic-codes)); `resources.threads = 1` is
      always read as "unset".

## Memory Declaration

### Rule of Thumb

| Operation Type | Memory Formula |
|---|---|
| **Alignment** | 2-4 × largest input file size |
| **Variant calling (WGS)** | 32-64G |
| **Variant calling (panel)** | 8-16G |
| **Sorting/indexing** | threads × 2G |
| **Assembly** | 100-200G for large genomes |

### Common Bioinformatics Tools

| Tool | Memory Recommendation |
|---|---|
| **BWA-MEM2** | 32G for human WGS |
| **STAR** | 64G for human genome |
| **GATK HaplotypeCaller** | 32G for WGS, 8G for panels |
| **GATK BaseRecalibrator** | 16G |
| **samtools sort** | threads × 2G per thread |
| **freebayes** | 16G |

```toml
# Example: WGS variant calling
[[rules]]
name = "haplotype_caller"

[rules.resources]
threads = 8
memory = "64G"
```

## GPU Resources

### SLURM GPU Request

```toml
[[rules]]
name = "gpu_training"

[rules.resources]
threads = 8
memory = "64G"

[rules.resources.gpu_spec]
count = 2
model = "A100"
memory_gb = 40
```

Generated SLURM directive: `--gres=gpu:A100:2:40g --mem-per-gpu=40G` (the model string is passed through verbatim)

### Common GPU Tools

| Tool | GPU Memory | Notes |
|---|---|---|
| **ParaBricks** | 40+ GB per GPU | NVIDIA A100 recommended |
| **Clara Parabricks** | 32+ GB | GPU-accelerated variant calling |
| **DeepVariant GPU** | 16+ GB | Faster than CPU version |

### PBS/SGE GPU

GPU syntax varies by site. Use `extra_args` in the `[cluster]` section:

```toml
[rules.resources]
gpu = 2

[cluster]
extra_args = ["-l ngpus=2:type=a100"]  # Site-specific
```

## Resource Hints for Unknown Requirements

When you don't know exact requirements:

```toml
[[rules]]
name = "novel_tool"
shell = "process_large_data.sh"

[rules.resource_hint]
input_size = "large"     # ~100GB input
memory_scale = 2.5       # Need 2.5× input size = 250GB
runtime = "slow"         # reserved: parsed, but not consumed by the scheduler — `lint` flags it with W037
```

Estimated memory: 100GB × 2.5 = 250GB

The `runtime` hint is a reserved scheduler knob — like `io_bound`, it is
parsed but not consumed today, and `lint` flags every use with `W037`.
Only `input_size` and `memory_scale` affect planning; for enforced limits
declare `resources.time_limit` / `resources.memory` / `resources.threads`
instead.

## Resource Budgets

```toml
[resource_budget]
max_threads = 64        # Cap any rule at 64 threads; also caps the local pool
max_memory = "256G"     # Cap any rule at 256GB; also caps the local pool
max_jobs = 10           # Cap concurrently running jobs (clamp; see below)
```

`max_threads`/`max_memory` are **hard per-rule caps on both paths**, and the
budget is versioned with the workflow — a shard launched locally with
`oxo-flow run` inside an allocation honors the same `.oxoflow` file as a
cluster submission (issue #1080).

- **Local** (`oxo-flow run`): a rule whose declared `threads`/`memory`
  exceeds the cap aborts the run **before any rule executes**, naming the
  offender. The cap also bounds the local pool ceiling: at most
  `max_threads` run concurrently and at most `max_memory` is committed.
  A budget is a ceiling, never a capacity claim — a `max_threads = 64`
  written for a cluster node does not raise a 10-CPU laptop's pool; the
  effective value is `min(detected, cap)`.
  A larger `--max-threads`/`--max-memory` CLI value is clamped by the
  budget (the budget is a cap); a tighter CLI value wins; a silent CLI
  inherits the budget, and the run prints a one-line notice naming the
  active cap.
- **Cluster** (`oxo-flow run --profile <name>`): a rule whose declared
  `memory`/`threads` exceeds the cap aborts the run **before anything is
  submitted**, naming every offender:

  ```
  Error: [resource_budget] exceeded by 1 rule — nothing was submitted:
    rule 'big_mem' requests 64G but [resource_budget] max_memory = 32G
  ```

- `oxo-flow cluster submit` generates scripts without the budget check —
  only the `run --profile` cluster path enforces it there.
- An unreadable or zero cap is an error, never "no cap": `max_memory =
  "lots"` and `max_threads = 0` abort with a message. `dry-run` resolves
  the same caps, prints them in the summary, and uses the thread cap for
  its `-j` suggestion.

Local fast-fail example:

```
Error: resource budget too small for 1 rule; no rules were run:
  - rule 'big_mem' requires 1024MB memory but the run's memory cap is 16MB (--max-memory / [resource_budget] max_memory)
```

`max_jobs` is a run-wide concurrency cap and **is enforced on both paths**:

- **Local** (`oxo-flow run`): it clamps the requested `-j/--jobs`
  concurrency. Asking for `-j 16` under `max_jobs = 4` runs at most 4 jobs
  at once and prints a warning saying so — it never errors, since a job
  count is a total, not a per-rule request.
- **Cluster** (`oxo-flow run --profile <name>`): it clamps the resolved
  in-flight submission cap (`--max-submitted` > profile value > driver
  default) the same way, with the same warning style.

The per-rule sentence above applies only to `max_threads`/`max_memory`: a
cap on jobs is a total, so there is no "single rule exceeds it" condition —
the budget throttles how much runs at once instead.

### Cost budgets (`max_cost`)

`max_threads`, `max_memory`, and `max_jobs` all cap a rule's *capability* —
how many threads, how much memory, how many jobs at once. The optional
`max_cost` table caps *spend* instead: it stops the engine from scheduling
**new** ready rules once the run's accrued cost reaches the declared amount
(issue #1400). It is built on the normalized resource ledger (issue #1371)
using **user-declared rates** — never vendor billing, and never a price
catalog.

```toml
[resource_budget]
max_cost = { amount = 10.0, currency = "USD", cpu_second_rate = 0.0001, gb_second_rate = 0.00002 }
```

- `amount` (number, required, > 0) — the cap, in `currency`.
- `currency` (string, required, non-empty) — a label; compared for
  **equality only**, with no FX conversion.
- `cpu_second_rate` (number, optional) — cost per accrued CPU-second.
- `gb_second_rate` (number, optional) — cost per accrued GB-second
  (`max_rss_bytes / 1e9 * wall_time_secs`, summed per rule).
- `on_unknown` (string, optional, default `"refuse"`) — behavior when
  accrual cannot be computed: `"refuse"` (fail-closed) or `"report"`.

At least one rate must be present and strictly positive; a rate of `0` is
allowed alongside a positive one (that metric contributes `0`), while a
missing rate means that metric is not priced. A budget whose only rates are
`0` is rejected — it would accrue a constant `0` and never trip.
Cost is accrued over the rules that actually executed in this invocation:

```text
accrued = cpu_second_rate * sum(rule.cpu_seconds)
        + gb_second_rate   * sum(max_rss_bytes/1e9 * wall_time_secs)
```

**Unknown is never zero.** If a configured rate depends on a metric that any
executed rule did not report, `accrued` is `null` (never `0`) and the
offending rule names are listed. A budgeted run then either refuses
(`on_unknown = "refuse"`, the fail-closed default) or keeps running and
reports the unbounded residual (`"report"`). Under `"report"` the cap does
**not** bind while accrual is unknown — dispatch continues, and the unknown
residual is surfaced in the ledger and run summary rather than enforced. A
currency mismatch between the budget and a rule's own reported cost is
**always** fail-closed.

- **Local** (`oxo-flow run`): enforcement happens at the scheduler dispatch
  gate — the same place the disk-pressure guard withholds new work. When
  `accrued >= limit` (or accrual is unknown under `refuse`), no new ready
  rule is dispatched. In-flight rules run to completion; they are **not**
  killed or rolled back. Once nothing is in flight and no new work may
  dispatch, the run aborts observably with the structured run status
  `cost_budget_exceeded`, naming the method, the accrued amount (or
  "unknown"), the cap, and any unknown residual, then exits non-zero. The
  latched stop outranks the DAG deadlock diagnosis (as the disk-pressure
  Hold park does), so a capped run whose remaining rules are blocked on a
  failed upstream is never mis-reported as "Deadlock detected". If the
  breach lands on the run's last settle — nothing left undispatched — the
  run ends by its own counts and the summary still carries the
  `cost_budget` detail, so the breach is never invisible.
- **Cluster** (`oxo-flow run --profile <name>`): the same accrual stops new
  submissions at the cap. The halt outcome is identified by the driver's
  own cost-stop error; a deadline or stall failure that follows a latched
  cap keeps its real identity, and a run that merely ends over its cap
  reports its own verdict with the `cost_budget` detail attached.
- A workflow with no `max_cost` behaves byte-identically to before.

Because no run metrics exist before execution, plan-time cost is
**unestimable**: `dry-run`/`validate` report the declared budget but never
fabricate an estimate. See
[Workflow Format → Cost budget](../reference/workflow-format.md#cost-budget-resource_budget-max_cost)
for the field table and
[`report` → Resource ledger](../commands/report.md#resource-ledger-resource-ledger)
for the ledger's `budget`/`cost_accrual` fields.

## CPU Oversubscription Warning

Thread auto-detection is intentionally generous: the scheduling pool can
use every detected CPU. That is a performance good and an operational
problem — on a shared 64-core node, `-j 8` with 8-thread rules occupies all
64 cores for hours, heats the package, and starves co-tenants. The engine
therefore warns **before execution** when the plan's worst-case thread
demand reaches 80 % of the detected CPUs (issue #1080).

The estimate is per scheduled parallel wave:
`min(-j, wave width) × heaviest rule threads in that wave`, with both
factors clamped by the effective pool ceiling. A strictly sequential
workflow (one rule per wave) is never reported as saturating even with a
large `-j`; a single 64-thread rule on a 64-core box is.

```
WARN planned thread demand 64 = up to 8 concurrent jobs × 8 threads/rule,
     vs 64 detected CPUs (100%) — the run will occupy all cores; pass
     --max-threads N or lower -j to leave headroom for the OS and co-tenants
```

The detected CPU count is cgroup-aware: `num_cpus` honours affinity/cpuset
and cgroup v1/v2 CPU quotas (`container --cpus`, systemd `CPUQuota=`,
`cpu.max`), so a 4-core host under a `CPUQuota=200%` scope is reported as
2 CPUs. The warning is computed against the **effective pool ceiling** a
local run will build (`min(detected, [resource_budget] max_threads)`, or an
explicit `--max-threads`), so a cap that leaves headroom silences it; a cap
at or above the host still reports saturation. The estimate is per
scheduled parallel wave, restricted to the rules the run will actually
execute (`-t`/`--module` filtering included).

`dry-run` uses the same detected count (and the workflow budget) for its
suggested `-j`, so a preview shows a safe starting point before the run
does.

## HPC vs Local Best Practices

| Environment | Recommendation |
|---|---|
| **Local workstation** | Declare what you have (undersubscribe for stability) |
| **Local server** | Declare 80-90% of capacity |
| **HPC cluster** | Declare what scheduler allocates |
| **Cloud** | Minimize for cost efficiency |

### Example: Same Workflow, Different Targets

Because rule names must be unique within one workflow, keep the per-target
variants in **separate files** (or give them distinct names):

```toml
# workflows/local.oxoflow — local development (undersubscribe)
[[rules]]
name = "align"

[rules.resources]
threads = 4
memory = "8G"
```

```toml
# workflows/hpc.oxoflow — HPC production (full allocation)
[[rules]]
name = "align"

[rules.resources]
threads = 32
memory = "128G"
partition = "highmem"
```

## Disk Space

Declare disk requirements for large intermediate files:

```toml
[[rules]]
name = "assembly"
shell = "assemble.sh"

[rules.resources]
disk = "500G"  # Warn if <500GB available
```

oxo-flow emits warnings when disk requirements exceed available space but cannot enforce usage.

## Troubleshooting

### Job Killed by OOM

- Increase memory declaration
- Check actual memory usage with system monitoring
- Consider splitting input into smaller chunks

### Timeout Killing Child Processes

Timeout kills the rule's whole process tree on Unix systems: SIGTERM first,
then SIGKILL after a 10-second grace window (deepest descendants first).

Solution: Use wrapper script that manages its own cleanup:

```bash
#!/bin/bash
cleanup() { kill $(jobs -p) 2>/dev/null; }
trap cleanup EXIT
your_long_running_command &
wait
```

### Oversubscription Warnings

If warnings appear but workflow succeeds, you can:

1. Reduce declarations to match system
2. Keep declarations and accept warnings
3. Increase system resources

## Optimizing with DAG Metrics

Use `oxo-flow graph` to get structural insights before tuning resources:

```bash
oxo-flow graph pipeline.oxoflow
```

The header shows key metrics:

```
┌──────────────────────────────────────────────────────────┐
│ Workflow DAG: 12 rules, 15 dependencies                   │
│ Depth: 5, Width: 4, Critical path: 5 steps               │
└──────────────────────────────────────────────────────────┘
```

### Interpreting metrics for resource planning

| Metric | What it tells you | Resource implication |
|---|---|---|
| **Depth** | Number of sequential stages | Determines minimum wall-clock time; each level is a synchronization barrier |
| **Width** | Max rules at any single level | Your peak parallelism — set `-j` to at least this value |
| **Critical path** | Longest chain of dependencies | The bottleneck — focus optimization efforts here |
| **Rules** | Total workflow nodes | Overall scope; large counts may benefit from cluster backends |

### Actionable guidance

- **Width = 1**: Your DAG is fully sequential. Before adding more threads, consider whether you can split large rules into independent sub-tasks to create parallelism.
- **Width > `-j`**: Some parallel rules will queue. Increase `-j` to match or exceed width for maximum throughput.
- **Critical path ≈ Depth**: All levels are equally deep — no obvious bottleneck branch. If runtime is too high, optimize the slowest rule at each level.
- **Critical path < Depth**: Some branches are shallower. The critical path rules are your optimization priority — give them more threads/memory.

### Example: tuning a diamond workflow

```bash
oxo-flow graph pipeline.oxoflow
# Depth: 3, Width: 2, Critical path: source → left → merge
```

Insights:

- Maximum parallelism is 2 (width) — `-j 2` is sufficient
- The critical path is `source → left → merge` — `right` is not on it
- If `left` takes 2 hours, that's your bottleneck regardless of `right`'s speed
- Optimize `left` (more threads, faster tool) before worrying about `right`

## See Also

- [Workflow Format Reference](../reference/workflow-format.md)
- [DAG Engine](../reference/dag-engine.md)
- [DAG Edit API](../reference/dag-edit-api.md)