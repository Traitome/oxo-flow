# Real-scheduler verification matrix (issue #356)

Containerized **real scheduler software** — Son of Grid Engine 8.1.9 built
from source and OpenPBS 23.06 built from source — arranged as
master + 2 execution hosts under Docker Compose, driven end-to-end by
oxo-flow. This is the harness behind the live evidence posted on issue #356:
mock shims cannot produce credible allocation semantics, but a real qmaster
/ qsub / qacct (or pbs_server / qmgr / qstat -x) inside containers can.

The shared bind mount (`shared/` → `/work` in every container) carries the
oxo-flow binary, the matrix workflows, and the evidence output, so jobs run
on exec containers against the same paths the submitting driver sees — the
same shared-storage assumption as a real cluster.

## What the matrix proves per backend

| #356 checklist item | Evidence produced |
|---|---|
| multi-rule dependency wiring | `events.jsonl` timeline: each rule SUBMITTED only after its upstream COMPLETED; artifacts record the executing host per rule |
| array chunking at MaxArraySize | `max_array_size = 4` chunks a 10-instance scatter into `#$ -t 1-4` arrays (SGE) / `-J` ranges; element ids like `20_1`; `index.json` mapping |
| walltime passthrough | rule `time_limit` renders `h_rt`/`walltime`; scheduler accepts and records it |
| GPU directives | a consumable `gpu` complex/resource is defined on the scheduler (no physical GPUs needed) and the `-l gpu=N` directive schedules |
| settlement from real accounting | driver settles from `qacct` / `qstat -x` — `status.json` carries exit code, elapsed, peak RSS |
| env wrapping on real nodes | job-side `env` capture shows the scheduler's job environment (`NSLOTS`, `JOB_ID`, exec-host `HOSTNAME`) on the wrapped command |

## Layout

- `sge/` — SoGE 8.1.9 image (source build), master/exec setup, runner
- `pbs/` — OpenPBS 23.06 image (source build), server/MOM setup, runner
- `workflows/` — the two matrix workflows and per-backend profiles

## Running the SGE matrix

```bash
cd sge
docker build -t sge:8.1.9 .
mkdir -p shared/bin shared/workflows/profiles shared/evidence
cp <oxo-flow-linux-binary> shared/bin/oxo-flow
cp ../workflows/*.oxoflow shared/workflows/
cp ../workflows/profiles/sge*.toml shared/workflows/profiles/
cp run-sge.sh shared/
docker compose up -d --force-recreate   # master initializes, execs register
sleep 45
docker exec sge-master bash /work/run-sge.sh
# evidence lands in shared/evidence/sge/
```

The PBS matrix is the same shape (`pbs/`, profiles `pbs*.toml`, evidence in
`shared/evidence/pbs/`).

## Notes from standing this up (2026-09-22)

- SoGE 8.1.9 needs a handful of 2026-compat patches to build on ubuntu:24.04
  — all carried in `sge/Dockerfile` with rationale comments (kernel arch
  detection, jemalloc `sys/sysctl.h`, `-Werror`, libtirpc for SunRPC, BDB
  spooling dropped in favor of classic flatfile spooling).
- Classic spooling params are `"<common>;<spool>"` — two absolute paths
  separated by a semicolon (`spool_classic_create_context`).
- Debian's gridengine packages segfault in their postinst inside containers
  on both 22.04 and 24.04 (spooldefaults/BDB init); the source build avoids
  that entirely.
- LSF could not be stood up the same way: the community-edition download is
  IBMid-gated and its historical direct links are dead. Its checklist items
  were instead cross-checked against the jokergoo/bsub reference client and
  IBM documentation, producing the `bjobs -a`, KB-memory (`-M`/`rusage`),
  and `-gpu "num=N"` fixes in `crates/oxo-flow-core` (issue #356). A second
  audit round added the suspend-state mapping (`USUSP` pending vs
  `PSUSP`/`SSUSP`/`SUSP` running), `account` → `-P`, `span[hosts=1]` for
  multi-thread jobs, and `base[index]` array element ids.

## Setup lessons worth keeping (2026-09-22/23 campaign)

SGE (SoGE 8.1.9 source build — the Debian gridengine packages segfault in
their postinst inside containers on 22.04 AND 24.04, `--privileged` does
not help):

- classic spooling params are `"<common>;<spool>"` — two absolute paths
  joined by a semicolon (`spool_classic_create_context`); there is no
  `-Acx`/`-scx`/`-Mcx` in 8.1.9, so the consumable `gpu` complex is seeded
  as a centry file via `spooldefaults complexes` before the qmaster starts
- the queue template rejects the build-in defaults: `qtype BATCH`,
  `ckpt_list`, `pe_list`, `rerun`, `slots`, `tmpdir`, `notify`,
  `user_lists`/`xuser_lists`, `subordinate_list`, `complex_values`,
  `processors`, `min_cpu_interval` are all REQUIRED; `resume_interval` does
  not exist (it is `suspend_method`/`resume_method`)
- exec-host templates (`qconf -Ae file`) take a FILE, not stdin, and
  reject `load_values` (that field belongs to the execd)
- the global configuration must carry `gid_range`, or the execd fails
  every job with "can not parse gid_range" and the queue goes to E state
- register exec hosts only after the worker containers' compose DNS
  aliases resolve; `sge_execd` daemonizes, so the container needs a
  keepalive process after it

OpenPBS 23.06 source build:

- configure needs X11/Tcl/Tk headers, libdb, libpq, libical, libxml2 and
  swig; python 3.12 removed `Py_SetProgramName` and `eval.h` which
  Libpython still uses (guard + drop include — see the Dockerfile)
- the datastore is an embedded postgres: `psql` must be on PATH and
  `/etc/init.d/pbs start` runs `pbs_habitat` to initialize it — never
  pre-create `PBS_HOME/datastore` by hand (pg_ctl then refuses it)
- root cannot submit by default (`acl_roots`), uid mapping needs
  `flatten_files`, and settlement-by-`qstat -x -f` needs
  `job_history_enable = true`
- `PBS_VERSION` in pbs.conf must match `qstat --version` exactly or
  `pbs_habitat` refuses to initialize ("Version mismatch")
- `pbs_mom` daemonizes; same keepalive pattern as the SGE execd

Operational:

- register exec hosts only after worker DNS aliases exist; run ONE
  oxo-flow per run directory (two concurrent runs race on the run dir and
  the lock); drive detached runs with an rc-marker file — a foreground
  `docker exec` dies with the ssh session that started it
