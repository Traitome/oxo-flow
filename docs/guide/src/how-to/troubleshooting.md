# Troubleshooting Guide

Common issues and their solutions when using oxo-flow.

## Workflow Parsing Errors

### TOML syntax error

**Symptom**: `parse error in workflow.oxoflow: ...`

**Solution**: Check your TOML syntax. Common mistakes:

- Missing quotes around string values
- Incorrect array-of-tables syntax (use `[[rules]]`, not `[rules]`)
- Unmatched brackets or braces

Use `oxo-flow validate workflow.oxoflow` to get detailed error messages.

### Duplicate rule names

**Symptom**: `duplicate rule name: 'step1'`

**Solution**: Every rule must have a unique `name` field. If you're using
`[[include]]` directives, use the `namespace` field to avoid conflicts:

```toml
[[include]]
path = "shared_rules.oxoflow"
namespace = "shared"
```

## Execution Errors

### Rule fails with non-zero exit code

**Symptom**: `rule 'bwa_align' failed with exit code 1`

**Solution**:

1. Run `oxo-flow debug workflow.oxoflow -r bwa_align` to see the expanded
   command with all variables substituted.
2. Check the log output for stderr messages.
3. Try running the expanded command manually in your terminal.
4. Verify that the required tool is installed and available in the rule's
   environment.

### Command not found

**Symptom**: `sh: bwa: command not found`

**Solution**: The tool is not in the system PATH. Either:

- Specify an environment in the rule:
  ```toml
  environment = { conda = "envs/alignment.yaml" }
  ```
- Or ensure the tool is installed and accessible.

### Timeout exceeded

**Symptom**: `command timed out after 3600s for rule 'variant_calling'`

**Solution**: Increase the timeout via `--timeout` flag or allocate more
resources (threads/memory) to the rule.

## Wildcard Issues

### Unresolved wildcards

**Symptom**: `wildcard error in rule '...': unresolved wildcards: sample`

**Solution**: Ensure wildcard values are provided. Wildcards like `{sample}`
must be resolved from:

- Input file patterns matched against existing files
- Explicit values in the config section
- Scatter configuration

### Wildcard constraint violation

**Symptom**: `wildcard 'chr' value 'invalid' does not match constraint '^chr[0-9XYM]+$'`

**Solution**: The wildcard value doesn't match the regex constraint defined in
your workflow. Check that your input filenames follow the expected naming
convention.

## Environment Issues

### Conda environment creation fails

**Symptom**: `environment error (conda): ...`

**Solution**:

1. Check that conda/mamba is installed: `conda --version`
2. Verify the environment YAML file exists and is valid
3. Check for network connectivity (package downloads)
4. Try creating the environment manually: `conda env create -f envs/tool.yaml`

### Docker image not found

**Symptom**: `environment error (docker): ...`

**Solution**:

1. Check that Docker is installed and running: `docker info`
2. Verify the image reference: `docker pull biocontainers/bwa:0.7.17`
3. Check for authentication if using private registries

### HPC modules not available

**Symptom**: Module load errors when using `modules` in environment spec

**Solution**: Verify that the module system is available on your HPC node
and that the specified module names and versions are correct:

```bash
module avail gcc
module avail cuda
```

## DAG Issues

### Cycle detected

**Symptom**: `cycle detected in workflow DAG: A -> B -> A`

**Solution**: Your rules have circular dependencies. Use `oxo-flow graph`
to visualize the DAG and identify the cycle. Break the cycle by:

- Removing unnecessary input/output connections
- Splitting a rule into separate steps
- Using `depends_on` for explicit ordering instead of file-based dependencies

### Missing input

**Symptom**: `missing input for rule 'step2': intermediate.txt`

**Solution**: Ensure that some other rule produces `intermediate.txt` as an
output, or that the file already exists before the workflow runs.

## Checkpoint and Resume

### Resuming a failed workflow

After fixing the cause of a failure, re-run the same workflow. oxo-flow will
check checkpoints and skip already-completed rules:

```bash
oxo-flow run workflow.oxoflow
```

### Clearing checkpoint state

To force a full re-run, delete the checkpoint file:

```bash
rm -rf .oxo-flow/checkpoint.json
oxo-flow run workflow.oxoflow
```

## Performance Tips

### Workflow runs slowly

1. **Increase parallelism**: Use `-j` to run more jobs concurrently:
   ```bash
   oxo-flow run workflow.oxoflow -j 8
   ```

2. **Check resource constraints**: Use `oxo-flow debug` to verify that
   resource requirements are reasonable.

3. **Use streaming**: For I/O-bound rules, set `pipe = true` to enable
   FIFO-based streaming where supported.

4. **Enable caching**: Set `cache_key` on rules to enable content-based
   output reuse.

### Memory issues with large workflows

For workflows with many samples (>1,000):

1. Process samples in batches using scatter/gather patterns
2. Increase system memory limits
3. Use cluster backends for distributed execution

## Getting Help

- Run `oxo-flow --help` for CLI usage
- Run `oxo-flow <command> --help` for subcommand details
- Run `oxo-flow debug workflow.oxoflow` to inspect resolved commands
- Check [LIMITATIONS.md](https://github.com/Traitome/oxo-flow/blob/main/LIMITATIONS.md)
  for known limitations
- [Open an issue](https://github.com/Traitome/oxo-flow/issues) for bugs or
  feature requests
