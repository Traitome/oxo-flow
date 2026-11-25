# Kubernetes image-based environment rendering — live proof (2026-11-25 / 26)

Live evidence for **issue #1388** ("environments: image-based environment
rendering for cluster backends"): a container-environment rule on Kubernetes
now runs AS its own image instead of folding the local `docker run` /
`singularity exec` wrapper into the pod command. The proof runs on the
single-node k3s host from the 2026-11-24 Kubernetes campaign.

| Step | Result |
| --- | --- |
| Image-mode docker rule | Pod `image:` = `ubuntu:22.04`; output `VERSION_ID=22.04` while the HOST is Ubuntu 24.04; `docker` CLI and `/var/run/docker.sock` absent in the pod; **no** `OXO_FLOW_K8S_DOCKER_SOCKET` |
| Bash-less image (review F1) | `docker = "busybox:1.36"` (no `/bin/bash`) STARTED and completed: pod command `/bin/sh -c '<bash-if-available shim>' sh '<cmd>'`, output `busybox-ok`, `bash-absent` |
| Bash prelude + array (review F1) | `[defaults] shell_prelude = "set -euo pipefail"` with `[[ ]]` bashisms succeeded in a single Job AND a 2-element Indexed Job: the single text and each per-index FILE run through the bash-if-available shim; `succeeded: 2/2` |
| System environment (review M2) | A `system` rule is annotated/recorded `environment-render: system` (no wrapper exists); it ran 1/1 |
| Wrapper fallback | `cluster submit` of a conda and a local-SIF workflow emits one WARN each and `wrapper-fallback` manifests with the folded wrapper intact |
| Image-render opt-out (review M3) | `OXO_FLOW_K8S_IMAGE_RENDER=0 cluster submit` renders the docker rules as `wrapper-fallback`: base image = the rule's image, `/bin/bash -c`, folded `docker run …` |
| Provenance | Checkpoints record `cluster_images` (image/fallback/system + offline digest); `provenance verify` prints it and matched 9/9 and 3/3 output checksums |
| Registry mirror | `OXO_REGISTRY_MIRRORS` rewrites the manifest `image:` at render time; the identity annotation stays the workflow-declared spec |

## Environment (§8 provenance)

- Host: `tx-ubuntu` `VM-4-4-ubuntu`, Ubuntu **24.04.4 LTS**, kernel `6.8.0-137-generic`, 4 vCPU, 3.6 GiB RAM
- Cluster: **k3s v1.31.4+k3s1** (single node, `vm-4-4-ubuntu`, containerd 1.7.23-k3s2)
- Client: `kubectl` 1.31.4 (k3s wrapper; `KUBECONFIG=$HOME/.kube/config` copied from `/etc/rancher/k3s/k3s.yaml`)
- Engine: oxo-flow `0.32.0`, branch `feat/1388-image-env-rendering` at `23aab914`
  (the tested binary was built from the same code tree; only documentation/evidence
  files changed after it)
  (source rsynced to `~/oxo-flow-1388`, `cargo build -p oxo-flow-cli`, debug binary;
  the checkout carries no `.git` objects so the run log records `git_sha=(none)`)
- Images preloaded into the k3s runtime (registry egress is unreachable from this
  node, see the 2026-11-24 audit): `ubuntu:22.04`/`ubuntu:latest`
  (`sha256:f64b2ffc…`), **`busybox:1.36`** (`sha256:73aaf090…`, imported with
  `docker save busybox:1.36 | sudo k3s ctr images import -`), `rancher/mirrored-pause:3.6`,
  `docker:27-cli`, `docker-cli-bash:local`
- Submit host's docker daemon holds `ubuntu:22.04` and `busybox:1.36`
  (`ubuntu@sha256:f64b2ffc…`, `busybox@sha256:73aaf090…`), which is what the
  offline digest resolution in `cluster_images` reports
- Workflows and raw logs: `/tmp/oxo-1388-final2` (main + renders) and
  `/tmp/oxo-1388-prelude` (bash/prelude) on the host
- Committed raw transcripts (`docs/superpowers/audits/data/`):
  `kubernetes-image-render-2026-11-25-run.log`,
  `…-prelude-run.log`, `…-prelude-provenance-verify.log`,
  `…-provenance-verify.log`, `…-outputs.txt`, `…-kubectl-jobs.json`,
  `…-fallback-render.log`, `…-no-image-render.log`, `…-mirror-render.log`,
  `…-mirror-manifest.json` (the rendered mirror Job, raw)

