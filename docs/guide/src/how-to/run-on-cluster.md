# Run on a Cluster

This guide explains how to execute oxo-flow workflows on HPC and cloud backends: the schedulers SLURM, PBS, SGE, and LSF, plus Kubernetes, AWS Batch, GCP Batch, and Tencent Batch for cloud runs.

---

## Overview

oxo-flow's cluster module translates each rule into a cluster job submission. Resource requirements declared in the `.oxoflow` file (`threads`, `memory`, `gpu`, `time_limit`) are mapped to the appropriate scheduler directives. The `disk` field is **not** mapped to any scheduler directive — it only produces a local warning during `oxo-flow run`.

There are two ways to reach a scheduler, and they suit different jobs:

| | What it does | Use when |
|---|---|---|
| `run --profile <NAME>` | Submits, tracks jobs to completion, and updates the checkpoint | Normal execution — you want the workflow to run |
| `cluster submit` | Writes job scripts and stops | You want to inspect or hand-edit scripts before submitting |

`run --profile` is the everyday path; it inherits everything `run` does — wildcard expansion, checkpoint/resume, `--samples`, `--rerun`, config-change invalidation. See [Cluster submission](../commands/run.md#cluster-submission) for the `[cluster]` profile block. `cluster submit` remains the escape hatch and is documented below.

**Environment wrapping is applied automatically** — conda, mamba, docker, singularity/apptainer, pixi, venv, and modules environments are wrapped in the generated scripts. Each rule applies **one** backend (the first declared in the resolver order mamba → conda → pixi → docker → singularity → venv → modules), so declaring e.g. both `singularity` and `modules` fails loudly instead of silently dropping the module loads.

---

## Supported Schedulers

| Scheduler | Status | Maturity | Directive prefix |
|---|---|---|---|
| **SLURM** | Supported | Production deployments + CI mock scheduler (operational evidence; not a per-commit live campaign) | `#SBATCH` |
| **PBS/Torque** | Supported | Real-scheduler matrix, OpenPBS 23.06 (2026-09-22/23 campaign; not re-run per commit) | `#PBS` |
| **SGE** | Supported | Real-scheduler matrix, SoGE 8.1.9 (2026-09-22/23 campaign; not re-run per commit) | `#$` |
| **LSF** | Supported | Experimental — no live campaign; directives/status semantics cross-checked against IBM's bsub reference | `#BSUB` |
| **Kubernetes** | Supported (spec generation + shared-filesystem execution) | Prototype — **live two-node k3s campaign (2026-11-26)**: k3s server + agent with separate kernels, a network NFSv4.2 export behind `[cluster.volume] kind = "nfs"`, the run workdir inside the export, an Indexed array settling per element across both nodes, a dependent 2048 MiB rule forced onto the second node by resource fit, outputs returned to the submit host through the volume, and `provenance verify` 3/3 — see [`kubernetes-multinode-2026-11-26.md`](../../../superpowers/audits/kubernetes-multinode-2026-11-26.md). Earlier: live single-node k3s campaigns (submit/status/artifacts/logs/SIGINT-cancel, and image-based environment rendering) for `system`- and container-environment rules; in single-node mode Job manifests mount the run workdir AND the array command directory via `hostPath`, set `imagePullPolicy: IfNotPresent`, and rewrite the container image through `OXO_REGISTRY_MIRRORS` (namespace/registry-qualified names). **Container environments are rendered as the pod image** (docker specs and singularity `docker://` URIs — see [Container environments on Kubernetes](#container-environments-on-kubernetes)): no docker CLI or node socket is needed. Non-renderable environments (local SIF, conda/pixi/venv/modules) keep the folded local wrapper and are marked `wrapper-fallback` (a `system` rule is marked `system`); `OXO_FLOW_K8S_IMAGE_RENDER=0` forces that fallback box-wide, and a fallback job whose command must reach the node daemon can opt in to `OXO_FLOW_K8S_DOCKER_SOCKET=1` (mounts the node daemon socket — trusted single-operator clusters only; never attached to an image-rendered pod). Multi-node clusters: declare a shared filesystem with `[cluster.volume]` (`nfs` live-qualified; `pvc` render-tested only, no live claim/provisioner drill — see [Cluster submission](../commands/cluster.md#shared-volume-clustervolume-kubernetes)) and pods mount it at `mount_root` instead of any submit-host path; `hostpath` keeps single-node semantics. Residuals: image mode combined with a multi-node `[cluster.volume]` run is single-node-proven only (see [`kubernetes-image-render-2026-11-25.md`](../../../superpowers/audits/kubernetes-image-render-2026-11-25.md)); PVC live qualification; multi-control-plane/HA and cloud-provider volumes — see [`kubernetes-live-campaign-2026-11-24.md`](../../../superpowers/audits/kubernetes-live-campaign-2026-11-24.md) | `k8s Job` |
| **AWS Batch** | Supported (spec generation) | Prototype — CLI wrapper (`aws`), mock/wire tests only; live campaign pending (#1390). Submit/describe/list/cancel shapes verified against the botocore Batch service model and the AWS CLI v2 reference (2026-11-25 audit); array child ids, `terminate-job` cancel, and the 100-id DescribeJobs cap are pinned by wire tests — [Audit evidence](../../../superpowers/audits/cloud-cli-accuracy-2026-11-26.md). | `aws-batch` |
| **GCP Batch** | Supported (spec generation) | Prototype — CLI wrapper (`gcloud`), mock/wire tests only; live campaign pending (#1390; endpoint unreachable from the qualification host). Submit (plain `--config` path, minted positional id)/describe/tasks-list/cancel shapes verified against the gcloud reference and `google/cloud/batch/v1` protos (2026-11-25 audit); cancel uses `CancelJob`, not `DeleteJob` — [Audit evidence](../../../superpowers/audits/cloud-cli-accuracy-2026-11-26.md). | `gcp-batch` |
| **Tencent Batch** | Supported (spec generation + live core path) | Prototype — CLI wrapper (`tccli batch`, API 2017-03-12); payload/state/cancel/log shapes verified against tccli's `api.json`/`examples.json`. **Live campaign 2026-11-26** (issue #1389): a named compute environment (`TENCENTBATCH_ENV_ID`) submit ran to `SUCCEED`; state poll, `DescribeTaskLogs` decode, and `TerminateJob` cancel verified; inline job-level env also provisioned on a second account; a single-rule engine run exposed the submit-host `cd` defect fixed in PR #1421 (cloud commands are stripped or mapped onto `[cluster.volume].mount_root`; arrays require the volume and fail closed otherwise). Array/artifact-pull and cross-region evidence remain outstanding; arrays settle on the base job verdict (no per-instance `DescribeTask`) — [Live evidence](../../../superpowers/audits/tencent-batch-live-2026-11-26.md), [static audit](../../../superpowers/audits/cloud-cli-accuracy-2026-11-26.md). | `tencent-batch` |

**Per-element outcomes.** Array elements settle on their own scheduler record whenever the backend exposes one: SLURM `squeue` element ids, Kubernetes Indexed Job completion indexes (`status.completedIndexes` / `status.failedIndexes`), AWS Batch child job ids (`{parent}:{arrayIndex}`), and GCP Batch task indexes (`gcloud batch tasks list`; `UNEXECUTED` tasks settle failed). One element failing never turns its successful siblings into failures, and a missing accounting record leaves the exit code unknown rather than reporting `0`. PBS/LSF/SGE and Tencent Batch are polled by the array base id in this adapter (Tencent's per-instance records sit behind a separate `DescribeTask` call); their per-instance records name that aggregate provenance in `skip_reason` and carry no base-level exit code or resource numbers, so an array-level verdict is never presented as an independently observed element outcome.

AWS Batch and GCP Batch are not qualified by a live campaign, and Tencent Batch has live submit/poll/log/cancel evidence but no live array or artifact-pull campaign: every cloud backend's maturity label stays **Prototype** until the CL-05 (#1158) fail/retry/artifact matrix exists.

### Cloud backend profile keys

The shared `[cluster]` keys carry a backend-specific meaning on cloud
backends, the same way `partition` names a scheduler partition on the HPC
backends:

| Backend | `partition` | `account` | Environment knobs |
|---|---|---|---|
| `aws-batch` | `SubmitJob.jobQueue` (default `oxoflow-queue`) | `SubmitJob.jobDefinition` (default `oxoflow-job-def`) | The job definition must carry the `OXO_FLOW_SENSITIVE_<n>` names (`containerProperties.secrets` or `environment`); SubmitJob has no secrets field |
| `gcp-batch` | `--location` (region, e.g. `us-central1`) | — | `OXO_FLOW_SECRET_REFS` projects routed names into `taskSpec.environment.secretVariables` |
| `tencent-batch` | `Placement.Zone` (**required**, e.g. `ap-shanghai-5`; the `tccli --region` is derived from it) | CVM `ImageId` the batch nodes boot (**required**; list with `tccli cvm DescribeImages`) | `TENCENTBATCH_INSTANCE_TYPE` (default `S5.MEDIUM4`), `TENCENTBATCH_ENV_ID` (target a pre-created compute environment instead of an inline anonymous one) |

`cluster status` / `cluster cancel` take the same value through
`--queue` where the cloud CLI needs it to resolve a bare job id (the GCP
location, the Tencent zone). AWS Batch's raw listing uses it as the
`list-jobs --job-queue` selector, falling back to the submit default.
`cluster logs` takes `--queue` for the same reason on GCP/Tencent; AWS reads
its region from the AWS config.

Cloud compute nodes do not share the submit host's filesystem. A cloud rule
without `[cluster.volume]` runs in the vendor agent's default directory (the
engine drops the submit-host `cd` guard instead of failing on a missing
path); file inputs and outputs still need a shared mount. Declare
`[cluster.volume]` whose `volume_root` contains the run workdir and whose
`mount_root` is the path the compute nodes have the volume mounted at — the
rendered command is rewritten onto it. Cloud **array** jobs require that
declaration: their per-index `cmd.N.sh` files are read from the command
directory on every node and are not uploaded to the vendor, so an array
without a shared volume is rejected before submission.

---

## Container environments on Kubernetes

A rule's **container environment becomes the pod image** (issue #1388) instead of
a local container runtime folded into the command:

| Declared environment | How the Kubernetes job runs | Node needs |
|---|---|---|
| `docker = "<image>"` | Image mode: pod `image:` = `<image>`; the rule command runs directly in it | Nothing — no docker CLI, no daemon socket |
| `singularity = "docker://<image>"` | Image mode: pod `image:` = the underlying `<image>` | Nothing |
| `singularity = "/shared/x.sif"` (or any non-`docker://` URI) | Wrapper fallback: the folded `singularity exec …` runs in the pod | None available: the base is `ubuntu:latest` (no singularity), and adding `docker` wins `kind()` → image mode; bake the SIF into an image |
| `conda` / `mamba` / `pixi` | Wrapper fallback: the folded wrapper runs in the pod | Declare `docker = "<image with the runtime>"` too — these rank above `docker`, so the wrapper stays and the image supplies the runtime; without it `ubuntu:latest` has no conda |
| `venv` | Wrapper fallback: `source …/activate` runs in the pod | None available: `docker` outranks `venv`, so adding an image flips to image mode; bake the venv into an image or run locally |
| `modules` | Wrapper fallback: `module load` runs in the pod shell | None available: `modules` + `docker` is rejected by validation and `ubuntu:latest` has no module system; run on HPC |
| no environment (`system`) | No wrapper exists: the command runs in `ubuntu:latest`; annotated `system` | — |

**Shell contract.** Image mode preserves the shell contract of the local
container wrapper it replaces: the pod starts `/bin/sh` (always present — a
busybox/alpine image included) and runs the rule text through the same
bash-if-available shim (`bash -c` when the image has bash, else `sh -c`), so
`[defaults] shell_prelude = "set -euo pipefail"` and other bashisms keep
working in images that carry bash. Indexed Jobs apply the same choice to each
per-index command file. A fallback job keeps the historical `/bin/bash -c`
dispatcher.

**What is visible in the artifact.** Every rendered Job carries annotations that
state the decision, so a manifest review does not have to infer it:

```yaml
metadata:
  annotations:
    oxoflow.io/environment-render: image              # or "wrapper-fallback" / "system"
    oxoflow.io/environment-kind: docker               # the backend that supplied it
    oxoflow.io/environment-image: biocontainers/bwa:0.7.17   # image mode only
```

A wrapper fallback also logs one `WARN` per rendered Job naming the rule and
environment kind. A fallback job whose command must reach the node daemon can
opt in to the `OXO_FLOW_K8S_DOCKER_SOCKET=1` fallback channel — it mounts the
node's `/var/run/docker.sock` into the pod (host-root-equivalent; trusted
single-operator clusters only). The channel is never attached to an
image-rendered rule: that pod cannot benefit and must not receive the socket.

**Forcing the fallback.** `OXO_FLOW_K8S_IMAGE_RENDER=0` (also `false`, `no`,
`off`) turns image rendering off for the whole box: a docker-environment rule
then keeps the folded `docker run` wrapper exactly like pre-#1388 — pair it with
`OXO_FLOW_K8S_DOCKER_SOCKET=1` and an image that carries the docker CLI when the
rule needs host paths outside the workdir or a nested container. The fallback
keeps the pre-#1388 `/bin/bash -c` dispatcher, so that image must carry bash
(and a docker CLI for the folded wrapper). Under this opt-out the per-Job WARN
says rendering is disabled — not that it is impossible. Truthy values keep
rendering on; an unrecognized value warns and stays on.

**Fallback reality check.** The fallback pod's base image is `ubuntu:latest`
unless the rule ALSO declares `docker`, which the fallback then uses as the pod
base image. Whether that stays a fallback depends on `EnvironmentSpec::kind()`
precedence: `mamba` / `conda` / `pixi` rank ABOVE `docker`, so
`environment = { conda = "envs/qc.yaml", docker = "<image with conda>" }` keeps
the conda wrapper and supplies a runtime-bearing base image (runnable). A local
SIF and `venv` rank BELOW `docker` — adding an image flips them to image mode
and the SIF/venv is ignored; `modules` + `docker` is rejected by validation.
Those three cannot acquire a compatible base image today: bake the environment
into an image and declare `docker = "<image>"`, or run the rule on a local/HPC
backend. The fallback remains functional for conda-family rules with a
runtime-bearing base image, for a docker rule that opts out (above), and for a
`system` rule that invokes docker itself.

Only Kubernetes consumes image rendering today. The plan seam
(`ScheduledRule::container_image` + `ExecutorBackend::renders_container_image`)
is backend-agnostic, so AWS Batch / GCP Batch / Tencent Batch can adopt it
without re-deriving the environment.

**Registry mirrors.** `OXO_REGISTRY_MIRRORS` is applied when the Job's `image:`
field is rendered (see [China network mirrors](../reference/china-mirrors.md)).
The annotation and the checkpoint record keep the workflow-declared spec, so the
identity does not change on a mirrored box.

**Provenance.** The cluster path records each planned rule's image identity in the
run checkpoint (`cluster_images`), and `oxo-flow provenance verify` prints
it: the rendered `image:` reference, the workflow-declared spec, and the
image digest **only when the submit host's local docker daemon could report one
offline**. A missing digest is reported as exactly that ("digest not resolved on
the submit host") — it does not prove what a compute node pulled. A fallback
entry says `wrapper-fallback` and names its pod base image; no image identity is
claimed for it.

**Limits.** In image mode the pod sees only the run workdir (or the
`[cluster.volume]` mount). Absolute host paths outside that tree that the local
docker backend would bind-mount are not mounted — stage reference data onto the
shared volume, or force the wrapper fallback with
`OXO_FLOW_K8S_IMAGE_RENDER=0` + `OXO_FLOW_K8S_DOCKER_SOCKET=1`. A configured
`[engine] tmp_dir` / `OXO_FLOW_TMPDIR` is exported into the pod command like on
every other backend, so it must name a directory the pod can see (put it under
the workdir or the shared volume). Image-mode pods run as the image's default
user — root in most stock images — with no `securityContext` rendered, so files
they create on a `hostPath` workdir are root-owned on the submit host (the local
docker wrapper runs as the invoking uid instead). Use an image whose `USER`
matches the run uid, a `pvc`/`nfs` volume with suitable ownership/permissions,
or the fallback path when host ownership matters. Building conda/pixi
environments into images automatically is out of scope; bake the image
explicitly and declare it as `docker = …`.

---

## Declaring Resources

Set resource requirements per rule:

```toml
[[rules]]
name = "align"
input = ["{sample}_R1.fastq.gz"]
output = ["aligned/{sample}.bam"]
environment = { singularity = "docker://biocontainers/bwa:0.7.17" }
shell = "bwa mem -t {threads} ref.fa {input} | samtools sort -o {output}"

[rules.resources]
threads = 16
memory = "32G"
time_limit = "24h"
```

> **Note on `gpu`:** `gpu = 0` means "no GPUs" — the engine omits the GPU
> directive entirely, exactly as if the field were absent. Only declare it
> when you need at least one GPU (`gpu = 1` or higher).

### Resource fields

| Field | Type | Example | Description |
|---|---|---|---|
| `threads` | Integer | `16` | Number of CPU cores |
| `memory` | String | `"32G"` | RAM allocation |
| `gpu` | Integer | `1` | Number of GPUs (simple count) |
| `gpu_spec` | Table | See below | Detailed GPU specification |
| `disk` | String | `"100G"` | Local disk space — **local warning only**, never emitted as a scheduler directive |
| `time_limit` | String | `"24h"` | Wall-time limit |

### Memory and walltime conventions

One engine-wide parser reads both `memory` and `disk`, so what you see in
`dry-run`/`run` resource output is what the scheduler gets:

- **Bare numbers are megabytes** (`memory = "4096"` = 4 GB) — on every
  backend. LSF's own default unit is KB, so oxo-flow converts the parsed
  figure ×1024 and submits an explicit KB number to `-M` /
  `rusage[mem=…]`; the same config cannot schedule 4 GB locally and 4 MB
  on the cluster.
- Accepted forms: a bare number, or a single-letter `K`/`M`/`G`/`T`
  suffix (`"4096"`, `"8G"`, `"16384M"`, `"1T"`). Fractional (`"16.5G"`)
  and multi-letter-suffix (`"4096MB"`, `"2GB"`) figures parse to nothing
  engine-wide and are **rejected at validate time** — the same predicate
  `check`'s [E004](../reference/glossary.md#diagnostic-codes) diagnostic applies, so a config that loads is a config
  the pool, the quota, and every scheduler render agree on. Because
  PBS `mem=` and SGE `h_vmem=` default to their own units (words/bytes),
  those two backends receive the figure with an **explicit unit**
  (`mem=8192mb`, `h_vmem=16384M`); SLURM's `--mem` and LSF's converted
  KB figure need no help (their defaults match or are converted).
- `time_limit` accepts the suffix forms `90s`, `30m`, `24h`, `2d` and the
  scheduler-style colon forms `01:00:00`, `24:00:00`, `1-06:00:00` — one
  shared parser for the local executor and every backend, so the same
  value means the same limit everywhere. Bare numbers (`"1500"`) are
  **rejected at validate time**: ambiguous between seconds and LSF
  minutes — write `"1500s"` or `"25m"`.
- A **present but unparseable** walltime is never silently replaced by a
  default: every backend renders it verbatim (with a warning in the log),
  so the scheduler rejects the script loudly instead of killing a long
  job at a hidden 1-hour limit. The local executor logs a warning and
  falls back to the global timeout.

### GPU Specification

For basic GPU requests, use the `gpu` field:

```toml
[rules.resources]
gpu = 2  # Request 2 GPUs
```

For advanced GPU configuration, use `gpu_spec`:

```toml
[rules.resources.gpu_spec]
count = 2
model = "a100"       # GPU model (optional — SLURM only)
memory_gb = 40       # Per-GPU memory in GB (optional — SLURM only)
```

`count` works on SLURM, PBS, and SGE; `model` and `memory_gb` only affect
SLURM directives (LSF ignores `gpu_spec` entirely).

Different schedulers handle GPU requests differently:

| Scheduler | GPU Directive | Notes |
|-----------|---------------|-------|
| **SLURM** | `--gres=gpu:2` or `--gres=gpu:a100:2:40g` | Full support for model and memory spec |
| **PBS** | `gpu=2` | Basic count only; model selection varies by site |
| **SGE** | `-l gpu=2` | Basic count only; requires queue configuration |
| **LSF** | `-gpu "num=2"` | Basic count only (rendered as a quoted `num=` run-options string) |

---

## SLURM Example

oxo-flow generates SLURM job scripts automatically. For the `align` rule
above with a sample group `batch = ["S1", "S2", "S3"]`, the script generated
for the first instance is:

```bash
#!/bin/bash
#SBATCH --chdir=/abs/path/to/workdir
#SBATCH --job-name=align_batch_S1
#SBATCH --cpus-per-task=16
#SBATCH --mem=32G
#SBATCH --time=1-00:00:00
#SBATCH --output=logs/align_batch_S1.out
#SBATCH --error=logs/align_batch_S1.err

set -e

mkdir -p logs
apptainer exec --bind /abs/path/to/workdir:/abs/path/to/workdir docker://biocontainers/bwa:0.7.17 sh -c 'if command -v bash >/dev/null 2>&1; then exec bash -c "$1"; else exec sh -c "$1"; fi' sh 'bwa mem -t 16 ref.fa S1_R1.fastq.gz | samtools sort -o aligned/S1.bam'
```

Note: one script is generated per **rule instance**. Wildcards expand before
the scripts are written, exactly as they do for `run`, so a 3-sample scatter
produces `align_batch_S1.sh`, `align_batch_S2.sh`, and `align_batch_S3.sh`
with concrete paths in each. The instance names match the ones `dry-run`
plans, so a script maps back to a planned rule by name. The `sh -c '…' sh
'<command>'` tail is the bash shim described under
[Environment Wrapping](#environment-wrapping), and `singularity` is
substituted when `apptainer` is absent.

Note the workdir pinning at the top of each script (`--chdir` / `cd` /
`-wd` / `-cwd`): without it SLURM starts a job in the submission
directory, PBS in `$HOME`, and SGE in a queue-configured directory, so a
rule's relative output paths would silently land outside the run
directory. The examples below show the pin for each backend.

---

## PBS Example

```bash
#!/bin/bash
#PBS -N align_batch_S1
#PBS -l nodes=1:ppn=16,mem=32768mb,walltime=1-00:00:00
#PBS -o logs/align_batch_S1.out
#PBS -e logs/align_batch_S1.err
cd /abs/path/to/workdir || exit 1

set -e

mkdir -p logs
apptainer exec --bind /abs/path/to/workdir:/abs/path/to/workdir docker://biocontainers/bwa:0.7.17 sh -c 'if command -v bash >/dev/null 2>&1; then exec bash -c "$1"; else exec sh -c "$1"; fi' sh 'bwa mem -t 16 ref.fa S1_R1.fastq.gz | samtools sort -o aligned/S1.bam'
```

---

## SGE Example

```bash
#!/bin/bash
#$ -wd /abs/path/to/workdir
#$ -N align_batch_S1
#$ -pe smp 16
#$ -l h_vmem=32768M
#$ -l h_rt=1-00:00:00
#$ -o logs/align_batch_S1.out
#$ -e logs/align_batch_S1.err

set -e

mkdir -p logs
apptainer exec --bind /abs/path/to/workdir:/abs/path/to/workdir docker://biocontainers/bwa:0.7.17 sh -c 'if command -v bash >/dev/null 2>&1; then exec bash -c "$1"; else exec sh -c "$1"; fi' sh 'bwa mem -t 16 ref.fa S1_R1.fastq.gz | samtools sort -o aligned/S1.bam'
```

---

## LSF Example

```bash
#!/bin/bash
#BSUB -cwd /abs/path/to/workdir
#BSUB -J align_batch_S1
#BSUB -n 16
#BSUB -R 'rusage[mem=33554432] span[hosts=1]'
#BSUB -M 33554432
#BSUB -W 24:00
#BSUB -o logs/align_batch_S1.out
#BSUB -e logs/align_batch_S1.err

set -e

mkdir -p logs
apptainer exec --bind /abs/path/to/workdir:/abs/path/to/workdir docker://biocontainers/bwa:0.7.17 sh -c 'if command -v bash >/dev/null 2>&1; then exec bash -c "$1"; else exec sh -c "$1"; fi' sh 'bwa mem -t 16 ref.fa S1_R1.fastq.gz | samtools sort -o aligned/S1.bam'
```

The memory figure is explicit KB, and `span[hosts=1]` rides in the `-R`
string — why both are emitted that way is explained once in the
[cluster command's LSF script section](../commands/cluster.md#lsf-script).
`-W 24:00` is the parsed form of the
rule's `time_limit = "24h"` — LSF takes `[HH:]MM`. The workdir pin is
explicit here too: the generated script carries `#BSUB -cwd`, the same
pinned-directory directive the other backends get.

---

## Environment Wrapping

When generating cluster scripts, oxo-flow automatically wraps commands
through the environment resolver (rules without an environment run the
command directly). This table is the single source for the per-backend
wrapping forms — other pages link here instead of restating them:

| Backend | Wrapping |
|---|---|
| Conda | `conda run --no-capture-output -n <env> bash -c 'export PATH="$CONDA_PREFIX/bin:$PATH"; <command>'` |
| Mamba | `<mamba\|micromamba\|conda> run -n <env> bash -c 'export PATH="$CONDA_PREFIX/bin:$PATH"; <command>'` (`--no-capture-output` added when the detected binary is conda) |
| Docker | `docker run --rm --user $(id -u):$(id -g) -v <workdir>:<workdir> -w <workdir> <image> sh -c '<shim>' sh '<command>'` |
| Singularity / Apptainer | `<apptainer\|singularity> exec --bind <workdir>:<workdir> <image> sh -c '<shim>' sh '<command>'` |
| Pixi | `pixi run --manifest-path <pixi.toml> bash -c '<command>'` |
| Venv | `source <venv>/bin/activate && <command>` |
| Modules | module-init block, then `module load <mod1> <mod2> && <command>` |

Three details worth knowing:

- **Container paths are absolute.** `docker -v` and `singularity --bind`
  reject relative sources, and a scheduler may start a job from any
  directory, so oxo-flow renders the resolved workdir. The rule's command is
  handed to a shim inside the container —
  `if command -v bash >/dev/null 2>&1; then exec bash -c "$1"; else exec sh -c "$1"; fi` —
  because the image's entrypoint shell runs first, and multi-line or
  `pipefail`-using scripts need bash, which not every image ships.
- **Paths outside the workdir are bound read-only (Docker backend).** A
  `[config]` reference genome or index living elsewhere on the shared
  filesystem is appended to the Docker mount list automatically
  (`-v /data/ref:/data/ref:ro`), so the rule can read it from inside the
  container. The Singularity/Apptainer backend does not add these mounts —
  if your site needs them, keep inputs inside the workdir or wrap the
  command yourself. (There is no `[cluster.container]` sub-table: the
  `[cluster]` profile block accepts exactly eight keys — `account`,
  `backend`, `extra_args`, `max_array_size`, `max_submitted`, `partition`,
  `poll_interval`, `walltime`.)
- **`apptainer` wins when both it and `singularity` are installed**, so
  clusters that renamed the binary keep working without configuration.

### Environment Examples

**Conda with GPU for deep learning:**

```toml
[[rules]]
name = "train_model"
input = ["data/train.h5"]
output = ["models/trained.pt"]
environment = { conda = "envs/pytorch.yaml" }
shell = "python train.py --input {input} --output {output}"

[rules.resources]
threads = 8
memory = "64G"
gpu = 2
time_limit = "24h"
```

The `gpu` field controls the **scheduler allocation only** (e.g.
`#SBATCH --gres=gpu:2`) — there is no `{resources.gpu}` shell placeholder.

**Singularity (common on HPC):**

```toml
[[rules]]
name = "variant_call"
input = ["aligned/{sample}.bam"]
output = ["variants/{sample}.vcf"]
environment = { singularity = "docker://broadinstitute/gatk:4.4.0.0" }
shell = "gatk HaplotypeCaller -I {input} -O {output}"

[rules.resources]
threads = 16
memory = "32G"
```

Only **one** backend applies per rule, so combining e.g. `singularity` with
`modules` in the same rule is rejected outright (the module loads would be
silently dropped) — use the module-based example below for module-only
setups.

**Pixi for reproducible environments:**

```toml
[[rules]]
name = "qc_check"
input = ["{sample}.fastq.gz"]
output = ["qc/{sample}_fastqc.html"]
environment = { pixi = "envs/pixi.toml" }  # the manifest FILE, not an env name
shell = "fastqc -t {threads} -o qc/ {input}"

[rules.resources]
threads = 4
```

**Pure Module-based (traditional HPC):**

```toml
[[rules]]
name = "align"
input = ["reads/{sample}.fq"]
output = ["aligned/{sample}.bam"]
environment = { modules = ["bwa/0.7.17", "samtools/1.17", "gcc/11"] }
shell = "bwa mem -t {threads} ref.fa {input} | samtools sort -o {output}"

[rules.resources]
threads = 32
memory = "64G"
```

!!! tip "Pre-build environments on cluster nodes"
    Ensure your conda environments, docker images, or singularity containers are available on all cluster nodes before submitting jobs. Use `--skip-env-setup` when environments are pre-built.

---

## Resource Enforcement

### Local Execution

When running locally (`oxo-flow run`), resource constraints are enforced:

- **Check**: Before execution, verify resources are available
- **Reserve**: Reserve resources before starting the job
- **Release**: Release resources after completion (or on failure/timeout)

```bash
# Limit to 16 threads and 32GB memory for local execution
oxo-flow run pipeline.oxoflow --max-threads 16 --max-memory 32768
```

### Cluster Execution

On clusters, the scheduler enforces resources based on the generated directives. oxo-flow does not manage resources during cluster execution — the scheduler handles that.

---

## Best Practices

!!! tip "Use Singularity on clusters"
    Most HPC clusters do not allow Docker. Use Singularity instead — oxo-flow handles the conversion automatically when you specify `singularity = "docker://..."`.

!!! tip "Set realistic time limits"
    Generous wall-time limits prevent premature job termination but may lower scheduling priority. Profile your jobs first.

!!! note "Failure handling on the cluster path"
    `--keep-going` / `-k` is honored on the cluster path (issue #1093).
    By default a `required = true` rule's failure cancels every in-flight
    job (`scancel`/`qdel`/`bkill`) and the run exits non-zero. With `-k`
    the failure is still recorded and still fails the run, but jobs
    already in flight run to completion and the driver keeps dispatching
    ready independent instances until the queue drains; dependents of the
    failed rule stay blocked (never submitted). Interrupts
    (Ctrl-C / SIGTERM / SIGHUP) and driver-internal errors always cancel
    every in-flight job regardless of `-k`. A `required = false` failure
    never cancels either way — mark salvageable steps `required = false`
    (best-effort QC, optional per-sample stages) when their failure
    should leave the exit code at 0: the failure is recorded, the run
    exits 0 with the failures listed, and re-running with `resume` (the
    standalone command that takes a checkpoint path) or `run
    --resume-failed` re-queues what failed.

!!! note "Submission failure semantics"
    A job the scheduler has accepted is never dropped silently. Once
    `sbatch`/`qsub`/`bsub`-style submission returns a job id, the driver
    registers the job in memory and then writes its local records
    (`jobs/<rule>/job.id`, `status.json`, and `index.json` for arrays). If
    any of those writes fails — full disk, permissions, a directory in
    place of a file — the driver cancels every accepted scheduler job and
    fails the run with the scheduler id(s) named in the error. When the
    cancel itself cannot be confirmed, the error says so explicitly
    (`failed to cancel N scheduler job(s) — they may still be running:
    <ids>`) and never reports those jobs as cancelled; cancel them
    manually before re-running. Array submissions follow the same rule:
    elements are registered before the bookkeeping writes, and a failed
    registration cancels the whole array through its base id.

    One window is outside the engine's reach: if the scheduler accepts a
    job but prints no parseable id, there is no handle to cancel. The run
    fails with the scheduler's own output quoted in the error so the job
    can be located and stopped by hand.

!!! note "Aborted in-flight siblings are incomplete, not failed"
    When a run aborts, rules still in flight are killed and recorded as
    **incomplete** (neither completed nor failed) — `resume` re-runs them
    from their checkpoint, so no output is silently trusted from a killed
    job. A run stopped by SIGINT/SIGTERM/SIGHUP exits `128 + signum`
    (130/143/129) and `--json` reports status `"failed"`, even when every
    interrupted rule is `required = false` — an interrupt is never
    reported as a completed run.

!!! note "What environment a cloud job sees"
    Cloud jobs (Kubernetes, AWS Batch, GCP Batch, Tencent Batch) never inherit the
    submitting host's environment. The generated manifest/spec carries the
    rendered rule command (and, when sensitive values are routed, a
    `secretKeyRef` env block referencing the operator's Secret), so the job
    process sees the container
    image's own `ENV` (or the VM image's environment for a GCP Batch
    script runnable) plus what the engine puts in the command: the rule's
    declared `envvars` (rendered as `export K='V'`) and the
    `OXO_FLOW_SENSITIVE_<n>` references for `sensitive = true` values.
    Ambient variables from the submit shell — API tokens, cloud
    credentials, proxy settings — are never forwarded, and a regression
    test per backend pins that boundary (issue #1171). To get a host-side
    value onto the compute side, declare it in `envvars` or deliver it
    through the backend's secret channel; `OXO_FLOW_ENV_PASSTHROUGH` does
    not apply to cloud jobs because there is no ambient environment to
    re-admit. HPC job scripts (SLURM/PBS/SGE/LSF) are different: they do
    run a shell on nodes that the scheduler seeded from the submit
    environment, so they apply the `env -i` allowlist described in
    [the run reference](../commands/run.md#hpc-cluster-submissions) and in
    [workflow format](../reference/workflow-format.md#environment-specification).

!!! warning "`sensitive = true` values need an operator-side secret on cloud backends"
    Cloud runs (`run --profile <NAME>` with `[cluster] backend = "kubernetes"`,
    `"aws-batch"`, `"gcp-batch"`, or `"tencent-batch"`, and `cluster submit -b`
    with the same backends) route every `sensitive = true` config value out of the job body
    before the manifest is generated: the body carries only an
    `OXO_FLOW_SENSITIVE_<n>` reference. Names are assigned longest-value-first
    (then lexically; values under 4 characters stay on the command line), and
    the same names are used across all rules and array elements. Supply the
    value through the backend's own secret channel — a Kubernetes Secret
    named `oxoflow-secrets` (one key per variable name), the AWS Batch job
    definition's `containerProperties.secrets`, or GCP Batch
    `taskSpec.environment.secretVariables` (the engine emits those entries
    for you; point each name at its Secret Manager path with
    `OXO_FLOW_SECRET_REFS`, a `NAME=REFERENCE` map — see
    [workflow-format: `[config]`](../reference/workflow-format.md#config-configuration-variables)) —
    Tencent Batch's `Task.EnvVars` is plaintext-only (there is no submit-time
    secret-reference field), so map the names on the CVM image or job
    environment yourself — or the reference expands empty
    and the rule fails.
    HPC scheduler scripts (SLURM/PBS/SGE/LSF) route the same way: the driver
    writes a private `0o600` env file beside the run holding only the
    references each rule uses, and the generated script sources it inside the
    `env -i` wrapper, so the scheduler-visible script body carries
    `$OXO_FLOW_SENSITIVE_<n>` and never the value.

!!! tip "Check resource availability"
    Use `sinfo` (SLURM), `pbsnodes` (PBS), or `qhost` (SGE) to verify available resources before submitting.

!!! tip "Cache environment setup"
    Use `--cache-dir` to persist environment setup state across runs for faster startup.

---

## Browser-Based Cluster Monitoring

The web UI can also drive cluster execution without a terminal: register
an SSH cluster connection on the **Clusters** page (or the platform config
file's `[[clusters]]` section), then pick *Execute on cluster* in the Run
dialog. The workdir is staged to the remote host, `oxo-flow run` is
launched there detached, and logs/files/report views work exactly as for
local runs once the results are pulled back. The remote host needs the
`oxo-flow` CLI on `PATH`, `tar`, and non-interactive SSH access (key
auth, `BatchMode`).

!!! warning "Result pulls are validated before a run can finalize"
    The pulled result stream is parsed by oxo-flow's own archive reader,
    not the host `tar`. A member whose path is absolute or contains `..`,
    a symlink or hard link whose target is absolute or contains any `..`
    component (even one that would "cancel out" lexically — it can escape
    through a symlinked path segment; extraction targets are created empty
    or from regular files only, so no pre-existing symlink can be
    traversed), or a member type other than
    file/directory/link rejects the whole extraction and fails the run
    with the offending member named — nothing is written outside the
    local workdir. After extraction every member is re-checked on disk.
    The wrapper appends a run-specific completion record to the stream
    **after** the result tree, so a transfer that stops early cannot carry
    it: a truncated pull fails the run instead of reporting a partial
    tree as complete, and `execution.log` plus `.exit-code` are required
    as well. The failure is recorded in the run's `artifacts_incomplete`
    note and the run is **never** reported as `completed`. Beyond the
    transfer checks, the pulled workdir is verified against the engine's own
    records before finalization: every declared output of every completed
    rule must exist (wildcard / `output_pattern` fan-out included, through
    the same oracle the local run loop uses), and when the run recorded
    checksums (`--provenance`, cluster `--provenance`, or a rule-level
    `checksum =`) each artifact is re-hashed — a changed file is `modified`,
    a zero-byte file whose recorded digest proves content is `truncated`, and
    an absent one is `missing`. Files that merely appear beside declared
    outputs are reported as `extra` for the audit trail but do not fail the
    closure (tools legitimately write sidecars such as `.bai`), and
    `temporary = true` outputs the engine cleaned are reported as `cleaned`,
    never missing. The structured report is stored in the run's
    `artifacts_incomplete` note. A non-zero remote exit always fails the
    run. Result trees that intentionally leave symlinks pointing outside the
    workdir (for example an externally rooted `OXO_FLOW_ENV_STORE` link, or
    any link target containing `..`) are rejected — pulled results must be
    self-contained.

Cluster connection management is admin-only outside personal mode.
Details: [Cluster Connections & Remote
Execution](../reference/web-api.md#cluster-connections-remote-execution)
in the REST API reference, and [Deploy modes](deploy-modes.md#mode-3-hpc)
for the HPC serve mode.

## Monitoring Jobs

After submission, use your cluster's native tools:

```bash
# SLURM
squeue -u $USER

# PBS
qstat -u $USER

# SGE
qstat

# LSF
bjobs
```

Or use oxo-flow's status command with a checkpoint file:

```bash
oxo-flow status .oxo-flow/checkpoint.json
```

`oxo-flow cluster status` answers two questions. For a deployment that runs
through `run --profile <cluster>`, it reads the newest run directory
(`.oxo-flow/runs/latest`, or `--run-dir <DIR>`) and prints one row per rule
instance — mapping scheduler ids back through `jobs/<instance>/job.id` —
plus per-state counts:

```bash
$ oxo-flow cluster status
Cluster: run .oxo-flow/runs/latest
  completed 12   running 3    pending 47      failed 1
  instance            job      state    elapsed    exit
  align_all_9c4e12  1400_1   running  2:24:11    -
```

`--json` emits the same document for dashboards. For logs,
`oxo-flow cluster logs <id-or-instance>` reads the engine-captured
`logs/<instance>.out/.err` before the accounting store, so it works on sites
without `slurmdbd`; `--tail N` trims the output.

**With `--all` or no run directory** it falls back to the raw scheduler
view. With no ids it lists every job the scheduler currently reports for
your user — the natural check right after a plain `cluster submit`:

```bash
$ oxo-flow cluster status --all
Cluster: Executing 'squeue -u alice --noheader -o %i|%T'...
Cluster: 2 queued jobs
  12345: running
  12346: pending
```

**With ids** (the ones submit/run printed) it answers exactly those,
settling finished jobs from the scheduler's accounting store:

```bash
oxo-flow cluster status 12345 12346
oxo-flow cluster cancel 12345
```

Every cluster action resolves its backend the same way: the `--backend` /
`-b` flag if given, then `$OXO_FLOW_CLUSTER_BACKEND`, then `slurm`. On a
SLURM site you never type `-b`; on a PBS site set the environment variable
once (`export OXO_FLOW_CLUSTER_BACKEND=pbs`). An explicit-but-typoed value
is still rejected outright (`unknown cluster backend '…' — expected slurm,
pbs, sge, lsf, kubernetes (k8s), aws-batch, gcp-batch, or tencent-batch
(Tencent Cloud BatchCompute)`) — the default never swallows a bad name.

### Resource ledger and scheduler limits

`oxo-flow report --run <DIR> --resource-ledger` turns the stored accounting
records into the normalized `oxo-flow.resource-ledger/v1` document (JSON or
CSV — see
[`report`](../commands/report.md#resource-ledger-resource-ledger)). The
cluster path persists the scheduler's measurements in the checkpoint's
`resource_usage` map together with the program that answered, so every
number in the ledger can be traced back to a raw field.

What each scheduler can supply:

| Scheduler | Accounting source | Reported |
|---|---|---|
| SLURM | `sacct` | exit code, `ElapsedRaw`/`Elapsed`, `MaxRSS`, `TotalCPU`, `Submit`→`Start` wait, `AllocCPUS`, `NNodes`, `AveDiskRead`/`AveDiskWrite` |
| PBS | `qstat -x` | exit code, `resources_used.walltime` / `.mem` / `.cput` |
| SGE | `qacct` | exit code, `ru_wallclock`, `ru_maxrss`, `cpu` |
| LSF | `bacct` | terminal state only |
| SLURM without `slurmdbd` | `scontrol` | `JobState` + `ExitCode` (controller cache only) |
| Kubernetes / AWS Batch / GCP Batch / Tencent Batch | — | nothing; the ledger carries a source note, not a fabricated zero |

Limits worth knowing before comparing numbers:

- **`sacct` omissions are honored.** A site whose accounting store leaves a
  column blank (or a store that predates the extended `--format` request)
  gets `null`/an empty CSV cell for that metric, never `0`. The same applies
  to every cloud backend: no accounting API is called, so every measurement
  is unknown rather than assumed.
- **Memory is a peak.** `MaxRSS` is the maximum over the job's step rows
  (the allocation and `.batch` rows describe the same work, so summing them
  would double-count). The ledger's run-level memory is the maximum across
  rules.
- **Array elements on PBS/LSF/SGE are aggregate verdicts.** This adapter
  polls those backends by the array base id; the record names its provenance
  in `skip_reason` and withdraws the aggregate's exit code, memory, and CPU.
  The ledger reports those elements' measurements as unknown instead of
  attributing the aggregate to one element.
- **Invocation boundary.** Every cluster run marks previously recorded
  rules as not executed before submitting anything; a rule that settles
  clears its mark, while a no-op re-run (nothing to submit), a subset
  submission, or a rule blocked behind a failure stays marked. The ledger
  therefore reports a cluster no-op as `rules_executed: 0` rather than
  recycling the earlier run's benchmarks.
- **Reconcile with an independent export.** `--resource-ledger --acct
  <sacct.csv>` (JSON only — CSV has no reconciliation columns, so
  `-f csv` + `--acct` fails closed as a usage error) reduces the CSV to the rules that executed in this run (a
  whole-account `sacct -a` export's unrelated jobs are ignored and counted
  in the source note), sums the remaining batch rows into
  `run.scheduler_totals`, and names that raw side's own provenance in
  `run.scheduler_source`. Disagreements beyond 5 % (1 s / 1 MiB floor) are
  listed under `run.conflicts`, naming both numbers and both sources;
  `run.reconciliation` records which metrics were compared and which were
  skipped (and why). A CSV that does not cover every executed rule is
  treated as a coverage gap, not a conflict.

---

## Driver Defaults and the `[cluster]` Profile Block

`run --profile` submits and tracks jobs through the backend driver, whose
built-in defaults (`max_submitted` 50, `max_array_size` 1001,
`poll_interval` 5s) are specified once in the
[`DriverConfig` table](../reference/execution-backends.md#backenddriver);
the profile's `[cluster]` block overrides those three knobs. The full
annotated TOML example (every key, the driver default each omission falls
back to, and where a rule's own `time_limit` wins) lives once in
[Cluster submission](../commands/run.md#cluster-submission) — that copy is
authoritative.

`extra_args` entries are emitted as scheduler directives **verbatim and
unvalidated** — a typo reaches the scheduler as written (the same applies to
`cluster submit --extra-arg`). Cluster CLI actions resolve `--backend` as
flag > `$OXO_FLOW_CLUSTER_BACKEND` > `slurm` (see
[Monitoring Jobs](#monitoring-jobs)); the profile block's `backend` key
remains required and is validated the same strict way.

### Cluster flag surface

`run --profile` does not consume every `run` flag. The flags below are
parsed but have no effect on the cluster path — each of the first three
prints a warning and the run proceeds (`-j` is dropped silently):

| `run` flag | Cluster-path behavior |
|---|---|
| `--retry` | Warned and dropped — there is no engine-side retry on the cluster path. Failed rules are recorded in the checkpoint and re-queued by `resume` (checkpoint path argument) or `run --resume-failed`. |
| `--timeout` | Warned and dropped — per-job walltime comes from the profile's `[cluster]` `walltime` key or the rule's own `time_limit`; the `--walltime` flag exists only on `cluster submit`. |
| `--max-threads` / `--max-memory` | Warned and dropped — per-rule `[resources]` and the `[cluster]` profile drive scheduler limits; `[resource_budget]` performs the pre-submit check. |
| `-j` / `--jobs` | Silently inapplicable — cluster concurrency is governed by `max_submitted` (the profile key or `--max-submitted`), clamped by `[resource_budget] max_jobs`. |
| `--skip-env-setup`, `--ai-recover` | Accepted for command-line parity (warned, issue #136): environments are prepared inside each job script, and AI recovery does not run on the cluster path. |

`--skip-ref-build` is accepted on the cluster path and is a no-op there by
construction: the reference-build step runs only on the local execution
path, and `run --profile` never builds references itself.

**Lifecycle hooks do not run on the cluster path.** Rule-level
`pre_exec` / `on_success` / `on_failure` and workflow-level
`on_complete` / `on_error` are run by the local executor and the CLI run
loop, neither of which the cluster path enters. A workflow that declares
any of them is **rejected at plan time** — before any job script is
rendered — naming the offending rule and hook field (issue #1340), on
both `run --profile <name>` and `cluster submit`, instead of silently
dropping the hook on the scheduler. Remove the hooks or run the workflow
locally (`oxo-flow run`).

`--provenance` **is** honored on the cluster path (issue #1093): each rule's
expanded outputs are hashed when the job settles and recorded into the
checkpoint's `checksums` map, exactly like a local run — `provenance verify`
and the freshness gate read the same entries from either execution path.
The per-rule `checksum = "sha256" | "md5"` declaration works there too. A
run that records nothing still warns, with the same wording as the local
completion path.

`--keep-going` / `-k` **is** honored on the cluster path (issue #1093).
It gates exactly one decision — what a `required = true` rule's failure
does to in-flight work:

| Mode | Required rule fails | `required = false` rule fails |
|---|---|---|
| default | cancels every in-flight job, then exits non-zero | never cancels; failure recorded, dependents blocked, run exits 0 |
| `-k` / `--keep-going` | in-flight jobs finish, ready independent instances keep dispatching until the queue drains, the failed rule's dependents stay blocked — then exits non-zero (a required rule failed) | same as default |

Interrupts (Ctrl-C / SIGTERM / SIGHUP) and driver-internal errors always
cancel every in-flight job regardless of `-k`; the flag never softens the
verdict — a run with a required failure exits non-zero either way.

---

## Scheduling Fairness

Cluster submission honors the same fairness contract as local execution:

- **Priority is honored**: each round, ready rules are submitted in
  effective-priority order — declared `priority` plus priority aging
  (every round a ready rule spends beyond the `max_submitted` cap gains
  +1). A producer parked at the cap therefore outranks fresh
  high-priority rules after `priority gap` rounds — priority-inversion
  starvation cannot persist.
- **In-flight jobs are cancelled on a required failure by default**: when
  a required rule fails, the driver cancels every job already submitted
  (`scancel`/`qdel`/`bkill`) before the run exits — no orphaned cluster
  jobs. With `-k` / `--keep-going` those jobs instead finish, ready
  independent instances keep dispatching until the queue drains, the
  failed rule's dependents stay blocked, and the run still exits
  non-zero (see [Cluster flag surface](#cluster-flag-surface)).
- **Waits inside the scheduler queue** are the scheduler's own
  ordering and are visible via `squeue`/`qstat`/`bjobs` (job ids are
  recorded per rule under the run directory). The local 60-second
  "waiting for resources:" diagnostics have no cluster counterpart by
  design — the pool lives on the scheduler, not in the engine.

### Per-sample dispatch order

Equal-priority ties are broken by instance name by default. Expanded
instance names are `<rule>_<group>_<sample>`, so that tie-break groups
every instance of one rule together: a per-sample pipeline submits all
`align` instances, then all `qc` instances, then all `merge` instances.
The first `merge` cannot run until the slowest sample's producers finish,
and no per-sample result exists until the end of the run.

Set `[engine] dispatch_order = "sample"` to order ties by the sample
binding first, then by rule declaration order:

```toml
[engine]
dispatch_order = "sample"

[[sample_groups]]
name = "cohort"
samples = ["S1", "S2"]

[[rules]]
name = "align"
output = ["out/{sample}/align.bam"]
shell = "align {sample} > out/{sample}/align.bam"

[[rules]]
name = "qc"
output = ["out/{sample}/qc.txt"]
shell = "qc {sample} > out/{sample}/qc.txt"

[[rules]]
name = "merge"
input = ["out/{sample}/align.bam", "out/{sample}/qc.txt"]
output = ["out/{sample}/merge.txt"]
shell = "merge out/{sample}/align.bam out/{sample}/qc.txt > out/{sample}/merge.txt"
```

Both producers read the same sample independently, and `merge` joins
them. Under the default `"rule"` mode the ready list is
`align_S1, align_S2, qc_S1, qc_S2, …`; under `"sample"` it is
`align_S1, qc_S1, align_S2, qc_S2, …`. Priority aging remains the
primary key: a sample-2 producer that has waited a round accrues +1
and can run before sample 1's newly-ready `merge_S1`. Sample mode
therefore keeps each sample's stages adjacent among equal-priority
ties and reduces phase-wise batching, but per-sample drain is
best-effort, not guaranteed. Total work and the `max_submitted` cap
are unchanged.

The setting is only a tie-break over *currently ready* rules: it never
reorders dependencies, and it does not delay sample 2's independent
producers behind sample 1's unfinished stages. `priority` remains the
primary key — a rule whose declared (`+ aging`) priority is higher still
jumps the queue, which is useful for a genuinely urgent one-off rule.
Use `priority` for per-rule urgency and `dispatch_order = "sample"` for
per-sample pipeline flow.

The same ordering is honored by local execution and by the cluster
driver. The default `"rule"` mode is byte-identical to previous
releases.

---

## See Also

- [Architecture: Cluster backends](../reference/architecture.md) — internal cluster module design
- [Environment System](../reference/environment-system.md) — Singularity and Docker on HPC
- [`run` command](../commands/run.md) — `--max-threads`, `--max-memory`, `--skip-env-setup`, `--cache-dir`
- [`cluster` command](../commands/cluster.md) — cluster submission reference
