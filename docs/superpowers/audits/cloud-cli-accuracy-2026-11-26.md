# Cloud CLI accuracy audit — AWS Batch, GCP Batch, Tencent Batch (2026-11-26)

> **Update (2026-11-26):** a live Tencent campaign was run after this static
> audit — see [`tencent-batch-live-2026-11-26.md`](tencent-batch-live-2026-11-26.md).
> AWS Batch and GCP Batch remain static-only (no credentials / unreachable
> endpoint).

Code-side evidence for **issue #1390** (cloud backends are CLI-wrapper /
mock-tested only). Every vendor CLI invocation and JSON field the three
cloud backends generate or read was compared against the vendors' own
service models and CLI sources on 2026-11-25/26. **No live cloud calls were
made** (no AWS credentials; the GCP endpoint was unreachable from the
qualification host), so this document records static verification plus
contract/wire tests only — it does not claim live behaviour for polling,
logs, cancellation, or arrays on any cloud.

## Method and sources

Vendor sources were fetched into `/tmp` and read locally; no vendor blobs
and no credentials are committed. Sources used:

- AWS: `botocore/data/batch/2016-08-10/service-2.json`
  (`SubmitJobRequest`, `ContainerOverrides`, `ResourceRequirement`,
  `ArrayProperties`, `JobStatus`, `DescribeJobs` ≤100 ids, `ListJobs`,
  `TerminateJob`); AWS CLI reference `batch list-jobs` ("You must specify
  only one of …"; no `--job-status` returns only RUNNING),
  `batch terminate-job` (STARTING/RUNNING → FAILED), the array-jobs guide
  (`parentJobId:arrayIndex`, zero-based, children individually terminable),
  and the CLI parameter-file guide (`file://`).
  `https://jmespath.org/specification.html` for the raw-string grammar.
- GCP: `googleapis/google/cloud/batch/v1/{job,task,batch}.proto`
  (`JobStatus.State` incl CANCELLED, `TaskStatus.State` incl UNEXECUTED,
  `taskCount`/`parallelism`, `logsPolicy` CLOUD_LOGGING,
  `Environment.secretVariables`, `BATCH_TASK_INDEX`, `CancelJob` RPC,
  job-id rules ≤63 lowercase); gcloud reference `batch jobs submit`
  (`<id>` positional, `--config` is a plain local path, no `--query`),
  `batch tasks list` (`--job` required), `batch jobs cancel`, and
  `batch jobs delete`.
- Tencent: `TencentCloud/tencentcloud-cli`
  `tccli/services/batch/v20170312/api.json` and `examples.json`
  (`Placement.Zone` required; JobName ≤60; `TaskInstanceNum`;
  `ComputeEnv` XOR `EnvId`; JobState enum; `DescribeJobs` `JobIds`/`Limit`
  ≤100; raw-Base64 task logs; `TerminateJob`), `tccli/action_caller.py`
  (tccli prints the unwrapped response **body**, not the HTTP
  `Response` envelope), `tccli/cli_input_json.py` (`file://` required),
  `tccli/argument.py` (JSON list arguments), and the Batch
  environment-variable page (`BATCH_TASK_INSTANCE_INDEX`).

## Audit table — AWS Batch

| Surface | Engine value | Vendor truth | Verdict | Fix / test |
|---|---|---|---|---|
| submit argv | `submit-job --cli-input-json file://<path> --query jobId --output text` | `file://` parameter files; response carries `jobId` | OK | `aws_batch_submit_returns_the_bare_job_id` |
| SubmitJob body | `jobName`, `jobQueue`, `jobDefinition`, `containerOverrides.command`, `resourceRequirements`, `arrayProperties.size` | all service-model fields; VCPU/MEMORY/GPU; array size 2–10,000 | OK (arrays only when chunk > 1) | spec tests |
| jobName length | 128 budget + `oxoflow-`/`oxoflow-array-` prefix | jobName ≤128 | **MISMATCH** (up to 136/142) | prefix counted; `generated_job_names_fit_the_128_char_cap` |
| array dispatch | `AWS_BATCH_JOB_ARRAY_INDEX + 1` → `cmd.N.sh` | env var zero-based | OK | spec test |
| array child ids | `{parent}:{ordinal-1}` | `parentJobId:arrayIndex`, zero-based | OK | `array_child_ids_are_zero_based_real_job_ids` |
| poll/list `--query` | `join(\|,...)` (unquoted separator) | JMESPath raw strings are single-quoted; a bare `\|` is the pipe operator | **P1 MISMATCH** — rejected before any API call | `join('\|',...)`; argv tests; reproduced with the npm `jmespath` parser (`ParserError: Invalid token (Pipe)`) |
| poll argv | `describe-jobs --jobs <ids…>`, ≤100 per call | DescribeJobs caps at 100 ids | OK | chunk tests |
| status enum | SUBMITTED/PENDING/RUNNABLE/STARTING/RUNNING/SUCCEEDED/FAILED | service-model enum | OK | parser tests |
| cancel | `terminate-job --job-id --reason` | valid in every state; running → FAILED | OK | cancel grammar test |
| raw listing | `list-jobs --job-queue <queue>` | exactly one selector; default status = RUNNING only | **MISMATCH** — queued jobs omitted; empty table when no queue although submit defaults to `oxoflow-queue` | per-live-status calls merged; shared `AWS_DEFAULT_JOB_QUEUE`; `aws_live_listing_asks_every_non_terminal_status` |
| queue/definition defaults | `oxoflow-queue` / `oxoflow-job-def` | both required in SubmitJob | engine convention; shared with the listing | `default_queue_and_job_definition_are_shared_with_the_lister` |
| logs | `describe-jobs` JSON | no log API | residual: metadata only | documented |

## Audit table — GCP Batch

| Surface | Engine value | Vendor truth | Verdict | Fix / test |
|---|---|---|---|---|
| submit argv | `batch jobs submit <minted-id> [--location] --config <plain path>` | `<id>` positional; `--config` plain local path (no URI scheme); no `--query` | OK after fix | extended `gcp_batch_submit_returns_the_minted_job_id` |
| job id | `oxoflow-<sanitized>-<millis>`, ≤63, lowercase | job_id ≤63, lowercase, `-` not first/last | OK | `minted_gcp_job_ids_respect_the_63_char_cap` |
| array chunk id source | immediate parent only → every rule's chunk was `chunk-N` | job ids are project-unique | **MISMATCH** (collision risk; unrecognizable) | template + chunk; `gcp_batch_array_chunk_ids_name_the_template` |
| job body | runnables script/container; `computeResource`; `maxRetryCount`; `taskCount`/`parallelism`; `logsPolicy`; `secretVariables` | all proto fields | OK | spec tests |
| array dispatch | `BATCH_TASK_INDEX + 1` → `cmd.N.sh` | env var zero-based | OK | spec test |
| element poll | `tasks list --job=<base> --location --format=value(name,status.state)` | `--job` required; TaskStatus.State incl UNEXECUTED | **MISMATCH** — UNEXECUTED mapped Unknown (blind settle) | UNEXECUTED → Failed; test |
| job state poll | describe `status.state` | enum includes CANCELLED | **MISMATCH** — CANCELLED rewritten Failed | CANCELLED → Cancelled; full-enum test |
| cancel | `batch jobs delete <id> --quiet` | BatchService has **CancelJob**; gcloud exposes `batch jobs cancel`; delete removes the record | **MISMATCH** — record lost, later describe 404 → blind settlement | `batch jobs cancel` + location; cancel grammar test |
| CLI location | describe/list/cancel had no `--location` source | bare id needs `--location` or a gcloud default | **MISMATCH** | `--queue` threaded through status/listing/cancel; tests |
| submit hint/wrapper | `--config file://<path>`, `--query 'name'`, script path in the JOB slot, no `--config` | plain path; no `--query`; id is positional | **MISMATCH** — hard failure + corrupted ids | minted id + `--config "$1"` + location; `cloud_submit_helpers_match_the_vendor_cli_grammar` |
| per-id element status | `{job}:{task}` sent to `jobs describe` | a job id cannot contain `:` | **MISMATCH** | engine handles filtered out of `status_invocations`; test |
| logs | `jobs describe` JSON | no log API (logsPolicy → Cloud Logging) | residual: metadata only | documented |

## Audit table — Tencent Batch (API 2017-03-12, tccli)

| Surface | Engine value | Vendor truth | Verdict | Fix / test |
|---|---|---|---|---|
| submit argv | `SubmitJob --cli-input-json file://<path> --output json [--region]` | tccli requires `file://` for `--cli-input-json` | OK | hint test |
| JobId parse | `Response.JobId` | tccli prints the **unwrapped** body (`{"JobId": …}`) | **P1 MISMATCH** — real submit id never parsed; run aborted at submit | `tccli_body` accepts body + envelope; envelope test |
| payload | Placement.Zone; JobName; Tasks[].TaskName/TaskInstanceNum/Application/ComputeEnv or EnvId; MaxRetryCount | all api.json fields; Zone required; ComputeEnv XOR EnvId | OK | spec tests |
| JobName length | 60 budget + `oxoflow-` prefix | ≤60 | **MISMATCH** (up to 68) | prefix counted; `generated_job_name_fits_the_60_char_limit` |
| array | `TaskInstanceNum = count`; `BATCH_TASK_INSTANCE_INDEX` | documented array knob; env var zero-based | OK | spec test |
| poll | `DescribeJobs --JobIds <json> --Limit 100 --output json`; read `Response.JobSet` | unwrapped `JobSet`; Limit ≤100 | **P1 MISMATCH** — always empty → blind FAILED; no >100 chunking | unwrapped parse + chunking + region; poll/status tests |
| job states | SUBMITTED/PENDING/RUNNABLE/STARTING/RUNNING/SUCCEED/FAILED/FAILED_INTERRUPTED | api.json enum exactly | OK | full-enum test |
| `--region` | derived in `region_from_zone`, never passed | tccli region must match the zone | **MISMATCH** | `tccli_region_args` on submit/poll/logs/cancel/status; tests |
| cancel | `TerminateJob --JobId` | documented (SUBMITTED forbidden; SUCCEED no-op; async) | OK (+region) | cancel grammar test |
| logs | DescribeJob → DescribeTaskLogs; data-URI decode only | unwrapped replies; raw Base64 per api.json, data URI in its example | **MISMATCH** | `tccli_body`; decode both; log tests |
| `cluster status <ids>` | empty invocations → every id unknown | DescribeJobs answers by id | **MISMATCH** | JSON status path (chunked ≤100); tests |
| raw `cluster status` | empty → "0 queued jobs" | DescribeJobs has no per-user/queue scope | **MISMATCH** (misleading) | explicit "unsupported" + `supports_my_jobs_listing` |
| per-id element status | element handles sent as `--JobIds` | `JobIds` takes real ids; an invalid one fails the call | **MISMATCH** | bases deduped; element handle → base verdict; test |
| array settlement | every element rides base `JobState` | `DescribeTask` exposes per-instance state | doc overstated per-instance settlement | doc corrected to aggregate provenance (`skip_reason`); residual below |

## Fixes and tests

The code changes are in PR #1406: `tccli_body`/`parse_tencent_job_states`
shared by submit/poll/status/logs; GCP `CancelJob`; zone-derived
`--region`; AWS `join('|',...)` and multi-status listing; name budgets
(prefix counted); GCP `--location` threading and chunk-id source; raw and
data-URI log decoding; element-handle filtering; one backend-name hint
constant (`gcp-batch` was missing from every error message). GCP
`JobStatus.State` is one `gcp_job_status` table shared by the poller and
the `cluster status` line parser, and the AWS row queries are two
constants (`AWS_DESCRIBE_JOBS_QUERY`, `AWS_LIST_JOBS_QUERY`) used by all
three call sites.

Deterministic tests (vendor-shaped JSON/text fixtures or shell shims, no
network): Tencent envelope/state/chunking/status/logs/poll and
submit/log-call `--region` argv; GCP submit argv, chunk id, the full
`status.state` table through both the poller and `status_report`/CLI
view, UNEXECUTED, and raw-listing `--location`; AWS caps, defaults (the
listing must reuse the submit queue), live listing, and the exact
JMESPath argv at all three call sites; cancel grammar; submit hints; CLI
submit-helper grammar and multi-invocation merge.

## Review follow-up (2026-11-26)

Independent review of PR #1406 (0 Critical/0 Important, 5 Minor) raised
test/claim gaps; all five are closed in the follow-up commit:

1. GCP job-state mapping is now one `gcp_job_status` table used by the
   poller AND the CLI status parser (`SCHEDULED` → pending,
   `CANCELLATION_IN_PROGRESS` → running, `CANCELLED` → cancelled), pinned
   by `gcp_cli_status_view_matches_the_poller_state_table`.
2. AWS `join('|',...)` is one constant per query, used at status, poll and
   listing; the poll and listing tests assert the exact `--query` argv, and
   `aws_row_queries_use_a_quoted_jmespath_separator` pins the constants'
   literal text so a broken separator cannot hide behind the constant.
3. GCP raw listing `--location` is pinned by
   `gcp_raw_listing_passes_the_location_from_the_queue`.
4. Tencent `--region` is pinned on the executor submit and both log calls
   by `tencent_submit_and_logs_carry_the_zone_region` (poll/status/cancel
   were already covered).
5. The AWS defaults test now also asserts the raw listing queries
   `AWS_DEFAULT_JOB_QUEUE`, so it fails if the listing stops sharing the
   submit default.

## Residual / blocked

- **Live AWS/GCP campaigns (#1390)**: not run — no AWS credentials and no
  reachable GCP endpoint. Issue #1390 is closed as not planned for the live
  campaign; the prerequisites, protocol, and remaining work are recorded in
  [`cloud-live-campaigns-todo-2026-11-26.md`](cloud-live-campaigns-todo-2026-11-26.md).
  No live claim is made for polling, logs, cancellation, or arrays on those
  backends; their maturity labels stay **Prototype**.
- **#1389** is closed by the 2026-11-26 live campaign
  ([`tencent-batch-live-2026-11-26.md`](tencent-batch-live-2026-11-26.md)):
  named-env submit `SUCCEED`, state poll, `DescribeTaskLogs` decode, and
  `TerminateJob` cancel; arrays require `[cluster.volume]` and fail closed
  without it.
- Tencent per-instance settlement via `DescribeTask` remains the RB-06
  residual: arrays settle on the base job verdict and record that
  aggregate provenance in `skip_reason`. `DescribeTaskLogs` is read as one
  page (default 5, max 10 instances).
- AWS/GCP `logs()` return job metadata (`DescribeJobs` / `jobs describe`);
  neither backend has an engine-wired log API.
- AWS raw listing is queue-wide (ListJobs has no per-user filter).
- **Tencent live-evidence reconciliation:** PR #1394's body additionally
  described live poll/log/cancel/run activity, while this audit records
  those paths as mock-verified because no committed artifact reproduces
  them. The conservative wording is intentional; if that evidence is real
  it should be recorded here so the repository does not tell two stories.

No credentials, secrets, or vendor source files are included in this
document or the repository.
