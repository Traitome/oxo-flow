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
threads = 16
memory = "32G"  # 2× expected input size
```

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
threads = 8
memory = "64G"
```

## GPU Resources

### SLURM GPU Request

```toml
[[rules]]
name = "gpu_training"
threads = 8
memory = "64G"

[rules.resources.gpu_spec]
count = 2
model = "A100"
memory_gb = 40
```

Generated SLURM directive: `--gres=gpu:a100:2:40g --mem-per-gpu=40G`

### Common GPU Tools

| Tool | GPU Memory | Notes |
|---|---|
| **ParaBricks** | 40+ GB per GPU | NVIDIA A100 recommended |
| **Clara Parabricks** | 32+ GB | GPU-accelerated variant calling |
| **DeepVariant GPU** | 16+ GB | Faster than CPU version |

### PBS/SGE GPU

GPU syntax varies by site. Use `extra_args`:

```toml
[rules.resources]
gpu = 2

[rules.resources]
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

```toml
# Local development (undersubscribe)
[[rules]]
name = "align"
threads = 4
memory = "8G"

# HPC production (full allocation)
[[rules]]
name = "align"
threads = 32
memory = "128G"
partition = "highmem"
```

Consider using separate workflow files or conditional logic.

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

Unix: timeout uses process group SIGKILL (reliable)
Windows: may leave orphan processes

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

## See Also

- [Workflow Format Reference](../reference/workflow-format.md)
- [Execution Backends](../reference/execution-backends.md)