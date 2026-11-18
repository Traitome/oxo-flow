# Custom Skills

oxo-flow's AI layer combines three skill mechanisms:

| Layer | Source | Status |
|---|---|---|
| Built-in bioSkills | 562 curated Agent Skills compiled into the binary | ✅ active (domain-matched injection, `lookup_skill`) |
| Pipeline knowledge graph | 78 workflow skills (nodes) / 465 transitions (edges), 254 of them carrying paper citations | ✅ active (`lookup_pipeline`) |
| Skill protocol docs | full SKILL.md document per documented pipeline-graph skill (77 of 78 nodes; the `shared` pseudo-node has none) | ✅ active (`lookup_doc`) |
| **User-defined skills** | `.skill.toml` or `SKILL.md` files you write | ✅ knowledge skills, ✅ MCP tool skills |

This page documents the user-defined layer.

---

## Discovery ≠ Activation

The security model has two strictly separated steps:

1. **Discovery** — `oxo-flow` scans `~/.oxo-flow/skills/` (user level) and
   `<project>/.oxo-flow/skills/` (project level) for `*.skill.toml` files.
   This is **read-only**: discovered skills are listed by `oxo-flow ai`
   (quick status), but nothing is loaded or executed.
2. **Activation** — a skill only takes effect when its name is explicitly
   declared in the workflow's `[ai]` section. This is the trust boundary:
   the same explicit-approval model used by MCP clients.

