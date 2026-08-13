# oxo-flow Examples

Two tiers of example workflows:

- **`examples/*.oxoflow`** — top-level pattern references
  (variant calling, paired experiment/control, cohorts, conditionals).
- **`examples/gallery/*.oxoflow`** — numbered progressive gallery
  (01 hello world → 10 transform operator), documented in
  `docs/guide/src/gallery/`.

## What ships, what doesn't

| Artifact | Shipped? | Notes |
|---|---|---|
| Workflow definitions (`.oxoflow`) | ✅ | Fully reviewed: valid, DAG-correct, read-group and environment consistent |
| Conda environment specs (`envs/*.yaml`) | ✅ | One file per environment name referenced by the workflows — copy `envs/` next to a workflow (or into your project) and adapt pins to your cluster |
| Input data (`raw/`, `/data/references/...`) | ❌ | Placeholders — the workflows are reference patterns; replace paths in `[config]` with your own data |
| Auxiliary scripts (`scripts/*.py`, `scripts/*.R`, `templates/*.Rmd`) | ❌ | User-provided analysis code; the workflows call them by convention, keep the filenames or adjust the rules |

## Try it

The first examples run with no data or environments at all:

```bash
oxo-flow run examples/gallery/01_hello_world.oxoflow
oxo-flow run examples/gallery/02_file_pipeline.oxoflow
```

Bioinformatics examples need the referenced data and tools:

```bash
mkdir -p my_project && cp examples/gallery/07_wgs_germline.oxoflow my_project/
cp -r examples/envs my_project/envs
# edit [config] paths to your reference data, then:
cd my_project && oxo-flow run 07_wgs_germline.oxoflow
```
