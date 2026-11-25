# Kubernetes multi-node live campaign — two-VM k3s + network NFS (2026-11-26)

Live-campaign evidence for **issue #1387** (`[cluster] volume support (PVC/NFS) +
multi-node live qualification`). The renderer (PR #1392) and the single-host NFS
drill landed earlier; this campaign is the remaining acceptance item: a **two-node
cluster with separate kernels**, a **network NFS export** shared by both nodes, a
run workdir **inside** the export, `[cluster.volume] kind = "nfs"`, and a
**dependent rule forced onto the second node**, with per-element settle and
provenance closure.

Result: **qualified**. 3 succeeded / 0 failed in both runs; the array settled
element-by-element on different nodes; `provenance verify` matched 3/3 with the
workflow content check verified. **No engine defect was found — no code changes
were needed for this drill.**

Raw transcript (all commands and outputs below):
[`data/kubernetes-multinode-2026-11-26.log`](data/kubernetes-multinode-2026-11-26.log).

## Environment

| Item | Value |
| --- | --- |
| Host | macOS 27.2.1 (26B131), arm64, 10 CPU, 24 GiB RAM |
| Hypervisor | Lima 2.3.0 (`vz` — Apple Virtualization.framework; no qemu/brew present) |
| VM1 `lima-vm1` | Ubuntu 24.04.5 LTS, kernel `6.8.0-147-generic`, 2 vCPU, 2.8 GiB RAM, 20 GiB disk, `192.168.104.1` |
| VM2 `lima-vm2` | Ubuntu 24.04.5 LTS, kernel `6.8.0-147-generic` (separate kernel instance), 2 vCPU, 2.8 GiB RAM, 20 GiB disk, `192.168.104.3` |
| VM network | Lima `user-v2` L2 network `192.168.104.0/24`; VM1↔VM2 `ping` 0.246–0.405 ms; TCP 22 and UDP (netcat) guest-to-guest verified |
| Cluster | k3s `v1.37.2+k3s1` (`f6973cb8`), containerd `2.3.7-k3s1`, kubectl `v1.37.2+k3s1`; VM1 server/control-plane, VM2 agent |
| Kubelet reservations | VM1 `system-reserved=cpu=200m,memory=1536Mi` (allocatable **1800m / 1396516Ki**); VM2 `system-reserved=cpu=100m,memory=256Mi` (allocatable **1900m / 2707236Ki**) |
| Pod image | `ubuntu:latest`, index digest `sha256:d66b366dac61b575bee2aef07e9c26e8070096fbe84a3d4d3eb0da069b5d76db`, pulled from Docker Hub into both nodes' k3s containerd (`k3s ctr images pull`) |
| NFS server (VM1) | export `/srv/oxoflow` to `192.168.104.0/24(rw,sync,no_subtree_check,no_root_squash)` |
| NFS client (VM2) | `/srv/oxoflow` mounted `192.168.104.1:/srv/oxoflow type nfs4 (vers=4.2, ...)` |
| Engine | oxo-flow `0.32.0`, commit `dedc11210b7c9d1973679b1d2eea5307f1a27231` (`main`), built on VM1 with rustc `1.98.0` (`cargo build --release -p oxo-flow-cli`, 9m03s) |

`kubectl get nodes` (both Ready, distinct names and IPs):

```text
NAME       STATUS   ROLES           AGE     VERSION        INTERNAL-IP     KERNEL-VERSION
lima-vm1   Ready    control-plane   12m     v1.37.2+k3s1   192.168.104.1   6.8.0-147-generic (arm64)
lima-vm2   Ready    <none>          9m49s   v1.37.2+k3s1   192.168.104.3   6.8.0-147-generic (arm64)
```

## Workflow and profile

The run workdir **is** inside the NFS export (`/srv/oxoflow/run1387` and
`/srv/oxoflow/run1387b`), so `[cluster.volume]` maps it to `/oxoflow/...` in-pod
and never emits a submit-host `hostPath`.

```toml
# wf.oxoflow
[workflow]
name = "k8s-multinode-1387"
version = "1.0.0"
description = "Two-node k3s + network NFS [cluster.volume] live qualification (issue #1387)"

[defaults]
threads = 1
memory = "256M"

[[sample_groups]]
name = "all"
samples = ["S1", "S2"]

[[rules]]
name = "make"
output = ["raw/{sample}.txt"]
shell = """
mkdir -p raw
{ printf 'sample=%s pod=%s kernel=%s at=%s\n' "{sample}" "$(hostname)" "$(uname -r)" "$(date -Is)"
  printf 'workdir-fs=%s\n' "$(findmnt -T . -o SOURCE,FSTYPE -n | tr -s ' ')"
} > raw/{sample}.txt
sleep 30
"""