## Image-mode docker rule + bash-less image

`/tmp/oxo-1388-final2/wf.oxoflow` (no `[defaults]` prelude — a `pipefail`
prelude itself requires bash, by contract):

```toml
[workflow]
name = "k8s-image-live"
version = "1.0.1"

# Bash-less image: busybox has /bin/sh only. Proves the pod starts `/bin/sh`
# and the shim's sh fallback.
[[rules]]
name = "busybox_probe"
shell = "echo busybox-ok > busybox.txt; if [ -x /bin/bash ]; then echo bash-present > busybox-bash.txt; else echo bash-absent > busybox-bash.txt; fi"
output = ["busybox.txt", "busybox-bash.txt"]
environment = { docker = "busybox:1.36" }

# Image mode: the pod IS ubuntu:22.04 (the host runs Ubuntu 24.04).
[[rules]]
name = "image_probe"
shell = """
. /etc/os-release
printf 'VERSION_ID=%s\n' "$VERSION_ID" > os.txt
if command -v docker >/dev/null 2>&1; then echo docker-cli-present > docker-cli.txt; else echo docker-cli-absent > docker-cli.txt; fi
if [ -S /var/run/docker.sock ]; then echo socket-present > socket.txt; else echo socket-absent > socket.txt; fi
echo image-mode-ok > result.txt
"""
output = ["os.txt", "docker-cli.txt", "socket.txt", "result.txt"]
environment = { docker = "ubuntu:22.04" }

# System environment, executed for real: annotated/recorded `system`.
[[rules]]
name = "fallback_probe"
input = ["result.txt"]
shell = "printf 'base-image=%s\n' \"$(. /etc/os-release; echo $VERSION_ID)\" > base.txt"
output = ["base.txt"]

# Image-mode fan-out (two instances).
[[values]]
name = "lane"
values = ["L1", "L2"]

[[rules]]
name = "fanout"
shell = "echo '{lane}' > '{lane}.txt'"
output = ["{lane}.txt"]
environment = { docker = "ubuntu:22.04" }
```

Command (socket channel explicitly unset):

```bash
cd /tmp/oxo-1388-final2
KUBECONFIG=$HOME/.kube/config env -u OXO_FLOW_K8S_DOCKER_SOCKET -u OXO_FLOW_K8S_IMAGE_RENDER \
  ~/oxo-flow/target/debug/oxo-flow run wf.oxoflow --profile k8s --provenance
```

`run-k8s.log` (excerpt):

```text
Info: Using workflow file: wf.oxoflow
Profile: Applied config values from profile 'k8s'
2026-11-25T17:05:36.429328Z  INFO workflow run started (log: /tmp/oxo-1388-final2/.oxo-flow/logs/oxo-flow.log) workflow=k8s-image-live version=1.0.1 git_sha="(none)" workdir=/tmp/oxo-1388-final2
DAG: 5 rules in dependency order
  1. fanout_lane_L2
  2. fanout_lane_L1
  3. image_probe
  4. fallback_probe
  5. busybox_probe
Cluster: submitting 5 jobs to kubernetes (max 2 in flight)
  run directory: /tmp/oxo-1388-final2/.oxo-flow/runs/2026-11-25T17-05-37-588

Cluster: 5 succeeded, 0 failed, 0 skipped
✓ Report snapshot: /tmp/oxo-1388-final2/.oxo-flow/reports/report-20261125-170558.json
```

Outputs on the host workdir (`…-outputs.txt`):

```text
busybox.txt        busybox-ok
busybox-bash.txt   bash-absent
os.txt             VERSION_ID=22.04
docker-cli.txt     docker-cli-absent
socket.txt         socket-absent
result.txt         image-mode-ok
base.txt           base-image=22.04
L1.txt             L1
L2.txt             L2
```

- `busybox-bash.txt = bash-absent` proves the busybox image genuinely has no
  bash; the pod still STARTED (`/bin/sh`) and the shim ran the text with `sh`.
- `os.txt = 22.04` proves the rule executed INSIDE `ubuntu:22.04` (host 24.04).
- `docker-cli-absent` + `socket-absent` prove no daemon/CLI is needed.

### Rendered Jobs (main run)

