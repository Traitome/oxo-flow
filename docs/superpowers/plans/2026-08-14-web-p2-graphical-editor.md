# P2: Graphical Workflow Editor — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Real node-based graphical programming of `.oxoflow` workflows: a React Flow canvas (create/connect/edit rules) round-tripping through the core engine, with edge kinds (declared vs file-inferred), a grounded tool palette, and monitor reuse.

**Architecture:** Backend first (TDD): extend the dag command API to full rule editing via TOML-patch semantics, tag DAG edges by kind, expose in-memory knowledge search. Then the frontend: replace cytoscape with `@xyflow/react` v12 + d3-dag, one canvas component for editor and monitor, inspector panel + palette fed by the knowledge endpoints.

**Tech Stack:** Rust (toml crate for value-level patching), @xyflow/react ^12, d3-dag, existing CodeMirror TOML pane stays.

**Spec:** `docs/superpowers/specs/2026-08-14-web-full-lifecycle-design.md` §6.2.

## Global Constraints

- Same CI gate as P0 (`make ci` per task: fmt + clippy -D warnings + workspace tests).
- TOML is the single source of truth: every edit round-trips parse → mutate → `format_workflow` → validate.
- Conventional commits; TDD red→green per task.
- `-p oxo-flow-web` tests: avoid `sqlite::memory:` (per-connection DBs), use `sqlite:{path}?mode=rwc`; no `block_on` inside tokio tests (see P0 memory notes).

---

### Task 1: Dag command API — full rule editing (backend)

**Files:**
- Modify: `crates/oxo-flow-web/src/domains/dag/service.rs` (execute_edit operations)
- Modify: `crates/oxo-flow-web/Cargo.toml` (add `toml = { workspace = true }`)
- Test: inline tests in service.rs + 2 integration cases in `crates/oxo-flow-web/tests/phase4_v09_integration.rs`

**Interfaces:**
- Consumes: `oxo_flow_core::{Rule, WorkflowConfig}`, `format_workflow`, existing `validate_pipeline`.
- Produces (new operation semantics, existing envelope unchanged):
  - `add_rule` payload `{ rule: {…rule table…} }` (full field support; legacy `{name, shell}` still accepted)
  - `update_rule` payload `{ name, patch: {…} }` — top-level keys of the rule table are replaced by patch keys (nested tables like `resources`/`environment` replace wholesale; the client sends complete sub-objects)
  - `update_workflow` payload `{ patch: {…} }` — patch keys replace top-level TOML sections (`workflow`, `config`, `defaults`); clients send complete sections
  - `update_params` remains as an alias of `update_rule`

- [ ] **Step 1: Write the failing tests** — append to the service.rs tests module:

