# Open or create a pipeline

Open any `.oxoflow` file in the workspace — syntax highlighting,
completion, and folding come alive immediately. No pipeline yet?

Two quick starts:

- **From the terminal**: `oxo-flow init my-pipeline` scaffolds a
  project with a starter `.oxoflow` file, directory layout, and
  `.gitignore`.
- **From scratch**: create an empty `pipeline.oxoflow` and type the
  `pipeline` snippet — it expands into a `[workflow]` table plus a
  first `[[rules]]` block.
- **With AI**: run **oxo-flow: Generate Pipeline with AI…** and
  describe the analysis in plain language (needs an `[ai]` provider
  configured).

While editing, the Problems panel shows `validate`/`lint` findings on
save (configurable via **oxo-flow.diagnosticMode**).
