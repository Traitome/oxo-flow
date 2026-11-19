# wait

Block until a run in a workdir finishes, then report its outcome. This is
the native replacement for a poll-for-output-files shell supervisor
(issue #1078 F4) and pairs with [`run --background`](run.md#background-runs-background)
(or `run --detach`, its alias).

## Usage

```bash
oxo-flow wait [--workdir DIR] [--checkpoint PATH] [--timeout SECS] [--interval SECS]
```

## Options

| Option | Default | Description |
|---|---|---|
| `--workdir` | current directory | Run directory to watch. A run holds `.oxo-flow/lock`; the background pid file is `.oxo-flow/background.pid`; the run outcome is `<checkpoint>.status` (beside the checkpoint) |
| `--checkpoint` | `<workdir>/.oxo-flow/checkpoint.json` | Checkpoint inspected for the final verdict. Use it with [`run --checkpoint`](run.md#variant-checkpoints-checkpoint). When `--workdir` is omitted, the run directory recorded in the checkpoint supplies the lock/pid/status location; before the checkpoint exists `wait` polls for it (never the caller's cwd), and a recorded `failed` status is reported immediately even when no checkpoint was ever written. Pass `--workdir` for an immediate wait |
| `--timeout` | `0` | Maximum seconds to wait before giving up (`0` = no timeout) |
| `--interval` | `5` | Poll interval in seconds between checkpoint inspections |

## Exit status

| Code | Meaning |
|---|---|
| `0` | The run recorded `succeeded` (authoritative — a `failed_rules` entry left by an earlier run is reported as `stale_failed`, never as a failure). Rules skipped by a false `when` gate count as skipped |
| `1` | The run recorded `failed` (the error message is printed — a preflight refusal included), was killed before recording a terminal status, or (legacy runs without a status file) a checkpoint rule failed/remained in-flight |
| `3` | The timeout elapsed while the run was still active — the run keeps going |
| `4` | No active run and no checkpoint found — including a `--checkpoint` path that never appeared within the timeout and an empty (zero-byte) checkpoint file |

The run's own terminal outcome comes from `<checkpoint>.status`, written
beside the checkpoint by the run process (armed right after the workdir
lock, before any preflight that can fail). The per-checkpoint location
keeps smoke/test/full variants sharing a workdir from aliasing each other's
outcome; it also covers a preflight failure that happens before the first
checkpoint save, which `wait --checkpoint` reports from the status file
alone. This is what makes a preflight failure after a previous clean
checkpoint a **failure**, not a false success. The failed/in-flight rule
names are printed on stderr. With the global `--json` flag, the same
verdict is emitted as one document on stdout:

```json
{
  "command": "wait",
  "status": "failed",
  "run_state": "failed",
  "message": "protected data tree drift detected (1 file(s) changed since baseline)",
  "checkpoint": "/data/run/.oxo-flow/checkpoint.json",
  "elapsed_seconds": 12,
  "failed": [],
  "running": []
}
```

A success document carries `"status": "succeeded"`, empty `failed`/`running`,
the skipped-by-`when` names, and `stale_failed` (failure records retained in
the checkpoint from an earlier run; informational only). `status` is
`"succeeded"`, `"failed"`, `"timeout"`, or `"no-run"`; `run_state` (when
present) is `"running"`, `"succeeded"`, `"failed"`, or `"unknown"`.

## How completion is detected

- A **background** child is alive while `.oxo-flow/background.pid` names a
  live process; the child removes the file when it exits (including the
  signal-termination path). Pid `0` and out-of-range pids are ignored.
- **Any** run (foreground or background) holds the workdir lock
  `.oxo-flow/lock` for its whole lifetime, so `wait` also detects a
  foreground run started in another terminal.
- Once neither signal is active, `<checkpoint>.status` supplies that
  checkpoint's terminal outcome. It is per-checkpoint: activity from another
  variant sharing the workdir never invalidates a finished variant's
  status. A status still `running` after its process is gone means the run
  was killed or crashed (exit 1); a still-`running` status whose process is
  alive gets one poll interval to record its terminal outcome. A
  `succeeded` status is authoritative; stale checkpoint `failed_rules`
  entries are reported as `stale_failed`, not failures.
- An empty (zero-byte) checkpoint is not a run: `wait` reports no-run (4)
  rather than success.
- For legacy runs without a status file, the checkpoint supplies the
  rule-level reconciliation: a checkpoint whose `running` set is non-empty
  means the run died before completing those rules; a `failed_rules` entry
  whose `when` verdict is `false` is a skip, not a failure.

Because the pid file is written by the launcher before it exits, the
sequence below is race-free:

```bash
oxo-flow run pipeline.oxoflow --detach
oxo-flow wait --workdir "$(pwd)" --timeout 3600 || echo "run failed or timed out"
```

## Examples

```bash
# Wait for a detached run, then run validation only on success
oxo-flow run pipeline.oxoflow --background
if oxo-flow wait --workdir . --timeout 7200; then
  oxo-flow run validate.oxoflow
fi

# Machine-readable supervision
oxo-flow wait --workdir . --json | jq -r '.status, (.failed[]?)'

# Watch a variant run that uses its own checkpoint (works from any cwd)
oxo-flow run pipeline.smoke.oxoflow \
  --checkpoint .oxo-flow/smoke.checkpoint.json --background
oxo-flow wait --checkpoint /data/run/.oxo-flow/smoke.checkpoint.json --timeout 600
```

## Notes

- `wait` is read-only: it never writes to the checkpoint or the run status
  and never signals the run. To stop a run, use `kill <pid>` (the pid is in
  the background summary and the pid file).
- Poll cadence is intentionally coarse (5 s). A finished run is detected on
  the next poll; for live per-rule progress use
  [`oxo-flow status`](status.md) or the run log.
- If only `--checkpoint` is given and the run has not written its first
  checkpoint yet, `wait` polls for that checkpoint (bounded by `--timeout`)
  to learn the run directory; it never probes the caller's cwd, so an
  unrelated run there cannot answer for this one. Pass `--workdir` for an
  immediate wait (the background launcher prints the exact pair to copy).
