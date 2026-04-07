# `oxo-flow debug`

Debug a workflow by showing each rule with its fully resolved shell command,
resource requirements, environment, dependencies, and metadata. Useful for
verifying that template variables are substituted correctly.

---

## Usage

```
oxo-flow debug <WORKFLOW> [OPTIONS]
```

---

## Arguments

| Argument | Description |
|---|---|
| `<WORKFLOW>` | Path to the `.oxoflow` workflow file |

---

## Options

| Option | Short | Description |
|---|---|---|
| `--rule <NAME>` | `-r` | Show only a specific rule (by name) |
| `--verbose` | `-v` | Enable debug-level logging |

---

## Examples

### Debug all rules in a workflow

```bash
oxo-flow debug pipeline.oxoflow
```

### Debug a specific rule

```bash
oxo-flow debug pipeline.oxoflow -r bwa_align
```

---

## Output

For each rule, the debug command shows:

- **Rule name** and description
- **Inputs and outputs** (with wildcard patterns)
- **Shell command** — both the raw template and the expanded version
- **Resources** — threads, memory, GPU specifications
- **Environment** — conda, docker, singularity, modules, etc.
- **Dependencies** — both file-based and explicit `depends_on`
- **Tags** — categorization labels
- **Format hints** — declared file formats
- **Metadata** — arbitrary domain-specific key-value pairs
- **Wildcards** — wildcard names extracted from patterns

```
── Rule: bwa_align ──
  Description: Align reads to reference genome
  Inputs: ["trimmed/{sample}_R1.fastq.gz", "trimmed/{sample}_R2.fastq.gz"]
  Outputs: ["aligned/{sample}.sorted.bam"]
  Shell (template): bwa mem -t {threads} {config.reference} {input} | samtools sort -o {output}
  Shell (expanded): bwa mem -t 16 {config.reference} trimmed/{sample}_R1.fastq.gz trimmed/{sample}_R2.fastq.gz | samtools sort -o aligned/{sample}.sorted.bam
  Resources: threads=16, memory=32G
  Environment: docker
  Dependencies: ["trim_reads"]
  Tags: ["alignment"]
  Wildcards: ["sample"]
```

---

## Notes

- The debug command does not execute any shell commands
- Template variables like `{input}`, `{output}`, and `{threads}` are
  substituted in the expanded view
- Wildcard placeholders like `{sample}` remain unresolved since actual
  values depend on input file discovery at runtime
- Use this command to verify variable substitution before running a workflow