```rust
    #[test]
    fn add_rule_with_full_spec() {
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "add_rule".into(),
            payload: serde_json::json!({
                "rule": {
                    "name": "fastp_trim",
                    "description": "Trim adapters",
                    "input": ["raw/{sample}_R1.fastq.gz", "raw/{sample}_R2.fastq.gz"],
                    "output": ["trimmed/{sample}_R1.fastq.gz", "trimmed/{sample}_R2.fastq.gz"],
                    "shell": "fastp --in1 {input[0]} --in2 {input[1]} --out1 {output[0]} --out2 {output[1]}",
                    "environment": {"conda": "envs/fastp.yaml"},
                    "resources": {"threads": 8, "memory": "16G"},
                    "retries": 2
                }
            }),
        };
        let r = execute_edit(TEST_TOML, "full-spec", &cmd).unwrap();
        assert!(r.success, "{:?}", r.validation_errors);
        let config = WorkflowConfig::parse(&r.toml_content).unwrap();
        let rule = config.rules.iter().find(|r| r.name == "fastp_trim").unwrap();
        assert_eq!(rule.input, vec!["raw/{sample}_R1.fastq.gz".to_string()]);
        assert_eq!(rule.retries, 2);
        assert_eq!(rule.resources.threads, Some(8));
    }

    #[test]
    fn update_rule_patches_fields_without_touching_others() {
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "update_rule".into(),
            payload: serde_json::json!({
                "name": "s1",
                "patch": {
                    "input": ["data/in.fastq"],
                    "output": ["res/out.txt"],
                    "shell": "process {input} > {output}"
                }
            }),
        };
        let r = execute_edit(TEST_TOML, "patch-1", &cmd).unwrap();
        let config = WorkflowConfig::parse(&r.toml_content).unwrap();
        let s1 = config.rules.iter().find(|r| r.name == "s1").unwrap();
        assert_eq!(s1.shell.as_deref(), Some("process {input} > {output}"));
        assert_eq!(s1.input, vec!["data/in.fastq".to_string()]);
    }

    #[test]
    fn update_rule_rejects_unknown_rule() {
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "update_rule".into(),
            payload: serde_json::json!({"name": "nope", "patch": {"shell": "x"}}),
        };
        assert!(execute_edit(TEST_TOML, "patch-2", &cmd).is_err());
    }

    #[test]
    fn update_workflow_replaces_sections() {
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "update_workflow".into(),
            payload: serde_json::json!({
                "patch": {"workflow": {"name": "renamed", "version": "2.0.0", "description": "d"}}
            }),
        };
        let r = execute_edit(TEST_TOML, "wf-patch", &cmd).unwrap();
        let config = WorkflowConfig::parse(&r.toml_content).unwrap();
        assert_eq!(config.workflow.name, "renamed");
        assert_eq!(config.workflow.version, "2.0.0");
    }
```

  Integration (phase4 file, follows its existing `app()` helper): `POST /api/pipeline/editor-1/command` with the `add_rule` full-spec payload above → 200, `success: true`, `toml_content` contains `fastp_trim` and `fastp --in1`.

- [ ] **Step 2: Run to verify failure**
  Run: `cargo test -p oxo-flow-web domains::dag` + the phase4 case
  Expected: `update_rule`/`update_workflow` → "Unknown operation"; `add_rule` full spec → only name+shell survive (input/output assertions fail).

- [ ] **Step 3: Implement**

```rust
/// Convert a serde_json::Value into a toml::Value (all JSON types map to the
/// TOML equivalents; null is not representable — caller must filter it).
fn json_to_toml(v: &serde_json::Value) -> toml::Value {
    match v {
        serde_json::Value::String(s) => toml::Value::String(s.clone()),
        serde_json::Value::Integer(i) => toml::Value::Integer(*i),
        serde_json::Value::Float(f) => toml::Value::Float(*f),
        serde_json::Value::Boolean(b) => toml::Value::Boolean(*b),
        serde_json::Value::Array(a) => toml::Value::Array(a.iter().map(json_to_toml).collect()),
        serde_json::Value::Object(o) => toml::Value::Table(
            o.iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), json_to_toml(v)))
                .collect(),
        ),
        serde_json::Value::Null => toml::Value::String(String::new()),
    }
}
```

  In `execute_edit`, add a shared post-edit tail — after any operation, the flow becomes:
  `let new_toml = format_workflow(&config);` (unchanged) → validate → respond (unchanged). The operations themselves:

```rust
        "add_rule" => {
            if let Some(rule_val) = command.payload.get("rule") {
                // Full rule table: append as a new [[rules]] entry.
                let mut doc: toml::Value =
                    toml::from_str(toml_content).map_err(|e| format!("TOML: {e}"))?;
                let rules = doc
                    .get_mut("rules")
                    .and_then(|v| v.as_array_mut())
                    .ok_or("workflow has no [[rules]] array")?;
                rules.push(json_to_toml(rule_val));
                let patched = toml::to_string(&doc).map_err(|e| format!("TOML: {e}"))?;
                config = WorkflowConfig::parse(&patched).map_err(|e| format!("Parse: {e}"))?;
            } else {
                // Legacy shape: name + shell only.
                let name = command.payload["name"].as_str().unwrap_or("new_rule").to_string();
                let shell_val = command.payload["shell"].as_str().unwrap_or("echo 'new step'").to_string();
                config.rules.push(Rule { name, shell: Some(shell_val), ..Default::default() });
            }
        }
```

