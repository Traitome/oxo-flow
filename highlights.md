Modern biology runs on computational pipelines, and increasingly those pipelines are drafted by AI. Yet whether a computational result can be trusted still depends on the discipline of whoever wrote and ran the code. oxo-flow turns trust, reproducibility, and safety from personal practice into properties of the infrastructure itself.

## Scientific advances

- **Validation of AI-written analyses becomes a property of the system, not of the user.** AI can draft analysis code that silently does the wrong thing. oxo-flow targets a workflow in which a draft reaches a run only after passing the engine's own checks, rejected drafts return specific errors, and an independent reviewer gets a bounded revision. That target is not yet a universal safety guarantee: the audit ledger records fail-open tool arguments, partial-file repair risk, and incomplete resolved-graph admission.

- **A reproducible standard for judging AI in science.** Claims that AI writes correct analyses are usually verified by the opinion of another AI, which nobody can reproduce. Here the grading is done by fixed tests built into the engine, rerun automatically with every change to the software, so any reported success rate can be re-checked by anyone.

- **Data integrity protected by the language, not by caution.** Sample names and file lists arrive from collaborators and public repositories, and in today's tools a name crafted to hide a command would be executed inside the analysis. oxo-flow vets every such value before any step runs and rejects embedded commands outright, even when a user relaxes other restrictions, so one poisoned spreadsheet cannot hijack an experiment.

## Engineering advances

- **Rigorous methods that travel.** The engine installs as a single program with no other software required and speaks one interface to every common packaging scheme, from Conda to containers to cluster modules, so a pipeline developed on a laptop runs unchanged on a national facility and in a collaborator's lab. When the recipe behind a software environment changes, the engine notices and rebuilds rather than silently reusing a stale one, a common and quiet source of wrong results.

- **Every result ships with one reviewable evidence record.** An analysis records the method version, inputs, outputs, exact commands, console output, and checkpoint state; a built-in command re-verifies stored output checksums where they were recorded. Record equality requires matching software environments, inputs, commands, and execution settings, and provenance does not by itself establish biological correctness.

- **Failures become diagnoses, not dead ends.** The engine recognizes recurring failure types and states the likely cause and a candidate remedy before any AI is consulted. AI repair is experimental: current simulations emit proposals but have not established applied, verified, and successfully rerun repairs; every proposed change requires an exact diff review and full validation.
