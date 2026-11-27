# Cost-budget enforcement (#1400) — live cluster evidence

Live verification that `[resource_budget].max_cost` is enforced on the cluster
path, not only the local scheduler. This closes reviewer finding #1 on the
#1400 change set: the plan doc asserted live cluster evidence, and this file is
that committed artifact.

Method: the same minimal three-rule workflow is run twice on the `fr` SLURM
cluster — once **without** a budget (control) and once **with** an absurdly
small cap (treatment). The treatment must stop after the first rule with the
structured status `cost_budget_exceeded`; the control must complete all three.
The treatment's output directory must contain **only** `step1.txt`, proving the
later rules never executed (anti-fabrication: a status string alone would be
consistent with a run that actually completed).

## Environment

- Host: `fr` SLURM cluster, `wangsx@202.197.87.243` (SSH port 8808)
- Evidence root: `/gpfs/hpc/home/wangsx/wangsx/oxo-flow-slurm-test/cb1400`
- Partition: `TEST`
- Binary under test: `.../cb1400/../bin/oxo-flow-1400` (reports `oxo-flow 0.32.0`)
- Workflow: `cb1400-minimal` — three chained rules `step1 → step2 → step3`,
  each `threads = 1 / memory = "1G"`, each shell a `mkdir -p out && echo … >
  out/stepN.txt && sleep 4`.

## Workflow (as authored)

```toml
[workflow]
name = "cb1400-minimal"

[resource_budget]
max_cost = { amount = 0.0000001, currency = "USD", gb_second_rate = 1.0 }

[[rules]]
name = "step1"
shell = "mkdir -p out && echo one > out/step1.txt && sleep 4"
depends_on = []

[rules.resources]
threads = 1
memory = "1G"

# step2 and step3 are identical but depend on the previous rule
```

Only `gb_second_rate = 1.0` is configured (`amount = 1e-7` USD); the CPU rate is
absent, so the accrual is `gb_second_rate * (max_rss_bytes/1e9 * wall_time_secs)`.

## Profile

```toml
[cluster]
backend = "slurm"
partition = "TEST"
walltime = "10m"
max_submitted = 4
poll_interval = "5s"
```

## Control run (`run1/`) — no budget

`run1/out.json`:

```json
"status": "completed"
```

`results.succeeded = 3`. Output directory `run1/cp.json/out` contains
`step1.txt`, `step2.txt`, `step3.txt` — all three rules really ran.

## Treatment run (`run2/`) — `max_cost` cap

`run2/out.json` (abridged to the fields that matter):

```json
"status": "cost_budget_exceeded",
"cost_budget": {
  "accrued": 0.004194304,
  "cpu_seconds": 0.0,
  "gb_cost": 0.004194304,
  "limit": 1e-7,
  "method": "gb_second_rate * (max_rss_bytes/1e9 * wall_time_secs)",
  "on_unknown": "refuse",
  "unknown_rules": []
},
"pending_ready": ["step3", "step2"],
"results": { "succeeded": 0, "failed": 0, "skipped": 0 }
```

`run2/err.log`:

```text
Cluster: submitting 3 jobs to slurm (max 4 in flight)
Cost budget: config error: cost budget exceeded: accrued 0.004194 USD
  (method: gb_second_rate * (max_rss_bytes/1e9 * wall_time_secs)) against the
  [resource_budget] max_cost cap of 0.0000001 USD, with 2 rule(s) ready but
  undispatched and nothing in flight — the run cannot make progress.
Error: workflow execution failed
```

`run2/cp.json/out` contains **only** `step1.txt` — `step2` and `step3` never
ran. The abort landed after the first rule settled and before any further
submission.

## What this proves

1. The cluster driver path enforces the cap: new submissions stop once the
   accrued cost reaches `amount` (`must_stop()` on the driver round gate), and
   the run terminates with the structured status `cost_budget_exceeded`
   (non-zero exit), exactly as the local path does.