```text
busybox_probe | image: busybox:1.36 | command: ["/bin/sh", "-c"] | render: image
  args[0]: if command -v bash >/dev/null 2>&1; then exec bash -c "$1"; else exec sh -c "$1"; fi
  args[2]: cd '/tmp/oxo-1388-final2' && echo busybox-ok > busybox.txt; …
image_probe | image: ubuntu:22.04 | command: ["/bin/sh", "-c"] | render: image
  args[0]: <the same bash-if-available shim>
  args[2]: cd '/tmp/oxo-1388-final2' && . /etc/os-release …
fallback_probe | image: ubuntu:latest | command: ["/bin/bash", "-c"] | render: system
  args[0]: cd '/tmp/oxo-1388-final2' && printf 'base-image=%s\n' …
```

No `docker run`, no `oxoflow-docker-sock` volume, no `DOCKER_HOST` env.

## Bash prelude + Indexed Job (review F1)

`/tmp/oxo-1388-prelude/wf-prelude.oxoflow`:

```toml
[workflow]
name = "k8s-image-prelude"
version = "1.0.0"

[defaults]
shell_prelude = "set -euo pipefail"

[[rules]]
name = "bash_single"
shell = "[[ -n \"single\" ]] && echo single-bash-ok > single.txt"
output = ["single.txt"]
environment = { docker = "ubuntu:22.04" }

[[values]]
name = "lane"
values = ["A1", "A2"]

[[rules]]
name = "bash_array"
shell = "[[ -n '{lane}' ]] && echo '{lane}' > '{lane}.txt'"
output = ["{lane}.txt"]
environment = { docker = "ubuntu:22.04" }
```

Run (3 succeeded / 0 failed) and outputs:

```text
Cluster: 3 succeeded, 0 failed, 0 skipped
single.txt   single-bash-ok
A1.txt       A1
A2.txt       A2
```

Rendered commands (ubuntu:22.04 has bash, so the shim selects it):

```text
bash_single | command: ["/bin/sh","-c"]
  args[0]: if command -v bash >/dev/null 2>&1; then exec bash -c "$1"; else exec sh -c "$1"; fi
  args[2]: cd '/tmp/oxo-1388-prelude' && set -euo pipefail
           [[ -n "single" ]] && echo single-bash-ok > single.txt

bash_array (Indexed, completions 2) | command: ["/bin/sh","-c"]
  dispatch: INDEX=$((JOB_COMPLETION_INDEX + 1)); sh -c 'if command -v bash >/dev/null 2>&1; then exec bash "$1"; else exec sh "$1"; fi' sh "/tmp/oxo-1388-prelude/.oxo-flow/runs/2026-11-25T17-06-06-481/jobs/bash_array/chunk-1/cmd.${INDEX}.sh"

cmd.1.sh: cd '/tmp/oxo-1388-prelude' && set -euo pipefail
          [[ -n 'A1' ]] && echo 'A1' > 'A1.txt'
```

Dash would abort at `set: Illegal option -o pipefail` and on `[[ ]]`; both
forms selected bash, so the prelude semantics are preserved in single and
array image-rendered jobs.

## Live scheduler state (`…-kubectl-jobs.json`)

```text
$ KUBECONFIG=$HOME/.kube/config kubectl get job <7 names> -o json
oxoflow-busybox-probe-c560b809 | succeeded: 1 / 1 | image: busybox:1.36 | render: image
oxoflow-image-probe-6ad9b23b   | succeeded: 1 / 1 | image: ubuntu:22.04 | render: image
oxoflow-fanout-lane-l2-e8cdbc7e| succeeded: 1 / 1 | image: ubuntu:22.04 | render: image
oxoflow-fallback-probe-c4693745| succeeded: 1 / 1 | image: ubuntu:latest | render: system
oxoflow-fanout-lane-l1-689b43ff| succeeded: 1 / 1 | image: ubuntu:22.04 | render: image
oxoflow-array-bash-array-b40e264a| succeeded: 2 / 2 | image: ubuntu:22.04 | render: image
oxoflow-bash-single-b1de6f36   | succeeded: 1 / 1 | image: ubuntu:22.04 | render: image
```

(The main run's two fan-out instances were submitted as single Jobs this round;
the prelude run exercised the Indexed-Job path — both paths are image mode.)

## Provenance

Main run (`…-provenance-verify.log`, 9/9 matched):

