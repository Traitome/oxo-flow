# `oxo-flow cluster`

Manage cluster job submission and monitoring.

---

## Usage

```
oxo-flow cluster <ACTION> [OPTIONS] [WORKFLOW/JOB_IDS]
```

---

## Actions

| Action | Description |
|---|---|
| `submit` | Submit a workflow to a cluster scheduler |
| `status` | Show the status of submitted cluster jobs |
| `cancel` | Cancel submitted cluster jobs |

---

## Arguments

| Argument | Description |
|---|---|
| `<WORKFLOW>` | Path to the `.oxoflow` workflow file (for `submit`) |
| `<JOB_ID>...` | One or more cluster job IDs (for `cancel`) |

---

## Options (Submit)

| Option | Short | Default | Description |
|---|---|---|---|
| `--backend` | `-b` | `slurm` | Cluster backend (`slurm`, `pbs`, `sge`, `lsf`) |
| `--queue` | `-q` | — | Partition / queue name |
| `--account` | `-a` | — | Account / project name |
| `--output-dir` | `-o` | `.oxo-flow/cluster` | Directory for generated scripts |

---

## Examples

### Submit to SLURM

```bash
oxo-flow cluster submit pipeline.oxoflow -b slurm -q work
```

### Check job status

```bash
oxo-flow cluster status -b slurm
```

### Cancel specific jobs

```bash
oxo-flow cluster cancel -b slurm 12345 12346
```

---

## Output

```
oxo-flow 0.3.0 — Bioinformatics Pipeline Engine
Cluster: Generating slurm job scripts for 5 rules
  ✓ .oxo-flow/cluster/fastqc.sh
  ✓ .oxo-flow/cluster/trim_reads.sh
  ✓ .oxo-flow/cluster/bwa_align.sh
  ✓ .oxo-flow/cluster/sort_bam.sh
  ✓ .oxo-flow/cluster/call_variants.sh

Done: 5 scripts written to .oxo-flow/cluster
  Submit with: sbatch .oxo-flow/cluster/*.sh
```

---

## Notes

- `submit` generates shell scripts tailored for the specified cluster backend
- Resource requirements (threads, memory) from the workflow are automatically translated to cluster directives
- `status` and `cancel` provide convenient wrappers around native cluster commands
