# Per-run user + PID namespace isolation — live acceptance (issue #1401)

Campaign evidence for issue #1401 (RB-01 residual). It proves the dimensions
`OXO_FLOW_WEB_ISOLATION=userns` claims, through the REAL web spawn path, on a
host-prepared Linux machine, **as uid 1000** — root can create user
namespaces unconditionally and would make the run trivially pass. It also
re-validates the Landlock/`userns` composition exclusion at uid 1000 (the
first probes of that question ran as root and were therefore not acceptable
as final evidence).

The residual list at the end is part of the result: this is a measured
per-run uid/PID boundary on one host, not a complete security boundary.

Raw key output: [`data/rb01-userns-acceptance-2026-11-27.log`](data/rb01-userns-acceptance-2026-11-27.log).
The full run log (including the build) stayed on the campaign host.

## Environment

| Field | Value |
| --- | --- |
| Host | `VM-4-4-ubuntu`, Linux 6.8.0-137-generic x86_64 |
| CPU / RAM | Intel Xeon Platinum 8255C, 4 vCPU / 3.6 GiB |
| util-linux | `unshare` 2.39.3 |
| Landlock | ABI 4 (reported by the probe) |
| Run identity | uid **1000** (`ubuntu`), AppArmor profile `oxo-flow-userns (unconfined)`, verified with `unshare -Ur true` (exit 0) and `/proc/self/attr/current` |
| Build | source archive of `security/1401-userns-isolation`, `CARGO_BUILD_JOBS=1`, isolated `CARGO_TARGET_DIR`; `cargo test -p oxo-flow-web --test rb01_userns_adversarial --no-run --locked` (6m 19s) |
| Suite | `crates/oxo-flow-web/tests/rb01_userns_adversarial.rs`, `--nocapture --test-threads=1` |

## Method

- Every run is created through the production `POST /api/runs` path and the
  executor's spawn choke point: `sh -c <exit-code wrapper>` → optional
  `unshare --user --map-root-user --pid --fork --mount-proc --kill-child=SIGTERM --`
  → the CLI payload. The payload is a probe stub; the spawn, environment,
  exit-record, and cancel machinery is the production code.
- **Non-vacuity**: the same stub first runs with the mode OFF (control) and
  every probe must LEAK. The isolated run must deny the same probes — a
  regression that removes the wrapper flips the assertions back to a leak.
- **Cross-tenant signals** target a REAL second run of tenant B: a long
  `sleep` inside B's own namespace, located by a unique argv[0] marker, and
  later cancelled through `POST /api/runs/{id}/cancel`.
- **Fail-closed leg**: the first run is submitted with a failing `unshare`
  prepended to `PATH`; the executor must refuse to spawn (run failed, reason
  logged, no CLI executed, quota reservation released). Because only a
  SUCCESSFUL probe is cached, the later legs probe again and succeed.
- **Graceful teardown leg**: a direct spawn with a namespace init that traps
  SIGTERM, and a marker child that IGNORES SIGTERM (it can only die with the
  namespace). SIGTERM alone — no SIGKILL — must remove it inside the 5 s
  escalation window.
- `RLIMIT_NPROC` composition is asserted by inheritance (the probe records
  `ulimit -u` inside the namespace), not by a fork-bomb: the host is shared.

## Results

| Probe | Control (mode off) | Isolated (`userns`) |
| --- | --- | --- |
| `id -u` inside the run | 1000 (server uid) | **0** (namespace root) |
| `/proc/<server-pid>` visible | VISIBLE | **hidden** |
| `cat /proc/<server-pid>/environ` | LEAKED | **denied** |
| `kill -0 <server pid>` | SIGNALLED | **denied** |
| `kill -0 <host victim pid>` | SIGNALLED | **denied** |
| `kill -0 <tenant B host pid>` | SIGNALLED | **denied** |
| numeric entries in `/proc` | 215 (host) | **4** (private PID namespace) |
| `ulimit -u` (operator `RLIMIT_NPROC=4096`) | 14566 (host limit) | **4096** (inherited through `unshare`) |
| write the run's own workdir | pass | **pass** (file owned by uid 1000 on the host) |

- **Exit-status propagation**: a run whose CLI exits 7 through `unshare` is
  recorded `failed` by the web executor — the `.exit-code` wrapper semantics
  are unchanged.
- **Fail-closed refusal** (requested mode, unusable host): run `failed`, the
  refusal reason persisted in `execution.log`, no CLI or wrapper executed,
  and the quota reservation released (`active_runs=0`).
- **Cancel**: tenant B's run reached `cancelled`; its namespaced process
  (host pid 1673564) disappeared and no process carrying the marker survived
  — group kill + `--kill-child=SIGTERM`, with the 5 s SIGKILL escalation as
  the guaranteed path for an init that does not trap SIGTERM.
