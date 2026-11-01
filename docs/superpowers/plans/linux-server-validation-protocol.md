# Linux Server Validation Protocol (Hardware x Software x Design x UX)

This protocol defines the full-scope validation executed on a dedicated Ubuntu
server by an independent subagent. It complements the macOS developer loop with
the Linux-only paths the Quad-Pillar architecture depends on: NUMA topology
discovery, storage-tier routing, headless VS Code integration, and the real
server-mode UX surface.

## 1. Scope and Success Criteria

| Pillar | What is validated | Success criterion |
| :--- | :--- | :--- |
| Hardware | NUMA discovery, resource pool, disk-pressure guard | No panic; discovery matches `lscpu`; graceful degradation on single-node hosts |
| Software | Full CI gate suite, engine unit/integration tests | `make ci` exits 0 with AI providers disabled |
| Design | Metro-DAG graph output, report snapshots, docs build | Metro render succeeds on gallery workflows; `mkdocs build --strict` exits 0 |
| UX | CLI learning surface, web server, 1-key remediation narration | Subcommand count matches docs; serve + health + OpenAPI respond; failure narration actionable |

## 2. Environment Audit (record verbatim)

```bash
uname -a && nproc && free -h && df -h .
lscpu | grep -E '^(CPU\(s\)|NUMA|Socket|Core|Thread)' || true
cargo --version && rustc --version && node --version && python3 --version
```

## 3. Deterministic Test Environment

AI provider auto-detection must be disabled for every test command so the
suites never touch a live endpoint:

```bash
unset ANTHROPIC_AUTH_TOKEN ANTHROPIC_BASE_URL ANTHROPIC_MODEL
export OXO_FLOW_AI_PROVIDER=disabled
```

## 4. Gate Suite

```bash
git fetch origin && git checkout codex/industrial-scientific-roadmap
git log --oneline -1   # record HEAD; must be at or after the SIGXCPU fix commit
make ci                    # fmt, clippy, build, test, schema-drift, version-check,
                           # audit, deny, tripwire, secrets, docs, frontend, eval, extension
make extension-integration # headless VS Code integration tests
mkdocs build --strict      # run inside docs/guide
```

`cargo audit` / `cargo deny` failures caused by sandboxed network access are
recorded as environment-limited, not product failures.

## 5. Usage Tests (compiled binary, real workflows)

```bash
OXO=target/debug/oxo-flow
$OXO --version && $OXO --help          # subcommand count must equal 32
$OXO validate examples/gallery/*.oxoflow
$OXO lint     examples/gallery/*.oxoflow
$OXO dry-run  examples/gallery/06_rnaseq_quantification.oxoflow
$OXO run      examples/gallery/01_hello_world.oxoflow
$OXO run      examples/gallery/03_parallel_samples.oxoflow
$OXO run      examples/gallery/04_scatter_gather.oxoflow
$OXO graph --format metro examples/gallery/06_rnaseq_quantification.oxoflow
```

Ecosystem transpiler: write a minimal `nextflow.nf` (one process with `cpus`,
`container`) and a minimal Snakemake `Snakefile` (two rules, one `temp()`),
then `oxo-flow import` each and `validate` both outputs.

Fault-injection and benchmark harnesses:

```bash
python3 eval/frontier/fault_injection.py              # expect 10/10 PASS
python3 eval/frontier/statistical_report.py
python3 eval/benchmarks/cohort_driver.py --scale-sweep
python3 eval/benchmarks/hardware_scaling.py --cohort-size 10000
```

NUMA path: if the host exposes more than one NUMA node, exercise a rule with
`[rules.<name>.resources] numa_pin = true` and confirm the `numactl` wrapper
appears in the execution log; on a single-node host confirm clean degradation.

Server UX surface: start `oxo-flow serve` (personal mode) in the background,
then `curl /api/health` and `curl /api/openapi.json`, and load the SPA index.
Stop the server afterwards.

## 6. Learning Test (fresh-eyes UX pass)

Reading only `--help` output for `run`, `import`, and `graph`: can a
bioinformatician who has never seen the tool infer the next action? Record any
flag whose name, default, or help text is ambiguous, and any command whose
error message does not suggest a fix.

## 7. Reporting and Fix Policy

- Report PASS/FAIL per section with exact commands and captured output tails.
- Environment or toolchain gaps (missing rustup, npm, mkdocs) may be installed.
- Product bugs get a minimal fix plus a regression test, committed to
  `codex/industrial-scientific-roadmap` with a conventional message and pushed.
- PR #997 stays a draft with its `[DO NOT MERGE]` guard; never merge it.
- Zero date tokens in every file and commit message, per repository invariant.
