# oxo-flow publish

Bundle a workflow with its environment files for sharing or archival.

## Usage

```
oxo-flow publish <WORKFLOW>
```

## Description

Creates a self-contained bundle directory containing the `.oxoflow` workflow
file and a `manifest.json` metadata file. The bundle is ready for sharing,
archiving, or submission to a workflow registry.

## Examples

```bash
# Publish a workflow
oxo-flow publish my_pipeline.oxoflow

# The bundle is created as my_pipeline-bundle/
#   my_pipeline-bundle/my_pipeline.oxoflow
#   my_pipeline-bundle/manifest.json
```

## See Also

- [oxo-flow export](export.md) — export to container definition or standalone TOML
- [oxo-flow package](package.md) — package into a container image
