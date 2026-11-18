import { test, expect, type APIRequestContext } from '@playwright/test';

// Issue #1149 (audit UX-07): the monitor's DAG-status canvas hard-coded
// `environment: 'system'` for every node, so a run whose rules use conda or
// docker was drawn as an all-system pipeline. The fix carries the real
// per-rule environment on `GET /api/runs/{id}/dag-status` (resolved from the
// run's own snapshot the way the run path resolves it: [defaults]
// environment folded onto rules, then the rule's inline environment) and the
// canvas renders it.
//
// The run below is REAL (created through POST /api/runs, dry_run so no rule
// executes and no backend is bootstrapped) and has three different
// environments; the assertions read the label the canvas actually paints.

const WORKFLOW = `[workflow]
name = "monitor-env-fixture"
version = "1.0.0"

[[rules]]
name = "plain_qc"
output = ["qc/plain.txt"]
shell = "true > {output}"

[[rules]]
name = "conda_align"
input = ["qc/plain.txt"]
output = ["aligned.bam"]
environment = { conda = "envs/align.yaml" }
shell = "align {input} > {output}"

[[rules]]
name = "docker_call"
input = ["aligned.bam"]
output = ["variants.vcf"]
environment = { docker = "biocontainers/bcftools:1.20" }
shell = "call {input} > {output}"
`;

/** Create a real (dry-run) run and return its id. */
async function createRun(request: APIRequestContext): Promise<string> {
  const resp = await request.post('/api/runs', {
    data: { toml_content: WORKFLOW, dry_run: true },
  });
  expect(resp.ok()).toBeTruthy();
  const { run_id } = await resp.json();
  expect(typeof run_id).toBe('string');
  return run_id as string;
}

test.describe('Monitor DAG canvas — real per-node environment (issue #1149)', () => {
  test('every node shows the environment resolved from the run snapshot', async ({ page, request }) => {
    const runId = await createRun(request);

    // Guard the UI assertion with the wire data: the canvas test is only
    // meaningful if the API really carries three distinct environments.
    const dagResp = await request.get(`/api/runs/${runId}/dag-status`);
    expect(dagResp.ok()).toBeTruthy();
    const dag = await dagResp.json();
    const envOf = (label: string) =>
      (dag.nodes as Array<{ label: string; environment: string | null }>).find(
        (n) => n.label === label,
      )?.environment;
    expect(envOf('plain_qc')).toBe('system');
    expect(envOf('conda_align')).toBe('conda');
    expect(envOf('docker_call')).toBe('docker');

    await page.goto(`/runs/${runId}`);
    await page.getByRole('tab', { name: 'DAG' }).click();

    const nodeLabel = (rule: string) =>
      page.locator('.rf-rule-node').filter({ hasText: rule }).locator('.rf-env-label');

    await expect(nodeLabel('plain_qc')).toHaveText('system');
    await expect(nodeLabel('conda_align')).toHaveText('conda');
    await expect(nodeLabel('docker_call')).toHaveText('docker');

    // The audit finding: `system` appears exactly once — on the rule that
    // genuinely runs there — not on every node.
    await expect(page.locator('.rf-env-label')).toHaveCount(3);
    await expect(page.locator('.rf-env-label').filter({ hasText: 'system' })).toHaveCount(1);
  });
});