[[rules]]
name = "fold"
input = ["raw/S1.txt", "raw/S2.txt"]
output = ["final.txt"]
shell = """
{ printf 'fold pod=%s kernel=%s at=%s\n' "$(hostname)" "$(uname -r)" "$(date -Is)"
  printf 'workdir-fs=%s\n' "$(findmnt -T . -o SOURCE,FSTYPE -n | tr -s ' ')"
  cat raw/S1.txt raw/S2.txt
} > final.txt
"""

[rules.resources]
threads = 1
memory = "2048M"
```

```toml
# profiles/k8s.toml
[cluster]
backend = "kubernetes"
max_submitted = 4
poll_interval = "2s"

[cluster.volume]
kind = "nfs"
server = "192.168.104.1"
path = "/srv/oxoflow"
mount_root = "/oxoflow"
```

Each rule requests 1 CPU / 256 MiB (the `fold` rule overrides to 2048 MiB). With
1800m/1900m allocatable CPU, **two 1-CPU array elements cannot fit on one node**,
and `fold`'s 2048 MiB request exceeds VM1's 1364 MiB allocatable — both are
native scheduler constraints, not node-name pinning (the renderer has no
`nodeSelector`/`toleration` support, and this drill did not add any).

## Run 1 — taint-windowed, first element guaranteed on VM1

VM2 joined with the taint `oxoflow.io/qualification:NoSchedule`, so the first
array element could only start on VM1. The taint was removed while that element
was Running, 4 s into the run, so the second element could use VM2.

Timeline (UTC; VM local time is +08:00):

| Time | Event |
| --- | --- |
| 17:18:05.241 | `oxo-flow run wf.oxoflow --profile k8s --provenance` starts; 3 rule instances planned (2-element Indexed Job `make` + `fold`) |
| 17:18:06 | element 0 `oxoflow-array-make-808e21fa-0-7fxz8` **Running on lima-vm1**; element 1 `…-1-9tv59` **Pending** |
| 17:18:10 | taint `oxoflow.io/qualification` removed from `lima-vm2` |
| 17:18:11 | element 1 starts on **lima-vm2** (its own output timestamp) |
| 17:18:40.566 | `make_all_S1` element handle `…-808e21fa_1` COMPLETED |
| 17:18:44.715 | `make_all_S2` element handle `…-808e21fa_2` COMPLETED (independent settle) |
| 17:18:46.778 | `fold` submitted; pod `oxoflow-fold-bf5aaecc-l9fc2` scheduled on **lima-vm2** |
| 17:18:50.990 | `fold` COMPLETED — **3 succeeded, 0 failed** |

Placement evidence:

```text
NAME                                  READY STATUS    NODE       IP
oxoflow-array-make-808e21fa-0-7fxz8   0/1   Completed lima-vm1   10.42.0.5
oxoflow-array-make-808e21fa-1-9tv59   0/1   Completed lima-vm2   10.42.1.3
oxoflow-fold-bf5aaecc-l9fc2           0/1   Completed lima-vm2   10.42.1.4

oxoflow-array-make-808e21fa completions=2 completedIndexes=0,1 failedIndexes= succeeded=2
oxoflow-fold-bf5aaecc        completions=1 completedIndexes=  failedIndexes= succeeded=1
```

The scheduler's own event on element 1 records the exact mechanism while the
taint was still present:

```text
Warning FailedScheduling 0/2 nodes are available: 1 Insufficient cpu,
       1 node(s) had untolerated taint(s). ...
Normal  Scheduled         Successfully assigned default/oxoflow-array-make-808e21fa-1-9tv59 to lima-vm2
```

The **dependent** rule (`fold`) cannot run on VM1: its `resources.memory =
"2048M"` is above VM1's 1364 MiB allocatable, while VM2 has 2643 MiB — the
scheduler therefore had exactly one legal node (`Successfully assigned …
oxoflow-fold-bf5aaecc-l9fc2 to lima-vm2`). The `make` array settled each
element separately (`_1` at 17:18:40.566, `_2` at 17:18:44.715; the engine's
`jobs/<instance>/status.json` records both as `COMPLETED` under the same Indexed
Job).

Volumes actually rendered (pod specs; **no hostPath anywhere**):

```text
volumes=[{"name":"oxoflow-shared","nfs":{"path":"/srv/oxoflow","server":"192.168.104.1"}},
         {"name":"kube-api-access-…","projected":{…serviceaccount…}}]
