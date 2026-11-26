# Cost-budget enforcement (issue #1400)

Branch: `feat/1400-cost-budget` (off `main` @ `8e6abda9`)

## Problem

Issue #1371 / PR #1408 added a normalized resource + cost accounting *ledger*
(`oxo-flow.resource-ledger/v1`) but accounting is not enforcement. A workflow can
consume arbitrary cloud/scheduler spend with no cap. Operators need a declared
budget that stops **new** work before the run exceeds it.

## Chosen design (per the issue's own triage: user-declared rates, not billing)

`[resource_budget]` already exists with hard-cap semantics (`max_threads`,
`max_memory`, `max_jobs`) enforced through `ResourceBudget::effective_caps` and
`check_budget_feasibility`. Extend it with a cost cap:

```toml
[resource_budget]
max_cost = { amount = 10.0, currency = "USD", cpu_second_rate = 0.0001, gb_second_rate = 0.00002 }
```

- `amount` (f64, > 0) — the cap, in `currency`.
- `currency` (string, non-empty) — ISO-ish label; compared for equality only.
- `cpu_second_rate` (Option<f64>) — cost per accrued CPU-second.
- `gb_second_rate` (Option<f64>) — cost per accrued GB-second
  (`max_rss_bytes / 1e9 * wall_time_secs` per rule, summed).

At least one rate must be present; `amount` and `currency` are required when
`max_cost` is present.

### Accrual (pure, in core)

Accrued cost is computed from the same ledger the report surfaces:

```
accrued = cpu_second_rate * sum(rule.cpu_seconds)
        + gb_second_rate   * sum(max_rss_bytes/1e9 * wall_time_secs)
```

New module `crates/oxo-flow-core/src/cost_budget.rs`:

- `CostBudget` (serde; lives in `config/model.rs` next to `ResourceBudget`).
- `CostAccrual { accrued: Option<f64>, currency, limit, cpu_seconds, gb_seconds,
  unknown_rules: Vec<String>, method: String }` — `accrued` is `None` (never 0)
  when a rule that a configured rate depends on lacks its metric; `unknown_rules`
  names them.
- `accrue_cost(rules: &[LedgerRule], budget: &CostBudget) -> CostAccrual`.

Rule: **unknown is never zero.** Absence of a metric makes `accrued = None`.

### Enforcement (scheduler dispatch gate, `scheduler_loop.rs`)

The dispatch gate is already the single place `available` is computed
(`available = 0` under disk Hold, else `jobs - running_count`; `to_submit =
ready.iter().take(available)`). Add the same shape:

1. Each dispatch iteration, compute `CostAccrual` from the checkpoint's
   `resource_usage` (per-rule metrics recorded on completion).
2. If `accrued >= limit` (or accrual is unknown and `on_unknown = "refuse"`):
   set `available = 0` — stop scheduling **new** ready rules.
3. Settle in-flight rules honestly (they run to completion; they are not killed).
4. When nothing is in flight and nothing new may dispatch, take the disk-pressure
   abort path as the template (save checkpoint, clear progress, emit a structured
   outcome, `bail!`): emit status `cost_budget_exceeded` with the method, the
   accrued number, the cap, per-rate contributions, and any unknown residual.

`on_unknown` (Option<String>, default `"refuse"`): `"refuse"` = fail closed when
accrual is unknown; `"report"` = keep running but record the unbounded residual in
the outcome and the ledger. A currency mismatch (a rule cost in a different
currency than the budget) is always fail-closed.

### Surfaces

- `dry-run` / `validate`: report the declared budget and the plan-time
  *unestimable* state (no run metrics yet) — never a fabricated estimate.
- `report --resource-ledger`: when the checkpoint's workflow declared
  `max_cost`, add `budget` + `cost_accrual` to the JSON (and a CSV column), with
  `accrued: null` + `unknown_rules` when it cannot be computed.
- Run JSON summary `status: "cost_budget_exceeded"` carries the structured detail.

### Schema / codegen / drift

- `RESOURCE_BUDGET_KEYS` (known_keys.rs:239) gains `"max_cost"` — which by the
  drift test (known_keys.rs:1328) *requires* the `resource_budget` block in
  `docs/schema/oxoflow-v1.schema.json` to gain a `max_cost` object.
- Re-run `npm run generate` in `editors/vscode` (CI drift-checks committed output).

### Docs (AGENTS.md change-management)

`workflow-format.md` (`[resource_budget]`), `run.md`, `report.md`,
`run-on-cluster.md`, `AGENTS.md`. `web-api.md` only if a route changes (it does
not: this is engine-side; the web spawn path forwards the CLI args unchanged).

## Tests

- core: accrual arithmetic; unknown-is-never-zero; currency mismatch; empty rules;
  rate absent; `on_unknown` both modes.
- cli: cap hit → `cost_budget_exceeded`, no new dispatch, in-flight settled;
  unknown accounting refuses / reports; legacy checkpoint without `resource_usage`;
  budget-free workflow unchanged (no behavior change).
- schema-drift + skill-drift + version-check (`make ci`).

## Cluster verification (`fr` = `202.197.87.243:8808`, user wangsx)

Minimal-resource check: submit a 2–3 rule workflow with a tiny `max_cost` and
assert the run stops observably with `cost_budget_exceeded` and the documented
method — the smallest reliable end-to-end evidence. No heavy pipelines.

Done — the committed evidence artifact is
[`docs/superpowers/audits/cost-budget-enforcement-2026-11-26.md`](../audits/cost-budget-enforcement-2026-11-26.md):
a control run (no budget → `completed`, 3/3) and a treatment run (`max_cost`
cap → `cost_budget_exceeded`, output directory holding only `step1.txt`),
both on the `fr` SLURM partition `TEST`.

## Out of scope

Authoritative billing, price catalogs, cross-run/org budgets, cost optimization.

## Campaign rules

- At most **2** concurrent subagents.
- Independent review before merge (project rule).
- Commit trailer: `Co-Authored-By: Claude Code <noreply@anthropic.com>`;
  PR body ends with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.
