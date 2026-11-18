# Build a Workflow on the Web Canvas

The web editor turns the `.oxoflow` TOML format into an interactive node
canvas. The TOML stays the single source of truth: every canvas action goes
through the engine's parse → edit → format → validate round-trip, so what you
see on the canvas is exactly what the CLI would run.

---

## Guided vs Canvas modes

The editor offers two views for the same workflow (TOML stays the single
source of truth):

- **Guided** (default for new users): one form card per rule — name,
  command, inputs, outputs, threads, memory, environment. Every change is
  converted to workflow TOML immediately; cards round-trip through canvas
  or AI edits (the backend parser returns each rule's shell).
- **Canvas + TOML**: the graph editor and raw TOML, for power users.

The choice persists across sessions. Validation errors now carry line
numbers; clicking a `line N` chip scrolls the TOML editor to the failing
line.

### Guided mode refuses documents it cannot represent

Guided mode models the `[workflow]` name/version and, per rule, `name`,
`shell`, `input`, `output`, `threads`, `memory`, and `environment` only.
It would silently drop or rewrite anything else — `[[values]]`, `[engine]`,
`[config]`, `when` gates, `depends_on`, hooks/retries, `script` bodies,
resources other than threads/memory, `[rules.*]` sub-tables, and map or
directory forms of `input`/`output` (issue #1143, audit UX-01).

When the loaded workflow contains any of those, Guided mode replaces the
form with a refusal that names every construct found, and its **Save**
refuses without writing anything. Use **Switch to TOML editor** (or the
**Canvas + TOML** mode button) to edit and save the full document — the
TOML editor path is unchanged and never strips advanced semantics.
`[workflow]` keys such as `description` are preserved and do not trigger
the refusal.

Table-header spelling is normalized to TOML semantics: `[[rules]]`,
`[[ rules ]]`, `[rules."resources"]`, dotted keys, inner whitespace, and
CRLF documents are read identically, and header-looking lines inside
multi-line strings stay string content. A line that starts like a table
header but cannot be parsed fails closed (the guided form refuses the
document) instead of being silently absorbed into the previous section.

## Canvas edits preserve your comments

Canvas, palette, and inspector edits are position-aware: the backend
patches the workflow TOML in place (via `toml_edit`), so hand-written
comments, blank lines, and formatting survive every edit — including
`remove_rule`/`connect`/`disconnect`/`add_rule` operations. Only the
edited rule's keys change; untouched rules keep their exact bytes.

## History

The left-rail History tab lists every saved revision (up to 50): load a
snapshot into the editor or roll the pipeline back — the current version
is preserved as a new revision first.

## The three panes

| Pane | What it does |
|------|--------------|
| **Tools / Assistant rail** (left) | Search the embedded Bioconda database (6,100+ tools) and add real tools as rules; or ask the AI assistant to draft a workflow |
| **Pipeline DAG** (center) | The node canvas: drag, connect, delete, double-click to edit (Metro Rule granularity adds a keyboard path — see [Metro view](#metro-view-and-grouped-granularities)) |
| **Workflow TOML** (right) | The canonical `.oxoflow` text — always in sync with the canvas |

## AI assistant: proposals require acceptance

The **Assistant** tab drafts workflows with the configured AI provider. Its
output is never written into the editor directly: each generated pipeline is
staged as a **proposal** with a line diff against the current editor content
(insertions green, deletions red). The editor keeps its exact content —
including unsaved hand edits — until **Apply to editor** is clicked;
**Discard** leaves it byte-identical (audit UX-05, issue #1147).

- A proposal that arrives while another is pending is queued, not swapped in:
  the panel keeps showing the pending proposal and reports `Proposal 1 of N`;
  **Review next proposal** switches between them explicitly.
- **Apply to editor** consumes only the accepted proposal. Any other queued
  proposals survive with their stable ordinals (`Proposal 2 of 2`) and remain
  reviewable; **Discard** likewise removes only the proposal on screen.
- Navigating away from the editor never applies a pending proposal; proposals
  are per-editor-session state and are discarded when the editor unmounts.
- The diff is recomputed against the live editor content, so if you keep
  editing while a proposal is pending, **Apply** states exactly what changes.
- An accepted proposal replaces the editor content as one edit; the usual
  debounced validation and (for a saved pipeline) **Save** flow applies
  afterwards — acceptance does not save or run anything.

## Adding a rule from the tool palette

1. Open the **Tools** tab and search (e.g. `fastp`).
2. Click **+** on a result. A new node appears with the skeleton command
   (`<tool> {input} -o {output}` — a starting point to edit, not a real
   invocation) and the tool's real name and version in its description —
   never a stub.

## Editing a rule

Double-click a node to open the inspector:

- **Name** — rename the rule; every `depends_on`, `extends`, and
  `scatter.gather` reference is rewritten to the new name (duplicates are
  rejected).
- **Shell command** — free-form, with `{input}`/`{output}` placeholders.
- **Inputs / Outputs** — file path patterns; wildcards (`{sample}`) allowed.
- **Environment** — system / conda / mamba / docker / singularity / venv /
  modules, with the spec string (`pixi` is not offered in the dropdown — a
  rule that already uses it stays editable via the TOML pane, and other
  unknown backend keys surface as `conda` with the full spec preserved).
- **Resources** — threads, memory, GPU, disk, time limit.
- **Conditions, retries, tags, logs, benchmarks** — the workflow format's
  execution controls.

Fields the inspector doesn't cover stay editable in the TOML pane.

The standard DAG view is the mouse-first editing canvas: double-click a node
to open the inspector, drag between handles to connect, and select elements
then press `Delete`/`Backspace` to remove them. A complete keyboard edit
surface exists in the **Metro view at Rule granularity** — focus order,
activation, connect, delete, and Escape behavior are documented under
[Metro view and grouped granularities](#metro-view-and-grouped-granularities)
(audit UX-06, issue #1148).

## Connecting rules

Drag from a node's right handle to another node's left handle. This adds an
explicit `depends_on` edge. In the **Metro view at Rule granularity** the
keyboard equivalent is:

1. Focus the source station and press `c` — the editor announces "Connecting
   from <rule>" and outlines the source station.
2. `Tab`/`Shift+Tab` to the target station and press `Enter` to write the
   connection. The live region reports the settled outcome after the edit
   round-trip ("Connected …" or "… was not applied").
3. Alternatively press `Escape` to cancel without writing anything.

Two connections are always rejected, before anything is written:

- connecting a node to itself, and
- any edge that would close a cycle — including cycles formed with
  file-inferred edges. The editor shows the backend's error and the
  workflow stays valid.

There are two kinds of edges, and the canvas draws them differently:

| Style | Kind | How to change it |
|-------|------|------------------|
| Solid | `depends_on` (declared) | Drag handles, or edit the TOML |
| Dashed | file-inferred | Edit the `input`/`output` paths — the engine infers the edge when an output pattern matches an input pattern |

The distinction matters: dashed edges are inferred from the
`input`/`output` paths — exact literal paths always connect, and wildcard
patterns connect too when they overlap the same files (for example
`aligned/{sample}.bam` feeding `dedup/{sample}.bam`). Patterns that can't
be resolved against each other leave the ordering ambiguous — use
`depends_on` to make it explicit.

## Deleting

Select one or more nodes and press `Delete` (or `Backspace`) to remove rules;
with an edge selected instead, the same keys disconnect it (file-inferred
edges are managed through the `input`/`output` paths, not by disconnect).
In the Metro view at **Rule** granularity, focus a single station and press
`Delete`/`Backspace` to remove exactly that rule (grouped Process/Module
stations ignore those keys — see [Metro view](#metro-view-and-grouped-granularities)).
Removing a rule whose outputs other rules consume will surface validation
errors on those rules — fix their inputs, and the workflow is valid again.

## Metro view and grouped granularities

**Metro View** (bottom-right of the canvas) draws the DAG as a transit map,
with a `Rule` / `Process` / `Module` granularity switch:

- **Rule** — one station per rule. This is the Metro editing surface: the
  keyboard actions below plus the mouse path (double-click to edit, drag
  handles to connect).
- **Process** — stations aggregate rules by environment backend (for example
  one `system` station containing every system rule).
- **Module** — stations aggregate rules by inferred bioinformatics modality
  (genomics, transcriptomics, epigenomics, metagenomics, infrastructure).

### Keyboard editing (Metro view, Rule granularity)

In the Metro view at **Rule** granularity, stations are the canvas's focus
stops, in rule-declaration order (`Tab` moves to the next station,
`Shift+Tab` to the previous one; edges are not focus stops there):

- `Enter` or `Space` — open the inspector for the focused rule.
- `Escape` — cancel a pending connection, or close the inspector and return
  focus to the station it came from.
- `c` — start a connection from the focused station (see
  [Connecting rules](#connecting-rules)).
- `Delete` / `Backspace` — remove the focused rule. The live region reports
  the settled outcome ("Removed rule …" or "… was not removed") after the
  edit round-trip.

These shortcuts (including the connect flow) are Metro Rule-granularity
features; the standard DAG view keeps its mouse-first editing path.

**Process and Module are read-only** (audit UX-06, issue #1148). Their
station ids are aggregation keys, not rule names, so a group operation cannot
be proven to preserve execution semantics — and a rule literally named like a
group key (for example `system`) would be silently edited instead of the
group. Grouped stations are therefore not buttons, carry a `read-only` badge,
keep their transit edges but expose only non-connectable handles, ignore
`Delete`/`Backspace`, and announce why when activated from the keyboard.
The toolbar shows the same note. Switch back to **Rule** granularity to edit
or connect. Grouping only changes the visualization — it never rewrites the
workflow document.

## Auto layout

Use **Auto layout** to re-run the layered Sugiyama layout; then drag nodes
wherever you like — positions are saved per pipeline (presentation only, never
part of the workflow file).

## Validation, dry-run, run

The badge above the TOML pane reports the engine's verdict live. **Dry-Run**
produces the execution plan without running anything; **Run** starts the real
pipeline and opens the monitor. Both honor the engine's checkpoint semantics:
re-runs reuse completed rules unless inputs, config, or rule definitions
changed.

The verdict is fail-closed (audit UX-03, issue #1145). When the validation
request itself fails — a network error or a server 5xx — the editor does not
keep the previous "Valid" result: the badge switches to **Not validated**,
**Save** refuses to report a save whose content was never validated, and
**Dry-Run**/**Run** stay disabled until a validation request actually
succeeds. A validation that returns findings behaves as before: the badge
shows the error count, the error list opens, and the save is refused with
that count. Recovery is automatic — the next successful validation (a new
edit or another **Save**) restores the run affordances.

## Look & feel

Dark mode (follows the OS or the header 🌙 toggle), Chinese interface
chrome (header 中文 toggle), and bilingual glossary tooltips on core
terms (pipeline, rule, wildcard, checkpoint, dry-run, depends_on).

## Browser storage and account switching

The editor keeps unsaved work in the browser so a reload or a crash does
not lose it. That state is **scoped to the account that created it**:

- **What is scoped**: the draft workflow TOML, the AI chat transcripts, and
  the canvas node positions. Each is stored under the username the server
  reports for the current session (`POST /api/auth/login` and
  `GET /api/auth/me`), in a bucket like `oxo_session::user:<username>`; a
  browser with nobody signed in uses a separate `guest` bucket.
- **Signing out clears all of it** — every account's bucket, not just the
  current one, plus the cached identity and the session token. A second
  account that signs in on the same browser starts from its own (empty)
  bucket and never sees the previous account's draft, chat, or canvas.
- **A rejected or expired session keeps your draft**: the token and the
  cached identity are dropped, but the bucket is only removed by an
  explicit sign-out, so signing back in as the same user restores the
  unsaved draft.
- **The cached identity is confirmed before anything renders**: when the
  browser holds a session token (or claims an identity), the app asks
  `GET /api/auth/me` before the first paint and restores **only** the bucket
  the server names. A stale or forged cached identity is corrected without
  ever rendering the wrong account's draft.
- **If the server cannot confirm the account, the app fails closed**: when
  the identity check fails (unreachable) or does not answer within a few
  seconds, no draft is shown at all — the cached identity is never used as a
  fallback. Instead you get a **Session not confirmed** notice with
  **Retry** and **Sign in again**. A late confirmation is adopted as soon as
  it arrives, so a slow server resolves to your own draft without a reload.
  Signing in again drops the token and the cached identity but keeps every
  saved draft, which returns with the same account.
- **Other tabs follow**: signing out (or in) in one tab re-initializes the
  same browser's other open tabs, and the sign-out wipe is authoritative —
  a background tab cannot re-save the signed-out account's state.
- **What is not scoped**: per-device preferences — theme, language, sidebar
  collapse, guided/canvas editor mode, and the onboarding banner. They
  describe the device, not the account.

Drafts saved by versions before this scoping existed were browser-wide and
cannot be attributed to an account, so they are discarded on first load.
Exports and saved pipelines are unaffected (they live on the server).

## See also

- [DAG Edit API](../reference/dag-edit-api.md) — the command API behind the canvas
- [Workflow Format](../reference/workflow-format.md) — full rule field reference
- [DAG Engine](../reference/dag-engine.md) — how dependencies are inferred
