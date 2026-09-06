# The `graph` Subcommand: Personas, Granularity Ladder, and Export Design

This page documents the design behind `oxo-flow graph` — the scenario-driven
rationale (who reads a workflow diagram, and why), the granularity ladder
that serves those scenarios, the format matrix, and the engine-side design
invariants that keep every export generic.

## Personas

Seven reader roles drive the design. Each reads a workflow DAG for a
different purpose, at a different level of detail:

| Persona | Scenario | Needs | Served by |
|---|---|---|---|
| **Learner** — first contact with a new workflow | learning | "What does this pipeline *do*?" — ~10-25 stops, plain-language section names, stage colors showing the read→align→quantify→report flow | `-f metro --granularity module`, gallery pages |
| **Reviewer** — checking a port against its upstream | preview | Tool names per step (is `samtools index` present?), correspondence with the upstream nf-core/snakemake process list | `-f metro --granularity process`, fidelity tables |
| **Operator** — running, rerunning, debugging a run | computation | Rule-level detail: which rules map to which checkpoint entries, files, and resource settings | `-f metro --granularity rule`, `-f ascii`, `-f tree`, `--expanded` |
| **Publication author** — preparing a figure for a paper | publication | A compact, print-friendly, color-blind-safe transit map at figure scale (~2:1 landscape, ≤ ~1800 px wide) | `-f metro --granularity module`, `nf-metro render` recipes |
| **Site maintainer** — regenerating diagrams for every community pipeline | documentation | Deterministic output, a repeatable automation script, pinned engine and renderer versions | `graph` + the site regeneration pipeline (issue #16) |
| **Engine developer** — extending the exporter | development | A generic design: one mechanism serves every workflow; no per-pipeline special cases | granularity ladder, shared stage/tool knowledge tables |
| **Auditor** — graph analysis with external tools | audit | Machine-readable DAGs for graphviz pipelines and custom analysis | `-f dot`, `-f dot-clustered`, `--expanded` |

## The granularity ladder

One DAG, three levels of abstraction — the same ladder the published
nf-core transit maps climb implicitly (their stations are process-level,
grouped into stage sections):

- **`rule`** (default) — every rule is one station. Mechanical truth:
  useful to map rules ↔ checkpoints ↔ files while operating a run, and as
  the base truth the coarser tiers collapse. Dense ported workflows (live:
  community mag, 600+ rules) produce very large maps here.

- **`process`** — rules that are chain-connected within a section and
  driven by the same tool collapse into one tool-named station (the
  nf-core idiom: `samtools sort` → `samtools index` → one "SAMtools"
  stop). The collapse is cycle-safe: a union applies only while the
  destination stays unreachable from the source once the direct station
  edge is removed, so cross-connected same-tool pairs keep separate
  numbered stops (`GATK (2)` onward) and the station graph stays acyclic.

- **`module`** — one station per module section: the publication/overview
  tier. Stations are the workflow's own module namespaces (SCC-contracted
  where they reference each other cyclically), ordered by the section DAG
  and colored by each section's dominant stage line. This lands dense
  ports in the published-map scale without any hand curation (live:
  community mag, 642 stations at rule granularity → 10 at module
  granularity, rendered at 760×362 px vs the upstream map's 413×260).

Every tier comes out of the same `to_metro` pipeline (stage inference →
section assignment → collapse → SCC contraction → topological ordering),
so the tiers never disagree about structure — only about zoom.

## The format matrix

`-f` selects the output format; the granularity ladder selects the zoom.
Not every combination is meaningful — the matrix below shows the pairs
each persona actually uses:

| Format | Purpose | Typical `--granularity` | Consumers |
|---|---|---|---|
| `ascii` | one-glance terminal DAG | — | operator |
| `tree` | indented dependency tree | — | operator |
| `dot` | graphviz input | `--expanded` for the runtime DAG | auditor, CI |
| `dot-clustered` | graphviz input grouped by workflow modules | — | auditor |
| `mermaid` | plain Mermaid `graph LR` for docs | — | documentation |
| `metro` | nf-metro transit map (SVG/HTML) | `rule` / `process` / `module` | learner, reviewer, publication, site |

**Constraint:** `--granularity` applies to `metro` only. With any other
format the CLI rejects it before the workflow is parsed — a zoom setting is
never silently ignored (the default is `rule`, so plain `-f metro` and
`-f metro --granularity rule` are identical). The rejected-combination error
names the option and the value, and tells the user to pass `-f metro` or drop
the option.

## Engine design invariants

The exporter is generic by construction — it knows nothing about any
particular workflow:

1. **Shared knowledge only.** Stage inference (shell keywords), tool
   display names, curated module titles, and stage colors live in one
   table (`stage.rs`) that every workflow consults. No rule name, module
   name, or pipeline name is special-cased anywhere in `dag.rs`.
2. **Deterministic output.** Stations follow the rule file order inside a
   topological ordering; every tie-break is documented. The same workflow
   file always produces byte-identical exports — a precondition for CI
   regeneration and diffs.
3. **Topological ordering everywhere.** Sections are Kahn-ordered over
   the section graph; stations are Kahn-ordered over the intra-section
   station subgraph (ties by file order). A left-to-right transit map
   needs producers left of consumers; file order alone can violate
   dataflow (ported workflows group rules by module), and nf-metro
   rejects backward intra-section edges as routing defects.
4. **Cycle-safe contraction.** Module sections that reference each other
   cyclically merge into one section (deterministic id, `" + "`-joined
   display) via Kosaraju SCCs — the contracted section graph is always
   acyclic without dropping any station or edge.
5. **Router-friendly shapes.** Edges are deduplicated per station pair;
   isolated stations go off-track (`%%metro off_track`); labels are
   sanitized for Mermaid and nf-metro alike.

## Evaluation

Every change to the exporter is evaluated against the full 24-pipeline
community corpus, not against single examples:

1. **Render matrix** — all 24 workflows are exported at all three
   granularities and rendered with nf-metro; every export must be a valid
   mmd and every module-tier export must render (the publication tier
   must never abort).
2. **Structural metrics** — canvas size, aspect ratio, station count, and
   text-overlap count are compared against the upstream nf-core maps for
   the seven pipelines that have one (atacseq, chipseq, mag, methylseq,
   rnaseq, scrnaseq, viralrecon); the target is the same order of
   magnitude at the module tier.
3. **Visual QA** — rendered maps are inspected per persona (the gallery
   QA page compares ours side-by-side with the upstream reference where
   one exists).

The render recipes (line-spread modes, spacing directives, and the
curve-invariant fallback for process-tier maps whose density exceeds what
nf-metro's router currently tables) live with the automation script, not
in the engine: they are rendering policy, and the engine must stay a
faithful, generic DAG exporter.

## Known boundary

The engine promises structurally valid, deterministic mmd — the ladder,
the cycle-safety, and the topological ordering are engine-side invariants.
Rendering success is nf-metro's concern, and its router has limits: for very
dense `rule`- or `process`-tier maps (live: community enrichment,
varlociraptor) nf-metro 1.1.0 aborts with a `CurveInvariantError` instead of
producing a broken picture — a package upgrade or the project's render-fallback
setup (above) is the fix. The `module` tier is the publication guarantee:
it renders for the whole corpus.
