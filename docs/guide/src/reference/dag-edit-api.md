# DAG Edit API

The DAG Edit API enables programmatic manipulation of workflow DAGs — adding and removing rules, connecting and disconnecting dependencies, and full undo/redo support. This API powers the Web UI's interactive DAG editor.

---

## Overview

The DAG Edit domain (`crates/oxo-flow-web/src/domains/dag/service.rs`) provides a **command-queue architecture** with automatic TOML round-tripping and validation on every edit. Each edit is:

1. **Parsed** — the current TOML content is parsed into a `WorkflowConfig`
2. **Applied** — the command modifies the in-memory config
3. **Formatted** — the config is serialized back to canonical TOML
4. **Validated** — the result is validated through the workflow validation pipeline

Edits that produce invalid workflows are still applied and returned — the
response reports `success: false` with the `validation_errors` populated, so the
problematic state can be inspected and fixed with a follow-up edit.

## Endpoint

All edit commands are sent to a single endpoint, with the pipeline ID in the URL path:

```
POST /api/pipeline/{id}/command
```

The pipeline ID is a logical key for the undo/redo stacks — it does not need to
reference a saved pipeline.

## Commands

All commands share a common request envelope. The request body must include the
current TOML content (`toml_content`) plus the command.

> The commands below reflect the v0.11+ extended API: `add_rule` accepts a
> complete rule table, `update_rule` patches any rule fields through a TOML
> table (with `null` meaning "remove this key"), and `update_workflow`
> replaces top-level sections. `update_params` remains as a legacy alias of
> `update_rule`.

```json
{
  "toml_content": "<current workflow TOML>",
  "source": "dag_editor",
  "operation": "<command>",
  "payload": { ... }
}
```

`source` is one of `dag_editor` (the interactive editor), `chat`, or `proposal`.
`operation` is one of `add_rule`, `remove_rule`, `connect`, `disconnect`,
`update_params`, `replace_tool`, or `reorder`.

### `add_rule`

Add a new rule to the workflow.

**Payload:**

| Field | Type | Required | Description |
|---|---|---|---|
| `rule` | Table | Yes (full form) | Complete rule table — any rule field from the workflow format (input, output, shell, environment, resources, envvars, when, retries, tags, …) |
| `name` / `shell` | String | No (legacy form) | Legacy minimal shape, kept for compatibility |

**Example (full form):**

```json
{
  "toml_content": "<current workflow TOML>",
  "source": "dag_editor",
  "operation": "add_rule",
  "payload": {
    "rule": {
      "name": "fastp_trim",
      "description": "Trim adapters",
      "input": ["raw/{sample}_R1.fastq.gz", "raw/{sample}_R2.fastq.gz"],
      "output": ["trimmed/{sample}_R1.fastq.gz"],
      "shell": "fastp --in1 {input[0]} --out1 {output[0]}",
      "environment": {"conda": "envs/fastp.yaml"},
      "resources": {"threads": 8, "memory": "16G"}
    }
  }
}
```

**Result:** The new rule is appended to the rule list and core parsing
validates the field set (unknown fields are rejected with a parse error).

---

### `remove_rule`

Remove a rule and all references to it.

**Payload:**

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | String | **Yes** | Name of the rule to remove |

**Example:**

```json
{
  "toml_content": "<current workflow TOML>",
  "source": "dag_editor",
  "operation": "remove_rule",
  "payload": { "name": "obsolete_step" }
}
```

**Result:** The rule is removed from the rule list. All `depends_on` entries in other rules that reference this rule are cleaned up automatically.

---

### `connect`

Add an explicit dependency edge between two rules.

**Payload:**

| Field | Type | Required | Description |
|---|---|---|---|
| `from` | String | **Yes** | Name of the upstream rule (runs first) |
| `to` | String | **Yes** | Name of the downstream rule (runs after `from`) |

**Example:**

```json
{
  "toml_content": "<current workflow TOML>",
  "source": "dag_editor",
  "operation": "connect",
  "payload": { "from": "fastqc", "to": "trim_reads" }
}
```

**Result:** `"fastqc"` is added to `trim_reads`'s `depends_on` list. If the dependency already exists, the command is idempotent (no duplicate). Returns an error if the target rule doesn't exist.

---

### `disconnect`

Remove an explicit dependency edge between two rules.

**Payload:**

| Field | Type | Required | Description |
|---|---|---|---|
| `from` | String | **Yes** | Name of the upstream rule |
| `to` | String | **Yes** | Name of the downstream rule |

