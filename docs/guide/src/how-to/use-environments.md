# Use Environments

This guide provides practical recipes for the environment backends supported
by oxo-flow. The engine ships eight backends: `conda`, `mamba`, `pixi`,
`docker`, `singularity`, `venv`, and `modules` (plus the implicit `system`
fallback when a rule declares no environment). Recipes below cover the most
commonly used ones; `mamba` uses the same YAML syntax as `conda`, and
`modules` takes a list of HPC module names.

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

1. oxo-flow checks if the environment already exists (keyed by the YAML specification)
2. If not, it creates it from the YAML file
3. The shell command runs inside the environment via `conda run -n <env-name> bash -c '<command>'`, where `<env-name>` is the `name:` from your YAML plus a short content-hash suffix for file specs (`<name>-<hash8>`, see workflow-format) — so two different YAMLs that happen to share a name never collide
4. The environment is created once and reused by every rule that references the same YAML

!!! tip "Reuse environments"
    Multiple rules can share the same conda YAML file. oxo-flow creates the environment once and reuses it — identical YAML content reuses the same env even across workflows.

---

## Docker Containers

### Use a BioContainers image

```toml
[[rules]]
name = "align"
environment = { docker = "quay.io/biocontainers/bwa:0.7.19--h577a1d6_1" }
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
docker pull quay.io/biocontainers/bwa:0.7.19--h577a1d6_1
```

Bare image names get one automatic retry against quay.io after a Docker
Hub miss — Biocontainers publishes on quay.io, not Docker Hub:

- `biocontainers/bwa:0.7.17` → retried as `quay.io/biocontainers/bwa:0.7.17`
- `bwa:0.7.17` (single name) → retried as `quay.io/biocontainers/bwa:0.7.17`

Explicit registries (`quay.io/…`, `docker.io/…`, `localhost:5000/…`) are
pulled verbatim and never shadowed by the retry.

---

## Singularity / Apptainer

### Pull from Docker Hub

```toml
[[rules]]
name = "align"
environment = { singularity = "docker://quay.io/biocontainers/bwa:0.7.19--h577a1d6_1" }
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
# pixi.toml — at the workflow root (the directory you run oxo-flow from)
[project]
name = "qc-tools"
channels = ["bioconda", "conda-forge"]
platforms = ["linux-64"]

[dependencies]
fastqc = "0.12.1"
fastp = "0.23.4"
```

### Reference in a rule

The `pixi` value is the pixi **environment name** (the default environment
in `pixi.toml` is named `default`), not a file path:

```toml
[[rules]]
name = "fastqc"
environment = { pixi = "default" }
shell = "fastqc input.fastq.gz -o qc/"
```

oxo-flow runs `pixi install -e default` once, then wraps the command as
`pixi run -e default <command>`.

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

The `venv` value is the **directory** where the virtual environment is
created; the requirements file goes in the separate `venv_requirements`
field (defaults to `requirements.txt` in the working directory):

```toml
[[rules]]
name = "plot_results"
environment = { venv = "venv/", venv_requirements = "envs/requirements.txt" }
shell = "python scripts/plot.py --input results.csv --output plot.png"
```

### How it works

1. oxo-flow creates the venv at the given directory with `python3 -m venv` (or reuses it if it already exists)
2. Packages from the requirements file are installed with pip
3. The shell command runs with the venv activated (`source <dir>/bin/activate && <command>`)

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
environment = { venv = "venv/", venv_requirements = "envs/requirements.txt" }
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