```rust
        "update_rule" | "update_params" => {
            let name = command.payload["name"].as_str().ok_or("Missing rule name")?;
            let patch = command
                .payload
                .get("patch")
                .ok_or("update_rule requires a 'patch' table")?;
            let mut doc: toml::Value =
                toml::from_str(toml_content).map_err(|e| format!("TOML: {e}"))?;
            let rules = doc
                .get_mut("rules")
                .and_then(|v| v.as_array_mut())
                .ok_or("workflow has no [[rules]] array")?;
            let rule = rules
                .iter_mut()
                .find(|r| r.get("name").and_then(|n| n.as_str()) == Some(name))
                .ok_or_else(|| format!("Rule '{name}' not found"))?;
            let table = rule.as_table_mut().ok_or("rule entry is not a table")?;
            let patch_table = patch.as_object().ok_or("patch must be a table")?;
            for (k, v) in patch_table {
                table.insert(k.clone(), json_to_toml(v));
            }
            let patched = toml::to_string(&doc).map_err(|e| format!("TOML: {e}"))?;
            config = WorkflowConfig::parse(&patched).map_err(|e| format!("Parse: {e}"))?;
        }
```

```rust
        "update_workflow" => {
            let patch = command.payload.get("patch").ok_or("update_workflow requires a 'patch' table")?;
            let mut doc: toml::Value =
                toml::from_str(toml_content).map_err(|e| format!("TOML: {e}"))?;
            let table = doc.as_table_mut().ok_or("workflow TOML is not a table")?;
            let patch_table = patch.as_object().ok_or("patch must be a table")?;
            for (k, v) in patch_table {
                table.insert(k.clone(), json_to_toml(v));
            }
            let patched = toml::to_string(&doc).map_err(|e| format!("TOML: {e}"))?;
            config = WorkflowConfig::parse(&patched).map_err(|e| format!("Parse: {e}"))?;
        }
```

  (`config` must become `let mut config` at the top of `execute_edit`; the removed legacy `add_rule`/`update_params` bodies are replaced by the arms above. The legacy `update_params` body's threads/shell handling is superseded — `update_params` with `{name, threads, shell}` legacy payload: convert internally `let patch = if payload.get("patch").is_none() { json!({"threads": ..., "shell": ...}) }` — keep both shapes working by building the patch from legacy keys when `patch` is absent.)

- [ ] **Step 4: Run to verify pass** + clippy + fmt.
- [ ] **Step 5: Commit** `feat(web): dag edit API supports full rule specs via TOML-patch update_rule`

---

### Task 2: Edge kinds in `/api/pipelines/dag` (backend)

**Files:**
- Modify: `crates/oxo-flow-web/src/domains/workflow/types.rs` (`DagJsonEdge` + `kind: String`)
- Modify: `crates/oxo-flow-web/src/domains/workflow/service.rs` (`build_dag` — classify each edge)
- Test: inline service test + phase1 integration assertion

**Interfaces:**
- Consumes: parsed `WorkflowConfig` rules (each rule's `depends_on`).
- Produces: `DagJsonEdge { from, to, kind }` where `kind == "declared"` iff `to.depends_on` contains `from`, else `"file"`.

- [ ] **Step 1: Write the failing test** — in service.rs tests:

```rust
    #[test]
    fn dag_edges_carry_kind() {
        let toml = "[workflow]\nname = \"e\"\n\n[[rules]]\nname = \"a\"\ninput = [\"x\"]\noutput = [\"a.txt\"]\nshell = \"echo a > a.txt\"\n\n[[rules]]\nname = \"b\"\ninput = [\"a.txt\", \"y.txt\"]\noutput = [\"b.txt\"]\nshell = \"cat a.txt > b.txt\"\ndepends_on = [\"c\"]\n\n[[rules]]\nname = \"c\"\ninput = [\"y.txt\"]\noutput = [\"y.out\"]\nshell = \"cat y.txt > y.out\"\n";
        let dag = build_dag(toml).unwrap();
        let file_edge = dag.edges.iter().find(|e| e.from == "a" && e.to == "b").expect("a→b inferred");
        assert_eq!(file_edge.kind, "file");
        let declared_edge = dag.edges.iter().find(|e| e.from == "c" && e.to == "b").expect("c→b declared");
        assert_eq!(declared_edge.kind, "declared");
    }
```

- [ ] **Step 2: Run to verify failure** — missing `kind` field.
- [ ] **Step 3: Implement** — in `build_dag`'s edge construction:

```rust
    let edges: Vec<DagJsonEdge> = config
        .rules
        .iter()
        .flat_map(|r| {
            dag.dependencies(&r.name)
                .unwrap_or_default()
                .into_iter()
                .map(|dep| DagJsonEdge {
                    from: dep.clone(),
                    to: r.name.clone(),
                    kind: if r.depends_on.contains(&dep) { "declared" } else { "file" }.to_string(),
                })
                .collect::<Vec<_>>()
        })
        .collect();
```

- [ ] **Step 4/5: verify + commit** `feat(web): dag edges tagged file vs declared`.

---

### Task 3: Knowledge endpoints for the tool palette (backend)

**Files:**
- Modify: `crates/oxo-flow-web/src/domains/ai/handlers.rs` (two GET handlers)
- Modify: `crates/oxo-flow-web/src/server.rs` (routes `GET /api/knowledge/tools`, `GET /api/knowledge/skills` — mount OUTSIDE the auth middleware's protected set is unnecessary: personal mode has no auth; in team mode they're behind auth like other endpoints)
- Test: `crates/oxo-flow-web/tests/knowledge_integration.rs` (new file; no DB needed — memory-backed)

**Interfaces:**
- Consumes: `oxo_flow_ai::knowledge::bioconda::{search_tools, tool_count}`, `oxo_flow_ai::knowledge::skills::{search_skills, skill_count}`.
- Produces: `GET /api/knowledge/tools?q=<str>&limit=<n>` → `{ "total": <db size>, "tools": [{name, version, summary, platforms}] }`; `GET /api/knowledge/skills?q=` → `{ "total": …, "skills": [{name, domain, description, primary_tool}] }` (match `SkillRecord`'s actual fields).

- [ ] **Step 1: Write the failing tests**:

```rust
#[tokio::test]
async fn tools_search_returns_grounded_entries() {
    let app = oxo_flow_web::server::build_router("personal");
    let resp = app.oneshot(
        Request::builder().uri("/api/knowledge/tools?q=fastp&limit=5").body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert!(body["total"].as_u64().unwrap() > 6000, "Bioconda DB has 6103 tools");
    let tools = body["tools"].as_array().unwrap();
    assert!(!tools.is_empty());
    assert!(tools.iter().any(|t| t["name"].as_str().unwrap().contains("fastp")), "{body:?}");
}
```

  (skills analog: `q=variant` returns entries with a `domain` field; 404 shape: unknown → 200 with empty list, not error.)

- [ ] **Step 2: Run to verify failure** — 404 (routes absent).
- [ ] **Step 3: Implement** — handlers:

```rust
/// GET /api/knowledge/tools — search the embedded Bioconda tool database.
pub async fn knowledge_tools(Query(params): Query<KnowledgeQuery>) -> ApiResult<serde_json::Value> {
    let limit = params.limit.unwrap_or(20).clamp(1, 50);
    let tools = oxo_flow_ai::knowledge::bioconda::search_tools(&params.q, limit);
    Ok(Json(serde_json::json!({
        "total": oxo_flow_ai::knowledge::bioconda::tool_count(),
        "tools": tools.iter().map(|t| serde_json::json!({
            "name": t.name, "version": t.version, "summary": t.summary, "platforms": t.platforms,
        })).collect::<Vec<_>>(),
    })))
}
```

  with `#[derive(Deserialize)] pub struct KnowledgeQuery { q: String, limit: Option<usize> }` and a default `q=""` (Deserialize default). Server routes: `.route("/api/knowledge/tools", get(ai::handlers::knowledge_tools))` + skills analog, next to the existing `/api/ai/*` mounts.
- [ ] **Step 4/5: verify + commit** `feat(web): knowledge search endpoints for the editor palette`.

---

### Task 4: React Flow editor canvas (frontend)

**Files:**
- Modify: `frontend/package.json` (+`@xyflow/react`, +`d3-dag`; remove `cytoscape`, `cytoscape-dagre`, `cytoscape-edgehandles`, `@types/cytoscape`, `@types/cytoscape-edgehandles`)
- Create: `frontend/src/components/WorkflowCanvas.tsx` — React Flow canvas: nodes from `/api/pipelines/dag` (id, label, color, env badge), edges (solid = declared, dashed = file, file edges non-deletable), drag-to-arrange (positions in `localStorage` keyed by pipeline id), connect handles → `api.dagCommand(…, "connect")`, delete (Backspace/Delete on selected node) → `remove_rule`, double-click node → open inspector, auto-layout button (d3-dag layered, 200×80 node size), minimap + fitView.
- Create: `frontend/src/components/RuleInspector.tsx` — modal/side panel: name, description, shell (textarea), script, inputs/outputs (list editor, add/remove rows), environment (backend select: system/conda/mamba/docker/singularity/venv/modules + spec string), resources (threads, memory, gpu, disk, time_limit), envvars (key-value rows), when, retries, tags (comma string), optional/required (checkboxes), log, benchmark. Save → `api.dagCommand(…, "update_rule", {name, patch})` where patch contains ONLY changed fields (complete sub-objects for environment/resources/envvars).
- Create: `frontend/src/components/ToolPalette.tsx` — search input → `api.knowledgeTools(q)` (new client wrapper); result rows (name, version, summary); "Add" → `dagCommand add_rule {rule: {name, description: "<name> <version> — <summary>", input: [], output: [], shell: "<name> {input} -o {output}"}}` + select the new node.
- Modify: `frontend/src/api/client.ts` (+`knowledgeTools`, `knowledgeSkills` wrappers)
- Modify: `frontend/src/pages/PipelineEditor.tsx` — replace the cytoscape `DagView` pane with `WorkflowCanvas`; add `ToolPalette` as a collapsible left rail; keep the TOML pane (CodeMirror) and the live-validation badge; canvas edits and TOML edits both update `session.state.pipelineToml` (last-write-wins, 300 ms debounce unchanged); keep undo/redo buttons.
- Modify: `frontend/src/pages/MonitorReport.tsx` — DAG tab uses `WorkflowCanvas` in read-only mode (`readOnly` prop; status colors from `dag-status`); delete `DagView.tsx` (its PNG export + LR/TB toggle move into WorkflowCanvas as optional props if trivial, else drop).
- Test: `frontend/e2e/editor-canvas.spec.ts` — the acceptance scenario (§6.2 of the spec): canvas-built 3-rule workflow (palette → add 2 nodes → connect → inspector sets inputs/outputs) → validation badge green → dry-run dialog → save. Plus a guard test that the old cytoscape packages are gone (`package.json` grep in a vitest-less check — assert via the build).

- [ ] **Steps:** TDD is awkward for pure-UI work — implement component by component, then `npm run build` + `npx playwright test e2e/editor-canvas.spec.ts` as the verification gate; fix until green. Commit per component: `feat(frontend): WorkflowCanvas`, `feat(frontend): RuleInspector + ToolPalette`, `feat(frontend): monitor reuses canvas; drop cytoscape`.
- [ ] Final gate for the task: `npm run build` + the canvas e2e + full `make ci`.

---

### Task 5: P2 final gate

- [ ] Full `make ci` + `cd frontend && npm run build && npx playwright test` (all specs).
- [ ] Manual live check: `make dev`, build a workflow on the canvas, dry-run, save, run, watch the monitor DAG.
- [ ] Update `docs/guide/src/reference/dag-edit-api.md` (new operations + edge kinds) and add a short "Web editor" how-to page; commit as `docs:`.
