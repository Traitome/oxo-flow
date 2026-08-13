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
      default, declare `threads = 2` or higher (a single thread is not
      expressible; there is no way to force 1).

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
runtime = "slow"         # >1 hour expected
```

Estimated memory: 100GB × 2.5 = 250GB

## Resource Budgets

Limit total concurrent resource usage:

```toml
[resource_budget]
max_threads = 64        # Don't exceed 64 threads total
max_memory = "256G"     # Don't exceed 256GB total
max_jobs = 10           # Max 10 concurrent jobs
```

Useful for shared servers or when running multiple workflows.

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

Timeout uses process group SIGKILL on Unix systems (reliable cleanup).

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