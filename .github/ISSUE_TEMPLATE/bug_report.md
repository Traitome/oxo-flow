---
name: Bug Report
about: Report a bug in oxo-flow
title: '[BUG] '
labels: bug
assignees: ''
---

## Description

A clear and concise description of the bug.

## Steps to Reproduce

1. Create a workflow file with the following content: ...
2. Run `oxo-flow run ...`
3. Observe the error

## Expected Behavior

A clear description of what you expected to happen.

## Actual Behavior

A clear description of what actually happened. Include any error messages, stack
traces, or log output.

```
Paste error output here
```

## Environment

- **oxo-flow version**: (output of `oxo-flow --version`)
- **Operating system**: (e.g., Ubuntu 22.04, macOS 14.2)
- **Rust version**: (output of `rustc --version`, if building from source)
- **Container runtime**: (e.g., Docker 24.0.7, Singularity 3.11)
- **Environment manager**: (e.g., conda 23.10, pixi 0.12)
- **Cluster scheduler**: (e.g., SLURM 23.02, N/A for local execution)

## Workflow File

If applicable, provide the `.oxoflow` file (or a minimal reproduction):

```toml
# Paste your .oxoflow file here
```

## Additional Context

Add any other context about the problem here (e.g., screenshots, related
issues, or workarounds you have tried).