mounts=[{"mountPath":"/oxoflow","name":"oxoflow-shared"}]
args=["INDEX=$((JOB_COMPLETION_INDEX + 1)); sh \"/oxoflow/run1387/.oxo-flow/runs/…/jobs/make/chunk-1/cmd.${INDEX}.sh\""]
args=["cd '/oxoflow/run1387' && … > final.txt"]
```

Outputs, written through the NFS volume and read back on the submit host:

```text
raw/S1.txt   sample=S1 pod=…-0 kernel=6.8.0-147-generic …   workdir-fs=192.168.104.1:/srv/oxoflow nfs4
raw/S2.txt   sample=S2 pod=…-1 kernel=6.8.0-147-generic …   workdir-fs=192.168.104.1:/srv/oxoflow nfs4
final.txt    fold pod=…-l9fc2 …                             workdir-fs=192.168.104.1:/srv/oxoflow nfs4
             + both raw records (element 0 on VM1, element 1 on VM2)
```

`oxo-flow provenance verify .oxo-flow/checkpoint.json`:

```text
• workflow content check: verified (sha256:181ec75d…)
✓ final.txt   sha256:f1aaa0ff2527ad362ebb1021ba14bc191be6a92474dc79f1c339d15e780bf2bf
✓ raw/S1.txt  sha256:762e89cc97802f2e0f643dc48b8e4b45050f02f70dd0719f234861c284d333f4
✓ raw/S2.txt  sha256:c6048dcebcfd328cdd4683a82f1cf7db0efd918ecfab136077647a0f306df131
Summary: 3 matched, 0 mismatched, 0 missing
```

## Run 2 — no taint, native cross-node spread (repeatability)

A second run in a fresh workdir (`/srv/oxoflow/run1387b`) started with **both
nodes untainted**. Because each element requests 1 CPU and neither node can host
two (1800m/1900m allocatable), Kubernetes had to spread the array — and it did,
in the opposite direction from run 1:

```text
oxoflow-array-make-94900c15-0-5vbph   0/1 Completed lima-vm2   10.42.1.5
oxoflow-array-make-94900c15-1-txgdw   0/1 Completed lima-vm1   10.42.0.6
oxoflow-fold-f1657413-9db7p           0/1 Completed lima-vm2   10.42.1.6

oxoflow-array-make-94900c15 completions=2 completedIndexes=0,1 succeeded=2
oxoflow-fold-f1657413              completedIndexes=  failedIndexes=  succeeded=1
```

`3 succeeded, 0 failed`; per-element `status.json` handles `…-94900c15_1` and
`…_2` both `COMPLETED`; `provenance verify` again matched **3/3**, workflow
content check verified (`final.txt` sha256 `daae8085…`). No manual scheduling
intervention happened in this run — the pod events show plain
`Successfully assigned … to lima-vm1` / `… to lima-vm2` / `… fold … to lima-vm2`.

## OS-level NFS round trip (independent of the pods)

The pods mount NFS directly through the kubelet, but the two VM kernels also
share the export:

```text
VM2$ mount | grep oxoflow
192.168.104.1:/srv/oxoflow on /srv/oxoflow type nfs4 (rw,relatime,vers=4.2,…,addr=192.168.104.1)

