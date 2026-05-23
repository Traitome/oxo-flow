# oxo-flow schema

Output the JSON Schema for the `.oxoflow` format.

## Usage

```
oxo-flow schema
```

## Description

Prints a JSON Schema (Draft-07) document describing the structure and
validation rules of `.oxoflow` workflow files. The schema can be used
for IDE validation, automated linting, or integrating oxo-flow into
larger toolchains.

## Examples

```bash
# Print the schema to stdout
oxo-flow schema

# Save to a file
oxo-flow schema > oxoflow-schema.json

# Validate a workflow against the schema (using a JSON Schema validator)
oxo-flow schema > schema.json
```

## See Also

- [oxo-flow validate](validate.md) — validate a workflow file
- [Workflow Format Reference](../reference/workflow-format.md)
