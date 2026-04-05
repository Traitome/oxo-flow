# Use Environments

This guide provides practical recipes for each environment backend supported by oxo-flow.

---

## Conda Environments

### Create an environment file

```yaml
# envs/alignment.yaml
name: alignment
channels:
  - bioconda
  - conda-forge
dependencies:
  - bwa=0.7.17
  - samtools=1.19
  - picard=3.1.1
```

### Reference it in a rule

```toml
[[rules]]
name = "align"
environment = { conda = "envs/alignment.yaml" }
shell = "bwa mem ref.fa reads.fastq.gz | samtools sort -o aligned.bam"
```

### How it works

1. oxo-flow checks if the environment already exists
2. If not, it creates it from the YAML specification
3. The environment is activated before the shell command runs
4. After the command completes, the environment is deactivated

!!! tip "Reuse environments"
    Multiple rules can share the same conda YAML file. oxo-flow creates the environment once and reuses it.

---

## Docker Containers

### Use a BioContainers image

```toml
[[rules]]
name = "align"
environment = { docker = "biocontainers/bwa:0.7.17--h7132678_3" }
shell = "bwa mem ref.fa reads.fastq.gz | samtools sort -o aligned.bam"
```

### Use a custom image

```toml
environment = { docker = "myregistry.io/my-tools:1.0.0" }
```

### Volume mounting

oxo-flow automatically mounts the working directory into the container. Input and output paths are resolved relative to the mount point.

### Pull policy

Images are pulled on first use. If you need offline operation, pre-pull images:

```bash
docker pull biocontainers/bwa:0.7.17--h7132678_3
```

---

## Singularity / Apptainer

### Pull from Docker Hub

```toml
[[rules]]
name = "align"
environment = { singularity = "docker://biocontainers/bwa:0.7.17--h7132678_3" }
shell = "bwa mem ref.fa reads.fastq.gz > aligned.sam"
```

### Use a local SIF file

```toml
environment = { singularity = "/shared/containers/bwa-0.7.17.sif" }
```

### HPC considerations

Singularity is the preferred container runtime for HPC clusters because it:

- Does not require root privileges
- Integrates with cluster schedulers (SLURM, PBS)
- Supports shared filesystem mounts automatically

---

## Pixi Environments

### Create a pixi.toml

```toml
# envs/pixi.toml
[project]
name = "qc-tools"
channels = ["bioconda", "conda-forge"]
platforms = ["linux-64"]

[dependencies]
fastqc = "0.12.1"
fastp = "0.23.4"
```

### Reference in a rule

```toml
[[rules]]
name = "fastqc"
environment = { pixi = "envs/pixi.toml" }
shell = "fastqc input.fastq.gz -o qc/"
```

---

## Python Virtual Environments

### Create a requirements file

```text
# envs/requirements.txt
pandas>=2.0
matplotlib>=3.8
seaborn>=0.13
```

### Reference in a rule

```toml
[[rules]]
name = "plot_results"
environment = { venv = "envs/requirements.txt" }
shell = "python scripts/plot.py --input results.csv --output plot.png"
```

### How it works

1. oxo-flow creates a venv in a cache directory (or reuses an existing one)
2. Packages from the requirements file are installed with pip
3. The venv is activated for the shell command

---

## Mixing Backends in One Workflow

```toml
[[rules]]
name = "align"
environment = { docker = "biocontainers/bwa:0.7.17" }
# ...

[[rules]]
name = "call_variants"
environment = { conda = "envs/gatk.yaml" }
# ...

[[rules]]
name = "annotate"
environment = { singularity = "docker://ensemblorg/ensembl-vep:112.0" }
# ...

[[rules]]
name = "report"
environment = { venv = "envs/requirements.txt" }
# ...
```

---

## Checking Availability

```bash
# List all backends available on this system
oxo-flow env list

# Check all environments in a specific workflow
oxo-flow env check pipeline.oxoflow
```

---

## Troubleshooting

| Problem | Solution |
|---|---|
| `conda: command not found` | Install Miniconda/Miniforge and ensure `conda` is on your `$PATH` |
| Docker permission denied | Add your user to the `docker` group or use Singularity |
| Singularity pull fails | Check network access; pre-pull images with `singularity pull` |
| Pip install fails in venv | Ensure `python3` and `pip` are available on the system |

---

## See Also

- [Environment Management tutorial](../tutorials/environment-management.md) — getting started with environments
- [Environment System reference](../reference/environment-system.md) — architecture details
