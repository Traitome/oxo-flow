# oxo-flow — Cover Letter Highlights

## Opening frame (first paragraph of the letter)

Modern biology runs on computational pipelines, and increasingly those pipelines are drafted by AI. Yet whether a computational result can be trusted still depends on the discipline of whoever wrote and ran the code. oxo-flow turns trust, reproducibility, and safety from personal practice into properties of the infrastructure itself.

## Scientific advances

- **Validation of AI-written analyses becomes a property of the system, not of the user.** AI can draft analysis code that silently does the wrong thing. oxo-flow accepts a draft only after it passes the engine's own checks, returns rejected drafts with specific errors like a referee's report, and grants an independent reviewer, who never saw how the code was written, at most one revision; to our knowledge, no workflow system in current use validates AI-generated analyses this way. Measurement decided the design: accepting review fixes without rechecking cut first-pass success from 83% to 56%.

- **A reproducible standard for judging AI in science.** Claims that AI writes correct analyses are usually verified by the opinion of another AI, which nobody can reproduce. Here the grading is done by fixed tests built into the engine, rerun automatically with every change to the software, so any reported success rate can be re-checked by anyone.

- **Data integrity protected by the language, not by caution.** Sample names and file lists arrive from collaborators and public repositories, and in today's tools a name crafted to hide a command would be executed inside the analysis. oxo-flow vets every such value before any step runs and rejects embedded commands outright, even when a user relaxes other restrictions, so one poisoned spreadsheet cannot hijack an experiment.

## Engineering advances

- **Rigorous methods that travel.** The engine installs as a single program with no other software required and speaks one interface to every common packaging scheme, from Conda to containers to cluster modules, so a pipeline developed on a laptop runs unchanged on a national facility and in a collaborator's lab. When the recipe behind a software environment changes, the engine notices and rebuilds rather than silently reusing a stale one, a common and quiet source of wrong results.

- **Every result ships with its own evidence.** Each analysis writes one complete record: the method version, the inputs, the outputs, the exact commands, and the console output; identical analyses yield identical records, and a built-in command re-verifies any stored result years later. Only the steps whose inputs or methods truly changed are rerun, so an audit never punishes revision with hours of recomputation. This is the provenance that journals and funders increasingly demand, delivered by default rather than by discipline.

- **Failures become diagnoses, not dead ends.** The engine recognizes thirty recurring failure types and states the likely cause and remedy in plain terms before any AI is consulted; an AI-proposed repair is applied only when the system judges it safe and the repaired pipeline passes every check, with the original always archived. For a lab without computing support, this is the difference between losing an afternoon and losing a week.
