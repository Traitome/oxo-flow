# Cloud live campaigns — remaining work / needs help (2026-11-26)

Status of the cloud batch backends after the 2026-11-26 qualification wave.
This page is the durable record for the live work that **cannot be performed
from the qualification host**; issue #1390 is closed as not planned for the
live campaign so it does not stay open indefinitely.

## Done (code-side and Tencent)

- AWS Batch / GCP Batch / Tencent Batch CLI invocations and JSON fields were
  compared against the vendors' own service models, CLI references, and
  protos; the audit and its contract/wire tests are in
  [`cloud-cli-accuracy-2026-11-26.md`](cloud-cli-accuracy-2026-11-26.md).
- Tencent Batch has a live campaign: named-env submit `SUCCEED`, state poll,
  `DescribeTaskLogs` decode, `TerminateJob` cancel; the shared cloud workdir
  bug was found and fixed (PR #1421). See
  [`tencent-batch-live-2026-11-26.md`](tencent-batch-live-2026-11-26.md).
- The AWS `logs()` arm returns describe-jobs metadata, not CloudWatch events;
  this is a documented residual (wiring `container.logStreamName` +
  `aws logs get-log-events` is part of the live campaign).

## Not done — needs help

**AWS Batch live campaign.** Needs a global-region AWS account (AWS China is
a separate account realm), a minimal spot compute environment, job queue,
and job definition, plus network access. The protocol: submit (single and
array), poll to terminal, per-element settlement, log retrieval, SIGINT
cancel, artifact pull + closure verification, provenance round-trip; record
every engine-assumption mismatch as a fix or a documented residual with raw
logs.

**GCP Batch live campaign.** Needs a GCP project/credentials and a host that
can reach the GCP endpoint (the current qualification host is in mainland
China and cannot). Same protocol as AWS; cancel uses `CancelJob`, not
`DeleteJob`.

Both remain **Prototype** with mock/wire tests only; no live behaviour is
claimed. The per-backend submit/queue/run/cancel/fail/retry/artifact
campaign is tracked as CL-05 / #1158 in the maturity rows.

## When credentials and a reachable host exist

1. Reopen #1390 (or file a fresh campaign issue) with the account/project
   and host details; never commit credentials.
2. Run the protocol above against a cost-capped environment; capture raw
   logs and exact CLI versions.
3. Fix every mismatch the campaign proves (with a regression test), or list
   it explicitly as a residual; update the maturity row and this page.
4. Do not move the maturity label past Prototype until the full protocol
   exists for that backend.

## References

- [`cloud-cli-accuracy-2026-11-26.md`](cloud-cli-accuracy-2026-11-26.md)
- [`tencent-batch-live-2026-11-26.md`](tencent-batch-live-2026-11-26.md)
- `docs/guide/src/how-to/run-on-cluster.md` (backend maturity table)
- `ROADMAP.md` (Stage 12.1 cloud executors row)