- **SIGTERM-only teardown**: with no SIGKILL sent at all, the namespace was
  gone in **11.1 ms** (`namespace_has_no_survivor=true`) — the graceful
  SIGTERM path has a real, measured window.
- The suite finished in 5.84 s with `1 passed; 0 failed`.

## Landlock composition — re-measured as uid 1000

Measured in the same session, under the same profile, as uid 1000:

| Variant | Result |
| --- | --- |
| no Landlock (`none`) | unshare exit 0 (control for the probe itself) |
| Landlock, system grants only (`system-only`) | `EACCES` opening `/proc/self/uid_map` |
| + file grants on `/proc/self/{uid_map,gid_map,setgroups}` (`proc-grants`) | `EACCES` |
| + rule on the whole `/proc/self` directory (`proc-self-dir`) | `EACCES` |

This reproduces the earlier root-run probe (the negative is kernel/landlock
behavior, not a privilege artifact). Mechanism: with `--fork` — required for
the PID namespace — util-linux clones and the **child** opens its own
`/proc/self/uid_map`; the child's host pid is assigned at runtime, so no
ruleset built before exec can name that inode. The no-fork self-mapping shape
is also denied after exec, because the Landlock path rule resolved on the
pre-exec proc entry does not cover the post-exec open of the dynamic pid
entry. Only a host-`/proc` grant would cover the write, and that would
re-expose every process on the host — the disclosure both modes exist to
prevent. The combination therefore fails closed; `web-api.md`,
`web-system-architecture.md`, `security.md`, and `deploy-modes.md` record the
mechanism and the one-mode-at-a-time rule.

## Reproduce

```bash
# Host prep (Ubuntu 24.04): the profile used for this campaign.
sudo tee /etc/apparmor.d/oxo-flow-userns >/dev/null <<'EOF'
abi <abi/4.0>,
include <tunables/global>

profile oxo-flow-userns /usr/local/bin/oxo-flow-web flags=(unconfined) {
  userns,
}
EOF
sudo apparmor_parser -r /etc/apparmor.d/oxo-flow-userns

# Prove the profile + uid before trusting any result.
sudo aa-exec -p oxo-flow-userns -- sudo -u ubuntu unshare -Ur true

# Build (isolated target dir) and run the suite as uid 1000.
CARGO_BUILD_JOBS=1 CARGO_TARGET_DIR=$HOME/oxo-build-target-1401 \
  cargo test -p oxo-flow-web --test rb01_userns_adversarial --no-run --locked
sudo aa-exec -p oxo-flow-userns -- sudo -u ubuntu env \
  OXO_FLOW_BIN=<absolute oxo-flow CLI> \
  <test-binary> --nocapture --test-threads=1
```

### Landlock composition probe (reproducible)

The four variants in the composition table are driven by
[`data/rb01-userns-landlock-probe.py`](data/rb01-userns-landlock-probe.py),
which installs the ruleset in-process and then execs `unshare` (the same
process, preserving the pid). Copy it to the host and run each variant as
uid 1000 under the profile:

```bash
for v in none system-only proc-grants proc-self-dir; do
  sudo aa-exec -p oxo-flow-userns -- sudo -u ubuntu \
    python3 -I /path/to/rb01-userns-landlock-probe.py "$v"
  echo "variant=$v exit=$?"
done
```

Expected on the measured host: `none` exits 0; the other three exit 1 with
`unshare: cannot open /proc/self/uid_map: Permission denied` — the measured
basis for the fail-closed mutual exclusion.

## Limits and residuals (not claimed)

- One host, one measured run per leg (fail-closed, control, isolated,
  exit-code, cancel, SIGTERM-only), kernel 6.8.0-137, Landlock ABI 4,
  util-linux 2.39.3. Other kernels/LSM configurations are covered only by
  the fail-closed probe.
- The spawned payload is a probe stub; the production executor, wrapper,
  env allowlist, rlimits, and cancel path are exercised, but no real
  bioinformatics rule shell was run inside the namespace in this campaign.
- PID exhaustion is represented by `RLIMIT_NPROC` **inheritance**; no
  fork-bomb was run (shared host, #1398 campaign constraint). The residual
  stands: `RLIMIT_NPROC` is counted per real uid and is not partitioned by
  namespaces, so a same-uid tenant can still exhaust the shared process
  budget.
- Single-uid mapping only (root inside, server uid outside); no per-run
  dedicated uid.
- Namespaces do not confine the filesystem or network: UDP/ICMP remain
  unrestricted, and filesystem reachability needs the Landlock mode (one
  mode at a time).
- Local runs only; cluster/remote runs execute on another host under that
  host's controls. No multi-instance, long-duration, or high-load
  measurements.
- `make ci` (local, macOS) is green on the implementation tree; this suite
  compiles there and skips by design (no Linux `unshare`). The live run in
  this page is the Linux acceptance.