2. The status is not cosmetic — the run genuinely halted (`step1.txt` only,
   `succeeded = 0`).
3. The control run is byte-healthy (`completed`, 3 of 3), so the treatment's
   halt is attributable to the budget and not to a broken workflow or cluster.
4. `unknown_rules` is empty and `cpu_seconds = 0.0`: the accrual is computed
   purely from the GB-second path with no fabricated value.

## Known limits (documented residuals)

- SLURM `TotalCPU` is truncated to integer seconds, so a sub-second rule can
  report `cpu_seconds = 0.0`; an accrual that depends on the CPU rate alone
  would then under-count. The GB-second path is unaffected. This run used only
  `gb_second_rate`, so it does not exercise that residual.
- Multi-instance / crash-recovery enforcement is out of scope here; this is a
  single-host SLURM submission.

## Follow-up: terminal contract (review findings F1–F4, 2026-11-27)

An independent re-review of the merged #1400 surfaced four edge defects and
this follow-up fixes them; the same fr cluster re-verified the two terminal
shapes with binary `bin/oxo-flow-1400fu` (musl static release via
`cargo zigbuild`, `oxo-flow 0.32.0`, evidence root
`.../oxo-flow-slurm-test/cb1400fu`).

1. **F1 — a latched budget stop outranks the deadlock diagnosis.** The local
   scheduler accrued cost BEFORE `check_deadlock` and defers the check while
   the stop is latched (the disk-pressure Hold park makes the same
   precedence trade); the abort fires on ANY rule still `Pending`
   (`Scheduler::pending_rules`), not only `ready` ones — a rule blocked on a
   failed upstream is never `ready`.
2. **F2/F3 — terminal breach is visible on both paths.** When the cap is
   breached by the run's LAST settle (nothing stranded), the run no longer
   ends silently: locally `handle_run_completion` attaches the final accrual
   to the `--json` summary with a loud console line; on the cluster path the
   driver consults the cost gate once more at the `all_done` break (the
   settle that breached may land after the round's gate consult), and the
   caller attaches the recomputed final accrual. Status stays the run's own
   verdict; `ClusterRunSummary.cost_halt` distinguishes the halted outcome.
3. **F4 — cost stops are classified by the driver's own error.** The Err
   path matches the gate's message against the driver's returned `Config`
   error instead of slot presence, so a deadline/stall failure after a
   latch keeps its identity.

### Evidence `cb1400fu/run1` — halt parity (F2/F4 rework regression guard)

The original three-chain treatment re-run on the fixed binary: exit 1,
`run1/out.json` reports `"status": "cost_budget_exceeded"` with
`pending_ready: ["step2", "step3"]` and the reworded message
("2 rule(s) remaining undispatched"); `run1/cp.json/out` holds **only**
`step1.txt` — identical behavioral contract to the original treatment,
now through the F4 message-matched classification.

### Evidence `cb1400fu/run2` — terminal breach (F2/F3 cluster Ok path)

One rule (`solo`, `sleep 4`, GB-second rate 1.0, cap 1e-7): its settle is
the breach and nothing is stranded. Exit **0**, `run2/out.json`:

```json
"status": "completed",
"cost_budget": {
  "accrued": 0.004194304, "cpu_seconds": 0.0,
  "gb_cost": 0.004194304, "limit": 1e-7,
  "method": "gb_second_rate * (max_rss_bytes/1e9 * wall_time_secs)",
  "on_unknown": "refuse", "unknown_rules": [],
  "pending_ready": [], "currency_mismatch": []
}
```

`run2/cp.json/out` holds `solo.txt` (the rule really ran and settled).
Before the driver's final-gate-consult fix, this shape ended `completed`
with `cost_budget: null` — the exact invisible-breach gap F3 describes;
the fix is what makes the detail appear. Local counterparts:
`tests/cost_budget.rs` cases 9 (no-deadlock) and 10 (terminal breach).
