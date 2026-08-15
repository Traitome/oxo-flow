# Environment Management

oxo-flow supports eight software environment backends. This tutorial shows how to use each one, how to mix them in a single workflow, and how to check that required environments are available.

---

## Supported Backends

| Backend | Keyword | Use case |
|---|---|---|
| **Conda** | `conda` | General bioinformatics tools via Bioconda |
| **Mamba** | `mamba` | Faster conda-compatible solver (libmamba) — drop-in conda replacement |
| **Pixi** | `pixi` | Fast conda-compatible package management |
| **Docker** | `docker` | Containerized, reproducible execution |
| **Singularity** | `singularity` | HPC-friendly containers (no root required) |
| **Python venv** | `venv` | Lightweight Python-only environments |
| **System** | *(no environment field)* | Host shell environment — no isolation (default) |
| **Modules** | `modules` | HPC environment modules (Lmod/Environment Modules) for cluster toolchains |

---

## Per-rule Environment Declaration

Each rule in an `.oxoflow` file can declare its own environment using the `environment` field:

```toml
[[rules]]
name = "align"
input = ["reads.fastq.gz"]
output = ["aligned.bam"]
environment = { conda = "envs/alignment.yaml" }
shell = "bwa mem ref.fa reads.fastq.gz | samtools sort -o aligned.bam"
```

If no environment is specified, the rule runs in the system's default shell environment.

---

## Conda

The most common backend for bioinformatics. Point to a YAML environment file:

```toml
environment = { conda = "envs/tools.yaml" }
```

The YAML file follows standard conda format:

```yaml
# envs/tools.yaml
name: tools
channels:
  - bioconda
  - conda-forge
dependencies:
  - bwa=0.7.19
  - samtools=1.24
```

!!! note "Why Bioconda?"
    The `bioconda` channel is a community-maintained repository for bioinformatics software. It provides thousands of pre-compiled binaries for tools like `bwa`, `samtools`, and `GATK`, which can otherwise be difficult to install from source.

oxo-flow activates the conda environment before running the rule's shell command and deactivates it afterward.

---

## Mamba

Mamba (or micromamba) uses the same YAML format as conda with a faster solver. oxo-flow auto-detects which binary is available:

```toml
environment = { mamba = "envs/tools.yaml" }
```

Mamba is a drop-in conda replacement — the YAML file is identical. If only conda is installed, the `mamba` keyword falls back to conda automatically.

---

## Pixi

[Pixi](https://pixi.sh) provides fast, lockfile-based environment management compatible with conda packages:

```toml
environment = { pixi = "envs/pixi.toml" }
```

```toml
# envs/pixi.toml
[project]
name = "alignment"
channels = ["bioconda", "conda-forge"]
platforms = ["linux-64"]

[dependencies]
bwa = "0.7.19"
samtools = "1.24"
```

---

## Docker

Use pre-built container images from registries like BioContainers:

```toml
environment = { docker = "biocontainers/bwa:0.7.19--h577a1d6_1" }
```

oxo-flow runs the rule's shell command inside the container, mounting the working directory automatically:

```bash
docker run --rm -v $(pwd):$(pwd) -w $(pwd) biocontainers/bwa:0.7.19--h577a1d6_1 \
  bwa mem ref.fa reads.fastq.gz
```

!!! tip "No daemon required at build time"
    oxo-flow only needs Docker at *runtime*. The workflow file itself is always plain TOML — no Dockerfile required in the project.

---

## Singularity / Apptainer

For HPC clusters where Docker is not available:

```toml
environment = { singularity = "docker://biocontainers/bwa:0.7.19--h577a1d6_1" }
```

Singularity can pull images directly from Docker registries. The working directory is bound automatically.

---

## HPC Modules

On clusters using Lmod / Environment Modules, load toolchain modules for the rule:

```toml
environment = { modules = ["gcc/11.2.0", "openmpi/4.1.1", "samtools/1.24"] }
```

The rule's shell command runs after all listed modules are loaded, and the module environment is cleaned up afterward. This is the standard way to use cluster-administered software stacks.

---

## Python venv

For rules that only need Python packages:

```toml
environment = { venv = "envs/requirements.txt" }
```

```text
# envs/requirements.txt
pandas>=2.0
matplotlib>=3.8
```

oxo-flow creates (or reuses) a virtual environment and installs the listed packages before executing the rule.

---

## Mixing Environments

A single workflow can use different environments for different rules:

```toml
[[rules]]
name = "align"
environment = { docker = "biocontainers/bwa:0.7.19--h577a1d6_1" }
# ...

[[rules]]
name = "call_variants"
environment = { conda = "envs/gatk.yaml" }
# ...

[[rules]]
name = "plot_results"
environment = { venv = "envs/requirements.txt" }
# ...
```

Each rule activates its own environment independently. This lets you use the best tool for each step without conflicts.

---

## Default Environment

Set a default environment in the `[defaults]` section so you don't have to repeat it for every rule:

```toml
[defaults]
threads = 4
environment = { conda = "envs/base.yaml" }

[[rules]]
name = "step_a"
# Uses the default conda environment
# ...

[[rules]]
name = "step_b"
environment = { docker = "custom/image:latest" }
# Overrides the default with Docker
# ...
```

---

## Checking Environments

Before running a workflow, verify that all required environment backends are available:

```bash
# List available backends on this system
oxo-flow env list
```

```
oxo-flow 0.13.0 — Bioinformatics Pipeline Engine
Available environment backends:
  ✓ system
  ✓ mamba
  ✓ conda
  ✓ docker
  ✓ venv
```

The output is system-dependent — only backends installed on the current machine are listed (unavailable ones are omitted entirely).

Check that all environments in a specific workflow are valid:

```bash
oxo-flow env check my-pipeline.oxoflow
```

```
oxo-flow 0.13.0 — Bioinformatics Pipeline Engine
  ✓ align (docker)
  ✓ call_variants (conda)
  ✓ plot_results (venv)
```

---

## Next Steps

- [Use Environments](../how-to/use-environments.md) — detailed how-to for each backend
- [Run on a Cluster](../how-to/run-on-cluster.md) — containers on HPC with Singularity
- [Environment System](../reference/environment-system.md) — architecture reference
