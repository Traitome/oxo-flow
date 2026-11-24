# Kubernetes live campaign — single-node k3s on tx-ubuntu (2026-11-24)

Live-campaign evidence for audit finding **CL-05 / issue #1158** ("cloud backend
tests are mock/CLI-wrapper only"). This campaign exercised the Kubernetes
backend end to end against a **real scheduler** (k3s), not a mock:

| Step | Result |
| --- | --- |
| Submit | 2 rules rendered as `batch/v1` Jobs, `kubectl create` accepted both |
| Observe | driver polled to terminal state: `2 succeeded, 0 failed` |
| Artifacts | declared outputs appeared on the host workdir (`b.txt` = `6`); `--provenance` recorded `a.txt`/`b.txt` SHA-256 and `provenance verify` matched 2/2 |
| Logs | `cluster logs -b kubernetes oxoflow-logger` returned the pod's real stdout (`hello-from-k8s-pod`) |
| Cancel | SIGINT on the driver cancelled the in-flight Job (`cancelled=1`), the Job left the API, run recorded failed |
| Standalone cancel | `cluster cancel -b kubernetes <job>` → `✓ Successfully canceled jobs`; Job deleted |
| Repeat run | running the same workflow again with the previous run's completed Jobs still in the API succeeds — names are run-scoped (`oxoflow-make-557458cc`, `…-fe5bada8`, …) |

## Environment (§8 provenance)

- Host: `tx-ubuntu` `VM-4-4-ubuntu`, Ubuntu kernel `6.8.0-137-generic`, 4 vCPU, 3.6 GiB RAM
- Cluster: **k3s v1.31.4+k3s1** (single node, `--disable traefik,servicelb,metrics-server`), containerd inside k3s
- Client: `kubectl` 1.31.4 (k3s wrapper)
- Engine: oxo-flow `0.32.0`, branch `feat/k8s-workdir-mount` @ `a7ae8f07`
- Images preloaded into the node (registry egress unreachable from the node, see residual):
  `rancher/mirrored-pause:3.6`, `ubuntu:22.04`, `ubuntu:latest`
- Workflow: 2 system-environment rules (`make` writes `a.txt`; `derive` reads it and writes `b.txt`)
  and a long-running `slow2` (`sleep 1800`) used only for cancellation; a `logger` rule
  printed to stdout for the log-retrieval check. Profile: `[cluster] backend = "kubernetes"`,
  `max_submitted = 4`, `poll_interval = "2s"`.

## Defects found and fixed by this campaign

1. **Jobs had no workdir mount.** The rule command begins with `cd '<workdir>' &&`, but a
   pod's filesystem is its own: the path did not exist and declared outputs could never
   reach the submit host. The manifest now parses the folded `cd` prefix and mounts the
   workdir at the SAME absolute path via `hostPath` (shared-filesystem deployment mode),
   with `imagePullPolicy: IfNotPresent` so preloaded images are used on air-gapped nodes.
2. **`cluster cancel` passed the bare job id to `kubectl`.** Live cancel failed with
   `unknown command "oxoflow-slow"`. Cancel invocations are now shared between the
   executor and the CLI: Kubernetes gets `kubectl delete job <ids...>`, AWS/GCP one
   invocation per id, the local schedulers one call with all ids.
3. **Job names collided across runs.** The name was the bare `oxoflow-<rule>`, so a second
   run failed with `Error from server (AlreadyExists): jobs.batch "oxoflow-make" already
   exists` while the first run's completed Jobs were still in the API. `ClusterJobConfig`
   now carries a `name_scope` (the run directory for `run --profile`, the output directory
   for `cluster submit`) folded into an 8-hex Job-name suffix; the driver still reads the
   created name back from `kubectl create`, so tracking/logs/cancel are unchanged. Re-runs
   now succeed beside the previous run's Jobs (captured in the raw log).

## Honest residual (not qualified by this campaign)

- **Docker/singularity environment rules**: the plan still folds the local container
  wrapper into the command, so those rules need that runtime on the node (or image-based
  environment rendering, not implemented). This campaign used `system`-environment rules.
- **Array Jobs**: the Indexed Job dispatch line references the per-chunk command
  directory; only the workdir is mounted, so array submission on Kubernetes needs that
  directory mounted too (or a shared volume).
- **Multi-node clusters**: `hostPath` is a single-node/shared-filesystem mode; a real
  multi-node deployment needs a shared volume (NFS/PVC) instead.
- **Registry egress**: the node could not reach `registry-1.docker.io`; images were
  preloaded with `ctr images import`. Operators on restricted networks must do the same.
- **AWS Batch / GCP Batch**: still mock/CLI-wrapper only (no credentials on this host);
  their maturity labels in `run-on-cluster.md` remain Prototype.
- The trailing `sleep` in a cancelled run may leave a Job's pod terminating briefly;
  SIGINT path deletes the Job and the driver exits (verified above).
- Completed Jobs are not garbage-collected by the engine; operators should set a
  namespace-level TTL or clean them up. Run-scoped names make that safe.
- Raw transcript: [`data/kubernetes-live-campaign-2026-11-24.log`](data/kubernetes-live-campaign-2026-11-24.log).

## Reproduction sketch

```bash
# on the host
sudo k3s server --disable traefik --disable servicelb --disable metrics-server \
  --kubelet-arg=image-gc-high-threshold=95 --kubelet-arg=image-gc-low-threshold=90
docker save <pause> ubuntu:22.04 ubuntu:latest | sudo k3s ctr images import -
cd /tmp/k8scamp        # wf.oxoflow + profiles/k8s.toml
oxo-flow run wf.oxoflow --profile k8s --provenance   # 2 succeeded
oxo-flow --json cluster logs -b kubernetes oxoflow-logger
# SIGINT a long run → "interrupt received — cancelling in-flight jobs cancelled=1"
```
