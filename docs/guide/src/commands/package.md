# `oxo-flow package`

Package a workflow into a container image definition (Dockerfile or Singularity definition file).

---

## Usage

```
oxo-flow package [OPTIONS] <WORKFLOW>
```

---

## Arguments

| Argument | Description |
|---|---|
| `<WORKFLOW>` | Path to the `.oxoflow` workflow file |

---

## Options

| Option | Short | Default | Description |
|---|---|---|---|
| `--format` | `-f` | `docker` | Container format: `docker` or `singularity` |
| `--output` | `-o` | stdout | Output file path |
| `--verbose` | `-v` | — | Enable debug-level logging |

---

## Examples

### Generate a Dockerfile

```bash
oxo-flow package pipeline.oxoflow
```

### Write Dockerfile to a file

```bash
oxo-flow package pipeline.oxoflow -o Dockerfile
```

### Generate a Singularity definition

```bash
oxo-flow package pipeline.oxoflow -f singularity -o pipeline.def
```

### Build the container

```bash
# Docker
oxo-flow package pipeline.oxoflow -o Dockerfile
docker build -t my-pipeline:1.0.0 .

# Singularity
oxo-flow package pipeline.oxoflow -f singularity -o pipeline.def
singularity build pipeline.sif pipeline.def
```

---

## Output

### Dockerfile example

```dockerfile
FROM ubuntu:22.04
LABEL maintainer="oxo-flow"
LABEL version="1.0.0"

RUN apt-get update && apt-get install -y \
    wget curl

# Install conda
RUN wget -q https://repo.anaconda.com/miniconda/Miniconda3-latest-Linux-x86_64.sh && \
    bash Miniconda3-latest-Linux-x86_64.sh -b -p /opt/conda && \
    rm Miniconda3-latest-Linux-x86_64.sh

ENV PATH="/opt/conda/bin:$PATH"

COPY . /workflow
WORKDIR /workflow
```

### Singularity definition example

```
Bootstrap: docker
From: ubuntu:22.04

%labels
    maintainer oxo-flow
    version 1.0.0

%post
    apt-get update && apt-get install -y wget curl

%files
    . /workflow

%runscript
    cd /workflow
    exec oxo-flow run pipeline.oxoflow "$@"
```

---

## Notes

- The `package` command generates container *definitions*, not built images — you must run `docker build` or `singularity build` separately
- If `--output` is not specified, the definition is printed to stdout
- The generated containers include all workflow files, environment specs, and scripts needed for self-contained execution
- Container packaging is key for reproducibility: the same container produces the same results regardless of the host system