VM1$ sha256sum /srv/oxoflow/run1387/final.txt
f1aaa0ff2527ad362ebb1021ba14bc191be6a92474dc79f1c339d15e780bf2bf
VM2$ sha256sum /srv/oxoflow/run1387/final.txt
f1aaa0ff2527ad362ebb1021ba14bc191be6a92474dc79f1c339d15e780bf2bf
VM2$ echo … | sudo tee /srv/oxoflow/run1387/vm2-host-nfs-proof.txt   # visible on VM1
```

## Acceptance mapping (#1387)

- **Live 2-node run**: yes — k3s server on VM1 + agent on VM2, distinct kernels/IPs, both Ready.
- **Shared volume**: yes — real kernel NFSv4.2 export on VM1, mounted by kubelet on VM2 and by the VM2 OS.
- **Workdir on the shared volume**: yes — `/srv/oxoflow/run1387*`, rendered as `/oxoflow/…` in-pod with no `hostPath`.
- **Dependent rule forced onto the second node**: yes — `fold` (2048 MiB) exceeds VM1's allocatable and ran on VM2 in both runs; the array elements themselves spread across both nodes (run 2 without any admin intervention).
- **Per-element settle**: yes — Indexed Job `completedIndexes=0,1`, independent `status.json` completion times/records per element.
- **Pull closure**: yes — outputs land in the submit-host workdir through NFS; `provenance verify` 3/3 with the workflow content check verified.
- **Docs updated**: the Kubernetes maturity row in
  [`run-on-cluster.md`](../../guide/src/how-to/run-on-cluster.md) now cites this
  multi-node evidence; `hostPath` = single-node and PVC = unqualified caveats
  remain.

## Scope and limits (honest)

- **Co-located host.** Both VMs run on one Mac through Apple Virtualization.framework. They are genuinely separate kernels, network stacks, and kubelets, and the NFS path crosses the virtual network — but they are not separate physical machines.
- **`user-v2` is a virtual L2**, not a physical switch. VM↔VM TCP, UDP, and ICMP were verified; cross-node **pod-to-pod** (flannel VXLAN) traffic was not exercised — the qualified data path is pod → node NFS.
- **Single control-plane + one agent.** No HA, no multiple control planes, no node failure/cordon/recovery drill.
- **`kind = "nfs"` only.** `kind = "pvc"` remains render-tested only (no live PV/provisioner drill); `kind = "hostpath"` keeps its single-node semantics and was not re-drilled here.
- **`no_root_squash`.** Pods run as root, so the export grants root writes. Production deployments should restrict the export or use pod security contexts (not rendered by the current backend).
- **`system`-environment rules only** (`ubuntu:latest`), consistent with the prior campaign. Docker-environment rules still need image-based environment rendering (#1388).
- **Run 1 used a taint window** to guarantee the first element landed on VM1 (documented above). The `fold` placement is resource-forced in both runs; run 2 used no taint manipulation at all.
- **Tiny VMs.** 2 vCPU / ~3 GiB per node means at most one 1-CPU rule per node at a time; the forcing depends on that resource asymmetry. Larger nodes would need larger requests or explicit node placement (which the renderer does not yet support).
- **No cloud provider volume qualification** (TKE/CBS/EBS), no cloud spend.
- Completed Jobs are still not garbage-collected by the engine (pre-existing residual, unchanged).

## Reproduction sketch

```bash
# Host: install Lima from the release tarball (no brew on this Mac)
limactl create --name=vm1 --cpus=2 --memory=3 --disk=20 --containerd=none \
  --network=lima:user-v2 --tty=false template:ubuntu-24.04
limactl create --name=vm2 --cpus=2 --memory=3 --disk=20 --containerd=none \
  --network=lima:user-v2 --tty=false template:ubuntu-24.04
limactl start --tty=false vm1; limactl start --tty=false vm2

# VM1: NFS export + k3s server
sudo apt-get install -y nfs-kernel-server nfs-common build-essential curl git
sudo mkdir -p /srv/oxoflow/run && sudo chmod 777 /srv/oxoflow /srv/oxoflow/run
echo '/srv/oxoflow 192.168.104.0/24(rw,sync,no_subtree_check,no_root_squash)' | sudo tee /etc/exports
sudo systemctl enable --now nfs-kernel-server && sudo exportfs -ra
curl -sfL https://get.k3s.io | sudo INSTALL_K3S_VERSION=v1.37.2+k3s1 sh -s - server \
  --disable traefik --disable servicelb --disable metrics-server \
  --write-kubeconfig-mode 644 \
  --kubelet-arg=system-reserved=cpu=200m,memory=1536Mi
sudo k3s ctr images pull docker.io/library/ubuntu:latest
# build the engine (commit under test)
git clone https://github.com/Traitome/oxo-flow.git ~/oxo-flow && cd ~/oxo-flow
cargo build --release -p oxo-flow-cli

# VM2: NFS client + k3s agent (tainted so the first element is guaranteed on VM1)
sudo mount -t nfs -o vers=4.2 192.168.104.1:/srv/oxoflow /srv/oxoflow
curl -sfL https://get.k3s.io | sudo INSTALL_K3S_VERSION=v1.37.2+k3s1 \
  K3S_URL=https://192.168.104.1:6443 K3S_TOKEN=<token> sh -s - agent \
  --node-taint=oxoflow.io/qualification:NoSchedule \
  --kubelet-arg=system-reserved=cpu=100m,memory=256Mi
sudo k3s ctr images pull docker.io/library/ubuntu:latest

# Submit host (VM1): workdir inside the export
cd /srv/oxoflow/run1387   # wf.oxoflow + profiles/k8s.toml as above
export KUBECONFIG=/etc/rancher/k3s/k3s.yaml
oxo-flow run wf.oxoflow --profile k8s --provenance
# while the first array pod is Running: kubectl taint nodes lima-vm2 oxoflow.io/qualification-
oxo-flow provenance verify .oxo-flow/checkpoint.json
```

## Verdict

The multi-node Kubernetes path with a network NFS `[cluster.volume]` is live-qualified
for the tested scope: two-node k3s, network NFSv4.2, workdir on the volume, an
array settling per element across both nodes, a dependent rule forced onto node 2,
outputs arriving on the submit host through the volume, and provenance closure.
The residual caveats above (co-located VMs, NFS-only live kind, no PVC live drill,
no cloud provider, root-write export) are the honest boundary of this evidence.