Activated **knowledge** skills contribute **prompt text only** — there is
zero code execution and no filesystem access. Activated **tool** skills do
register tools: each `mcp://` requirement in their `requires` list is
bridged as an `mcp_<server>_<tool>` tool via the MCP client (Streamable
HTTP only — the engine never spawns server processes). See
[Tool Skills via MCP](#tool-skills-via-mcp).

---

## Skill Format

A skill is a small TOML file named `<name>.skill.toml`:

```toml
# ~/.oxo-flow/skills/qc-expert.skill.toml
name = "qc-expert"
version = "1.0.0"
description = "Advises on FASTQ QC thresholds for WGS"
author = "Your Lab"            # optional
domains = ["qc", "wgs"]        # optional, free-form tags
skill_type = "knowledge"       # or "tool" (MCP bridge, see below)

prompt_additions = [           # optional — appended to the system prompt
  "Prefer fastp with --qualified_quality_phred 20 for human WGS reads.",
  "Require a QC report before alignment for clinical samples.",
]
```

| Field | Required | Notes |
|---|---|---|
| `name` | ✅ | Unique; must match the activation entry |
| `version` | ✅ | Semantic version |
| `description` | ✅ | Shown in `oxo-flow ai` (status output) |
| `skill_type` | ✅ | `"knowledge"` (prompt text) or `"tool"` (MCP bridge) |
| `author`, `domains`, `prompt_additions`, `requires`, `entry` | — | Optional |

Invalid manifests (missing required fields) are skipped during discovery,
never fatal.

---

## Activation

Declare the skills to activate in the workflow's `[ai]` section:

```toml
[ai]
enabled = true
skills = ["qc-expert", "somatic-review"]
```

Only discovered skills whose names appear in this list are activated.
Activated `prompt_additions` are appended to the system prompt of
AI-powered commands (`template --ai`, `dry-run --ai`, `validate --ai`,
`lint --ai`), each clearly headed with `## Skill: <name>`.

Project-level activation can also live in `<project>/.oxo-flow/ai.toml`
(the same `[ai]` table shape) so the workflow file stays clean.

---

## Inspecting Skills

```bash
# Lists discovered skills (user + project level) — no AI provider needed
oxo-flow ai
```

```
Custom skills:
  qc-expert (knowledge) — Advises on FASTQ QC thresholds for WGS
```

---

## SKILL.md Format (Alternative)

Skills may also be written in the emerging SKILL.md markdown standard —
a directory containing a `SKILL.md` file with YAML frontmatter and a
markdown body:

```
~/.oxo-flow/skills/
└── rnaseq-reviewer/
    └── SKILL.md
```

```markdown
---
name: rnaseq-reviewer
description: Reviews RNA-seq pipelines for strandedness mistakes
version: 1.2.0
---
# Guidance

Always check that featureCounts `-s` matches the library prep, and that
STAR `--sjdbOverhang` is read-length minus one.
```

Discovery scans both `~/.oxo-flow/skills/<name>/SKILL.md` and
`<project>/.oxo-flow/skills/<name>/SKILL.md`. The whole markdown body
becomes the skill's prompt content; activation works exactly like
`.skill.toml` (`[ai] skills = ["rnaseq-reviewer"]`). `.skill.toml`
remains the format for machine metadata (`requires` MCP endpoints,
`skill_type = "tool"`).

## Tool Skills via MCP

A `skill_type = "tool"` skill references an MCP server in `requires`:

```toml
# ~/.oxo-flow/skills/clinical-db.skill.toml
name = "clinical-db"
version = "1.0.0"
description = "Queries the clinical variants MCP server"
skill_type = "tool"
requires = ["mcp://127.0.0.1:5050/mcp"]
```

On activation, oxo-flow connects over **Streamable HTTP** (JSON-RPC
over POST with SSE response support), discovers the server's tools, and
registers each as `mcp_<server>_<tool>` in the agent's tool registry.
The engine never spawns MCP server processes — the server must already
be reachable over HTTP (stdio servers are not supported).

### Human Approval for Non-Read-Only Tools

MCP tools execute under the same approval policy as all agent tools:

- The server-asserted `annotations.readOnlyHint` is **advisory only** —
  it never authorizes auto-execution (a hostile server can claim it
  for an effectful tool).
- **Every MCP tool invocation is refused unless a human approves that
  specific call** — the AI prompts on the terminal (`Allow execution?
  [y/N]`); non-interactive sessions (CI, redirected stdin, the web
  server surfaces) always refuse.

Outbound network egress from a built-in tool is gated the same way
(#1123, audit AI-05). A model-initiated `fetch_url` is read-only in the
local sense — it mutates nothing — but it ships data off the machine, so
it is a **data-release decision** and never inherits the read-only
auto-approval:

- In an interactive session the AI prompts for that specific call
  (`Allow execution? [y/N]`), showing the destination URL.
- In a non-interactive session (CI, redirected stdin, the web
  chat/translate loop) the call is refused, and the model receives an
  error naming the destination origin — never the URL's path or query,
  and never any file content.
- An operator can pre-authorize automatic release to specific origins
  with `OXO_FLOW_AI_FETCH_TRUSTED_ORIGINS` — comma-separated exact
  `scheme://host[:port]` origins (default ports omitted, paths ignored,
  no wildcards or suffix matches; unparseable entries are warned about
  and ignored). This knob governs *data release*, not reachability: the
  SSRF screen and `OXO_FLOW_AI_FETCH_ALLOW` still apply independently.
  Unset or empty means no origin is pre-authorized.

The read half of the chain is unchanged: a scoped `read_file` still
auto-executes for project files, but a model cannot ship what it read to
an arbitrary endpoint without an explicit human decision (or an
operator-listed origin).

The endpoint itself is SSRF-screened at connect time: internal
address space (loopback, RFC1918/ULA, link-local, cloud-metadata ranges,
CGNAT 100.64/10, benchmarking 198.18/15, NAT64 `64:ff9b::/96`, 6to4
`2002::/16`) and site-local names (`localhost`, `.local`, `.internal`) are refused
unless the exact host is exempted via `OXO_FLOW_AI_FETCH_ALLOW` — the
same opt-out documented for `fetch_url`. A trusted in-house MCP server on
a private address must be listed there explicitly.

When the endpoint resolves through DNS, the vetted address is pinned for
the connection: the client connects to exactly what was screened,
closing the DNS-rebinding window between screening and connecting
(allowlisted hosts and IP literals skip DNS and need no pin). Redirect
hops are re-screened the same way — a hop that lands in internal space is
refused, and an approved hop connects to its own vetted address.

These controls narrow the attack surface but are not a complete trust
boundary. The generic `OXO_FLOW_MCP_TOKEN` is released only to origins the
operator explicitly approves (see below), and model-initiated outbound
fetches require an explicit data-release decision (see above), so an
auto-approved project read cannot be shipped out without one. Activate
only reviewed skills and MCP servers and treat this subsystem as
prototype-grade until the remaining audit gates close.

### Protocol Version and Authentication

On connect the client proposes the newest MCP protocol version it speaks
(`2025-06-18`) at `initialize` and uses whatever version the server
answers with; a server answering with a version this client does not
understand (`2025-06-18`, `2025-03-26`, `2024-11-05`) fails the session
closed instead of proceeding against an unvetted dialect. The negotiated
version is sent back as `MCP-Protocol-Version` on every subsequent
request. If the server terminates the session (HTTP 404 on a request that
carried `Mcp-Session-Id`), the client starts a new session with a fresh
`initialize` and retries the request once; a server that keeps expiring
sessions fails the call instead of looping.

MCP servers requiring bearer authentication read the token from the
environment — never from a skill manifest, which is attacker-influenced
text:

- `OXO_FLOW_MCP_TOKEN` — shared token. It is sent **only** to origins
  listed in `OXO_FLOW_MCP_TRUSTED_ORIGINS`; to any other endpoint it is
  withheld entirely.
- `OXO_FLOW_MCP_TOKEN_<SERVER>` — per-server token (server name
  uppercased, non-alphanumeric characters folded to `_`; e.g. server
  `fs-gdrive` → `OXO_FLOW_MCP_TOKEN_FS_GDRIVE`). Setting this variable is
  itself the operator's decision for that endpoint, so it applies without
  a trust-list entry.
- `OXO_FLOW_MCP_TRUSTED_ORIGINS` — comma-separated exact origins approved
  to receive the generic token (e.g.
  `https://mcp.example.com,http://127.0.0.1:9000`). Entries are
  `scheme://host[:port]`; the `mcp://` manifest spelling is also accepted
  (it denotes the plain-HTTP endpoint `mcp://` URLs resolve to, i.e.
  `mcp://fs-gdrive:9000` = `http://fs-gdrive:9000`). Paths are ignored,
  there are no wildcards or suffix matches, and an entry that cannot be
  parsed as an origin is ignored with a warning. Unset or empty means no
  origin is approved, so the generic token is never released.

The per-server variable wins when both are set. The `<SERVER>` name is
folded from the endpoint URL's host, port and path (everything after the
scheme — `mcp://fs-gdrive:9000/v1` → `OXO_FLOW_MCP_TOKEN_FS_GDRIVE_9000_V1`),
so two servers sharing a host on different paths get distinct variables.
Note that the identity is scheme-agnostic: `https://host:9000` and
`http://host:9000` fold to the same `<SERVER>` name, so a per-server token
is reachable over both schemes on that host — prefer `https://` endpoints.

Tokens are attached as `Authorization: Bearer …` on every request to the
endpoint's origin and are never logged (not even a prefix); a redirect to a
different origin (checked per hop) carries no `Authorization`, session or
protocol headers.

Because the token rides `Authorization` on every request, prefer
`https://` endpoints when a token is configured — a plain-`http://`
endpoint sends the credential unencrypted on the wire.

### Bounded Agent Tools

The agent's built-in `read_file` tool refuses files larger than **256 KiB**
— a model-directed read of a huge data file would otherwise be
buffered whole and dumped into the model transcript (an OOM and
prompt-injection vector). The error steers the model to targeted reads
(`head`, `grep` over a shell channel) or a smaller summary file.
(`fetch_url` carries the same bound under its 256 KiB response cap.)

## Roadmap

- SKILL.md `metadata.allowed-tools` and license fields may be honored in a
  future release once the ecosystem settles on their semantics.