**Example:**

```json
{
  "toml_content": "<current workflow TOML>",
  "source": "dag_editor",
  "operation": "disconnect",
  "payload": { "from": "fastqc", "to": "trim_reads" }
}
```

**Result:** `"fastqc"` is removed from `trim_reads`'s `depends_on` list. File-based dependencies (inferred from input/output matching) are **not** affected — only explicit `depends_on` entries are managed by connect/disconnect.

---

### `update_rule`

Update any fields on an existing rule.

**Payload:**

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | String | **Yes** | Name of the rule to update |
| `patch` | Table | **Yes** | Rule fields to replace. Nested tables (`resources`, `environment`, `envvars`) replace wholesale — send complete sub-objects. A `null` value removes the key (e.g. dropping an environment) |

**Example:**

```json
{
  "toml_content": "<current workflow TOML>",
  "source": "dag_editor",
  "operation": "update_rule",
  "payload": {
    "name": "bwa_align",
    "patch": {
      "shell": "bwa-mem2 mem -t {threads} {config.reference} {input} | samtools sort -o {output}",
      "input": ["raw/{sample}_R1.fastq.gz", "raw/{sample}_R2.fastq.gz"],
      "resources": {"threads": 32, "memory": "64G"},
      "retries": null
    }
  }
}
```

**Result:** Patch keys replace the rule's fields; all unpatched fields are
preserved. `update_params` (`{name, threads, shell}`) remains as a legacy
alias.

### `update_workflow`

Replace top-level TOML sections (`workflow`, `config`, `defaults`, …). Patch
keys replace the corresponding section wholesale — send complete sections.

```json
{
  "toml_content": "<current workflow TOML>",
  "source": "dag_editor",
  "operation": "update_workflow",
  "payload": {
    "patch": {"workflow": {"name": "renamed", "version": "0.14.0", "description": "d"}}
  }
}
```

---

## Undo & Redo

The DAG Edit API maintains an undo/redo stack per pipeline ID with a maximum depth of 50 entries.

### `undo`

Reverts the last edit, restoring the previous TOML state.

```
POST /api/pipeline/{id}/undo
```

Returns the previous TOML content, or `404` with code `NO_UNDO` if the undo stack is empty.

### `redo`

Re-applies the last undone edit.

```
POST /api/pipeline/{id}/redo
```

Returns the redone TOML content, or `404` with code `NO_REDO` if the redo stack is empty.

**Note:** Performing a new edit after an undo clears the redo stack (standard undo/redo semantics).

---

## Response Format

All edit commands return a `DagEditResponse`:

```json
{
  "success": true,
  "toml_content": "[workflow]\nname = \"...\"\n...",
  "validation_errors": []
}
```

| Field | Type | Description |
|---|---|---|
| `success` | Boolean | `true` if validation passed (no hard errors) |
| `toml_content` | String | The complete workflow TOML after the edit |
| `validation_errors` | Array of String | Validation error messages (lint-level findings such as a missing description may appear even when `success` is `true`) |

If validation fails, `success` is `false` and the edit is **still applied** to the returned TOML (so the user can see the problematic state), with `validation_errors` populated.

Malformed commands (unknown operation, missing required payload fields such as the rule `name`, or a `connect`/`disconnect`/`update_params` target rule that does not exist) are rejected with HTTP `400` and code `DAG_EDIT_ERROR`; the edit is not applied.

---

## Important Notes

- **File-based edges are immutable via the edit API.** The `connect`/`disconnect` commands only manage `depends_on` entries. File-based dependencies (inferred from input/output matching) are controlled by the `input` and `output` fields of rules, which the edit API cannot currently modify.
- **All edits are validated.** The edit API runs the full workflow validation pipeline after every command. Edits that introduce cycles, duplicate names, or invalid syntax are flagged in `validation_errors`.
- **TOML round-tripping preserves formatting.** The core `format::format_workflow` function produces canonical TOML, so edited workflows will have consistent formatting regardless of the original style.
- **Undo/redo is in-memory.** Stacks are per-pipeline and live only for the duration of the server process. They do not persist across server restarts.

---

## See Also

- [DAG Engine](./dag-engine.md) — how the DAG is built and validated
- [Workflow Format](./workflow-format.md) — rule field reference
- [Web API](./web-api.md) — REST endpoint documentation
- [Web System Architecture](./web-system-architecture.md) — how the edit API fits into the Web UI
