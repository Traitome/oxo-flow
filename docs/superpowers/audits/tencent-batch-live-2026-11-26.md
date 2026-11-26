# Tencent Batch live qualification — 2026-11-26

Live campaign for the `tencent-batch` backend (issue #1389) on the designated
`tx-ubuntu` host, using a real Tencent Cloud account and the release binary.
Credentials were passed only through `TENCENTCLOUD_SECRET_ID` /
`TENCENTCLOUD_SECRET_KEY` in the process environment (tccli reads them when no
credential file exists); no credential, account id, or secret was written to
the repository or to this document.

## Environment

| Field | Value |
| --- | --- |
| Host | `VM-4-4-ubuntu`, Linux 6.8.0-137-generic x86_64 |
| CLI | `tccli 3.1.208.1` (Batch API `2017-03-12`) |
| Region / zone | `ap-shanghai` / `ap-shanghai-5` |
| Instance type | `S5.MEDIUM4` |
| Image | Ubuntu Server 24.04 LTS (`img-mmytdhbn`) |
| Baseline binary | `0.32.0`, main `40ccf1eb` (release) |
| Fixed binary | PR #1421 head `0c04a107` (release) |

The named compute environment used for the campaign
(`oxoflow-live-20261126`, `env-m89xpntg`, default VPC `vpc-loijqvuo` /
subnet `subnet-5zu5thq3`) was created for the test and deleted afterwards.
An inline job-level compute environment was also submitted; it provisioned
successfully on this account (STARTING → RUNNING in ~2 min), unlike the
2026-11-25 account where anonymous creation failed.

## Baseline findings (main `40ccf1eb`)

| Step | Result |
| --- | --- |
| `cluster submit -b tencent-batch --dry-run` | payload generated with `Placement.Zone`, `ComputeEnv.EnvData.{InstanceType,ImageId}`, `Application.Command` |
| Inline-env submit (default workdir `/home/ubuntu/tencent-live`) | `job-9h5zmzyk` STARTING → RUNNING → **FAILED**, exit 2, task reason `ProcessExecutionError.UserCommandError:cd '/home/ubuntu/tencent-live' && echo tencent-live-ok`; stderr `sh: 1: cd: can't cd to /home/ubuntu/tencent-live` |
| Named-env submit with a node-visible workdir (`/tmp`) | `job-gl47mgyq` **SUCCEED**; the engine run reported 1 succeeded, 0 failed |
| Logs | `DescribeTaskLogs` returned base64 `data:` stdout decoding to `tencent-live-ok` |
| Cancel | a `sleep 300` job (`job-0byigi96`) RUNNING → `TerminateJob` → **FAILED**; the engine run settled failed |
| `cluster logs <id> -b tencent-batch` | failed: the command had no way to pass the zone/region, so tccli could not resolve the job |

**Root cause of the failure:** the shared plan builder folds
`cd '<submit-host-workdir>' &&` into every rule command. A cloud compute node
does not share the submit host's filesystem, so the job died before its shell
ran. The single-rule run only succeeded when the workdir was `/tmp` (which
also exists on the node).

## Fix (PR #1421)

- Cloud backends strip the folded `cd` prefix when no shared volume is
  declared; the rule runs in the vendor agent's default directory.
- When `[cluster.volume]` is declared, the prefix and workdir-anchored paths
  are rewritten onto `mount_root` (the path the operator mounted the shared
  filesystem at on the compute nodes).
- Cloud **array** jobs now fail closed without `[cluster.volume]`: their
  per-index `cmd.N.sh` files are never uploaded to the vendor.
- `cluster logs --queue` supplies the GCP location / Tencent zone; AWS reads
  its region from the AWS config.

## Fixed-binary verification (PR #1421, `0c04a107`)

Release build on `tx-ubuntu` (12m32s, `CARGO_BUILD_JOBS=1`). Each step used
the same named environment:

| Step | Result |
| --- | --- |
| Single-rule run, default workdir `~/tl-fixed` (a path absent on the node) | **SUCCEED** — `1 succeeded, 0 failed`; job `job-889heale`; the submitted command no longer carries `cd '~/tl-fixed'` |
| `cluster logs job-889heale -b tencent-batch --queue ap-shanghai-5` | exit 0, printed `tencent-live-ok` (the `--queue` zone reaches tccli and `DescribeTaskLogs` decodes the data-URI stdout) |
| Array run without `[cluster.volume]` (`max_submitted = 10`, two samples) | exit 1 with `cloud array jobs require [cluster.volume] declaring the shared filesystem …`; **no job submitted** (job count 13 before, 13 after) |

Cleanup: all jobs were terminal, the named compute environment was deleted,
and the credential file was shredded. The array test with `max_submitted = 1`
submitted the two instances as individual jobs instead of an array — which is
why the fail-closed path is exercised with a grouping threshold above one.

## Limits

- One account, one zone, one image, one host; no cross-region or
  multi-account claim.
- Arrays require a shared filesystem mounted on the compute nodes; no live
  shared-NFS array was run in this campaign.
- No artifact pull: cloud rules produce outputs on the node unless the
  workdir is a shared mount.
- AWS Batch / GCP Batch remain unqualified live (no AWS credentials; the GCP
  endpoint is unreachable from the qualification host); see
  [`cloud-cli-accuracy-2026-11-26.md`](cloud-cli-accuracy-2026-11-26.md) for
  their static verification.