```text
$ ~/oxo-flow/target/debug/oxo-flow provenance verify .oxo-flow/checkpoint.json
  • environment-lock identity: 4/5 rule(s) resolved
    • busybox_probe: docker|busybox@sha256:73aaf090f3d85aa34ee199857f03fa3a95c8ede2ffd4cc2cdb5b94e566b11662
    ! fallback_probe: system|unavailable
    • fanout_lane_L1: docker|ubuntu@sha256:f64b2ffc…
  • cluster images: 5 rule(s)
    • busybox_probe: image busybox:1.36 (busybox:1.36) — busybox@sha256:73aaf090f3d85aa34ee199857f03fa3a95c8ede2ffd4cc2cdb5b94e566b11662
    • fallback_probe: system environment (no wrapper), pod base image ubuntu:latest
    • fanout_lane_L1: image ubuntu:22.04 (ubuntu:22.04) — ubuntu@sha256:f64b2ffc…
    • fanout_lane_L2: image ubuntu:22.04 (ubuntu:22.04) — ubuntu@sha256:f64b2ffc…
    • image_probe: image ubuntu:22.04 (ubuntu:22.04) — ubuntu@sha256:f64b2ffc…
Summary: 9 matched, 0 mismatched, 0 missing
```

Prelude run (`…-prelude-provenance-verify.log`, 3/3 matched): all three rules
`image ubuntu:22.04 (ubuntu:22.04) — ubuntu@sha256:f64b2ffc…`.

The raw checkpoint `cluster_images` for `fallback_probe` is
`{"render":"system","kind":"system","spec":"","image":"ubuntu:latest"}` — a
system rule is not labelled a wrapper fallback (review M2). The system rule's
`env_lock_identity` remains `system|unavailable`.

**What the digest binds and does not bind.** `resolved_digest` is what the
SUBMIT HOST's local docker daemon reported offline for the rendered reference
(`docker image inspect`, bounded to 5 s and deduped per unique image). It is not
a node-side inspection: it does not prove which digest the k3s runtime pulled or
had cached. A host without the image records no digest and `provenance verify`
says "digest not resolved on the submit host".

## Wrapper fallback + opt-out (renders)

`wf-fallback.oxoflow` (conda + local SIF) — `cluster submit`, one WARN each:

```text
$ ~/oxo-flow/target/debug/oxo-flow cluster submit wf-fallback.oxoflow -b kubernetes -o fallback-scripts
WARN Kubernetes: ... wrapper-fallback ... rule=sif_align environment="singularity"
WARN Kubernetes: ... wrapper-fallback ... rule=conda_qc environment="conda"
```

```text
conda_qc | image: ubuntu:latest | annotations: {"oxoflow.io/environment-kind": "conda", "oxoflow.io/environment-render": "wrapper-fallback"}
  args[0]: conda run --no-capture-output -n qc bash -c '…; fastqc input.fq'
sif_align | image: ubuntu:latest | annotations: {"oxoflow.io/environment-kind": "singularity", "oxoflow.io/environment-render": "wrapper-fallback"}
  args[0]: apptainer exec --bind '…' '/shared/containers/bwa.sif' sh -c '…' sh 'bwa mem ref.fa input.fq'
```

These two were rendered, not executed. The fallback pod's base image is
`ubuntu:latest` unless the rule also declares `docker`. For the conda family
(`mamba`/`conda`/`pixi` rank ABOVE `docker` in `EnvironmentSpec::kind()`), a
`docker = "<image with conda>"` field keeps the conda wrapper and supplies a
runtime-bearing base image, so that route IS runnable. For a local SIF and for
`venv` (both rank BELOW `docker`), adding `docker` flips the environment to
image mode and the SIF/venv is ignored; `modules` + `docker` is rejected by
validation. Those three cannot acquire a compatible base image today — bake the
environment into an image and declare `docker = …`, or run on a local/HPC
backend. The executed fallback here was the `system` rule above.

**Opt-out** — `OXO_FLOW_K8S_IMAGE_RENDER=0 cluster submit wf.oxoflow …`
(`…-no-image-render.log`, 4 WARNs) renders the docker rules as the pre-#1388
wrapper fallback:

```text
opt-out busybox | image: busybox:1.36 | command: ["/bin/bash", "-c"]
  annotation: {"oxoflow.io/environment-kind": "docker", "oxoflow.io/environment-render": "wrapper-fallback"}
  wrapper present: True | args starts: docker run --rm --user $(id -u):$(id -g) -v '/tmp/oxo-1388-final2':'/t…
```

## Registry-mirror rewrite timing (`…-mirror-render.log`, `…-mirror-manifest.json`)

```text
$ OXO_REGISTRY_MIRRORS="docker.io=mirror.example.com" \
    ~/oxo-flow/target/debug/oxo-flow cluster submit wf-mirror.oxoflow -b kubernetes -o mirror-scripts
image: mirror.example.com/biocontainers/bwa:0.7.17
annotations: {"oxoflow.io/environment-image": "biocontainers/bwa:0.7.17",
              "oxoflow.io/environment-kind": "docker",
              "oxoflow.io/environment-render": "image"}
```

The rendered manifest is committed raw as
`…-mirror-manifest.json`, so the image and annotation claims are re-derivable.

## What this proves / honest residual

Proven:

- Image mode runs a docker-environment rule with **no docker CLI, no node
  daemon socket and no `OXO_FLOW_K8S_DOCKER_SOCKET`**, inside the declared
  image (`os.txt` = 22.04 on a 24.04 host).
- The replaced wrapper's shell contract is preserved: a bash-less busybox image
  starts and executes via the shim's `sh` fallback, while a
  `set -euo pipefail` + `[[ ]]` workflow succeeds in a single Job AND a
  2-element Indexed Job via the shim's bash selection (review F1).
- System rules are `system`, not `wrapper-fallback` (review M2); fallbacks are
  annotated/warned; `OXO_FLOW_K8S_IMAGE_RENDER=0` reproduces the pre-#1388
  wrapper manifest (review M3).
- Provenance records per-rule image/fallback/system decisions with the
  image reference and an offline digest only when actually resolved; the
  raw `kubectl` JSON, outputs, and render logs are committed (review M5).

Not proven / residual:

- Single-node k3s with `hostPath` workdir mounts. Multi-node scheduling,
  NFS/PVC combined with image mode, and cross-node image availability are not
  exercised here.
- The recorded digest is submit-host docker evidence, not node-side proof.
- Fallback executability depends on precedence: conda/mamba/pixi can run on
  Kubernetes when a `docker = "<runtime image>"` base image is declared
  (conda ranks above docker); local-SIF/venv fallbacks cannot acquire a base
  image (docker wins `kind()`), and `modules` + `docker` is rejected — those
  are rendered + warned but not runnable; bake an image or use local/HPC. No
  conda-family fallback was executed live (only rendered/warned).
- Image-mode pods run as the image's default user (root in stock images);
  hostPath outputs are root-owned. Documented in run-on-cluster.md's Limits.
- Only Kubernetes consumes image rendering today; AWS/GCP/Tencent keep the
  wrapper (the `renders_container_image`/`submitted_cmd` seam is the adoption
  point).
- The live run used a debug build (oxo-flow 0.32.0, commit `23aab914`; the
  binary was built from the same code tree, later changes are docs/evidence only).

## Reproduction sketch

```bash
# on tx-ubuntu (k3s host)
git clone <repo> ~/oxo-flow-1388 && cd ~/oxo-flow-1388
git switch feat/1388-image-env-rendering
PATH=$HOME/.cargo/bin:$PATH cargo build -p oxo-flow-cli

mkdir -p ~/.kube && sudo cat /etc/rancher/k3s/k3s.yaml > ~/.kube/config && chmod 600 ~/.kube/config
docker pull busybox:1.36 && docker save busybox:1.36 | sudo k3s ctr images import -

mkdir -p /tmp/oxo-1388-final2/profiles /tmp/oxo-1388-prelude/profiles   # copy the workflows + profiles/k8s.toml
cd /tmp/oxo-1388-final2
KUBECONFIG=$HOME/.kube/config env -u OXO_FLOW_K8S_DOCKER_SOCKET -u OXO_FLOW_K8S_IMAGE_RENDER \
  ~/oxo-flow-1388/target/debug/oxo-flow run wf.oxoflow --profile k8s --provenance
~/oxo-flow-1388/target/debug/oxo-flow provenance verify .oxo-flow/checkpoint.json
cd /tmp/oxo-1388-prelude
KUBECONFIG=$HOME/.kube/config env -u OXO_FLOW_K8S_DOCKER_SOCKET -u OXO_FLOW_K8S_IMAGE_RENDER \
  ~/oxo-flow-1388/target/debug/oxo-flow run wf-prelude.oxoflow --profile k8s --provenance
```
