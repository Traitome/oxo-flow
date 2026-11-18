import { test, expect, type Page } from '@playwright/test';

// Issue #1146 (audit UX-04): the failure panel used to offer "Apply & Retry"
// while the handler called `retryRun` directly — the rerun repeated the same
// failure after the UI implied it had applied a repair. The diagnostics
// `fix_action` is advisory only (placeholder values, no server endpoint
// applies it), so the honest control is "Retry unchanged": it re-runs the
// same workflow and never claims a mutation. These tests pin that contract.

const FAILED_RUN = 'run1146aaaaaaaaaaaaaaaa';
const RETRY_RUN = 'run1146bbbbbbbbbbbbbbbb';

function runItem(id: string, status = 'failed') {
  return {
    id,
    user_id: 'default',
    pipeline_id: null,
    workflow_name: 'demo-pipeline',
    dry_run: false,
    status,
    phase: 'executing',
    workdir: `/tmp/oxo-flow-e2e/runs/${id}`,
    started_at: null,
    finished_at: null,
    created_at: new Date().toISOString(),
  };
}

const MONITOR_STATUS = {
  overall: 'failed',
  alerts: [],
  resource_forecast: {
    cpu_trend: 'normal',
    memory_trend: 'stable',
    disk_trend: 'normal',
    oom_risk: 0,
    timeout_risk: 0,
  },
  estimated_completion: null,
};

/** AI monitor alert card: `auto_fixable` renders the action button in the
 *  Monitor tab. Its `suggestion` is prose too, so the relabel applies there
 *  as well (issue #1146). */
const MONITOR_STATUS_WITH_ALERT = {
  ...MONITOR_STATUS,
  alerts: [
    {
      alert_id: 'failed:star_align:oom_killed',
      level: 'critical',
      rule_name: 'star_align',
      prediction: 'Rule star_align is likely to run out of memory',
      suggestion: 'Increase the memory allocation for star_align.',
      auto_fixable: true,
      needs_approval: false,
      timestamp: new Date().toISOString(),
    },
  ],
};

const REPORT = {
  qc_summary: {},
  key_findings: [],
  narrative_md: '',
  caveats: [],
  suggested_next: [],
  file_tree: [],
  charts: [],
};

const DAG_STATUS = {
  nodes: [],
  edges: [],
  parallel_groups: [],
  critical_path: [],
  metrics: {
    total_nodes: 0,
    completed_nodes: 0,
    failed_nodes: 0,
    running_nodes: 0,
    pending_nodes: 0,
    skipped_nodes: 0,
    cancelled_nodes: 0,
    eta_ms: null,
  },
};

const PREVIEW = {
  checkpoint_preview: {
    summary: { will_run: 1, will_skip: 0, protected_outside: 1 },
    plan: [{ name: 'star_align', status: 'will-rerun' }],
    cascade_chains: [],
  },
  execution_order: ['star_align'],
};

/** The exact shape GET /api/runs/{id}/diagnostics returns: the diagnostics
 *  engine's `FixAction` (description + placeholder config_change, command
 *  always null today) — NOT an applicable mutation payload. */
const DIAGNOSTICS_WITH_FIX = {
  failed_nodes: [
    {
      rule: 'star_align',
      error_pattern: 'oom_killed',
      likely_cause: 'Process killed by OS/cgroup due to memory limit (OOM / SIGKILL 137).',
      suggestions: ['Increase memory in [resources] section.'],
      auto_fixable: true,
      fix_action: {
        description: 'Scale memory allocation.',
        config_change: {
          path: 'resources.memory',
          old_value: 'current',
          new_value: 'increase',
        },
        command: null,
      },
      relevant_log_lines: ['Exit code 137 or SIGKILL received'],
    },
  ],
  warnings: [],
  resource_bottlenecks: [],
};

const DIAGNOSTICS_NO_FIX = {
  failed_nodes: [
    {
      rule: 'star_align',
      error_pattern: 'unknown_error',
      likely_cause: 'Process exited with code 1. Check logs.',
      suggestions: ['Review full execution log.'],
      auto_fixable: false,
      fix_action: null,
      relevant_log_lines: ['fatal error'],
    },
  ],
  warnings: [],
  resource_bottlenecks: [],
};

interface Captured {
  method: string;
  path: string;
  body: string;
}

function record(captured: Captured[], method: string, url: string, body: string) {
  captured.push({ method, path: new URL(url).pathname, body });
}

/** Register the routes that carry this spec's assertions.
 *
 *  Playwright resolves page.route handlers LAST-registered-first, so the
 *  catch-all (which proves no repair endpoint is called) goes in before the
 *  specific stubs; every non-GET under /api/runs/<id>/ is then either the
 *  retry route or an unexpected mutation. */
async function stubRun(page: Page, diagnostics: unknown, captured: Captured[]) {
  await page.route(`**/api/runs/${FAILED_RUN}/*`, (route) => {
    const req = route.request();
    if (req.method() === 'GET') return route.fallback();
    record(captured, req.method(), req.url(), req.postData() ?? '');
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ ok: true }) });
  });
  await page.route(`**/api/runs/${FAILED_RUN}/retry`, (route) => {
    record(captured, route.request().method(), route.request().url(), route.request().postData() ?? '');
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ new_run_id: RETRY_RUN, will_rerun: ['star_align'], will_skip: [] }),
    });
  });
  await page.route(`**/api/runs/${FAILED_RUN}/ai-status`, (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(MONITOR_STATUS) }),
  );
  await page.route(`**/api/runs/${FAILED_RUN}/report`, (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(REPORT) }),
  );
  await page.route(`**/api/runs/${FAILED_RUN}/dag-status`, (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(DAG_STATUS) }),
  );
  await page.route(`**/api/runs/${FAILED_RUN}/diagnostics`, (route) => {
    record(captured, route.request().method(), route.request().url(), route.request().postData() ?? '');
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(diagnostics) });
  });
  await page.route(`**/api/runs/${FAILED_RUN}/preview`, (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(PREVIEW) }),
  );
  // The retried run's detail panes: after the RetryPlan navigates, the page
  // loads them in place — answer with the same no-fix fixture.
  for (const pane of ['ai-status', 'report', 'dag-status', 'diagnostics', 'preview']) {
    await page.route(`**/api/runs/${RETRY_RUN}/${pane}`, (route) => {
      const body =
        pane === 'ai-status' ? MONITOR_STATUS
        : pane === 'report' ? REPORT
        : pane === 'dag-status' ? DAG_STATUS
        : pane === 'preview' ? PREVIEW
        : DIAGNOSTICS_NO_FIX;
      return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(body) });
    });
  }
}

/** Mutations other than the diagnostics GET and the retry itself. */
function unexpectedMutations(captured: Captured[]) {
  return captured.filter(
    (c) => c.method !== 'GET' && !c.path.endsWith(`/api/runs/${FAILED_RUN}/retry`),
  );
}

async function gotoDiagnostics(page: Page) {
  await page.goto(`/runs/${FAILED_RUN}`);
  await page.getByRole('tab', { name: 'Diagnostics' }).click();
  await expect(page.getByTestId('retry-unchanged').first()).toBeVisible();
}

test.describe('Monitor failure panel — Retry unchanged (issue #1146)', () => {
  test('advisory fix is displayed and Retry unchanged sends only the retry request', async ({ page }) => {
    const captured: Captured[] = [];

    await page.route('**/api/runs', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ items: [runItem(FAILED_RUN), runItem(RETRY_RUN, 'queued')], next_cursor: null, total: 2 }),
      }),
    );
    await stubRun(page, DIAGNOSTICS_WITH_FIX, captured);

    await gotoDiagnostics(page);

    // The repair is rendered as a manual suggestion, never as a one-click fix.
    const suggestion = page.getByTestId('fix-suggestion');
    await expect(suggestion).toHaveCount(1);
    await expect(suggestion).toContainText('Suggested fix (apply manually):');
    await expect(suggestion).toContainText('Scale memory allocation.');
    await expect(suggestion).toContainText('resources.memory: current → increase');
    await expect(page.getByText('Apply & Retry')).toHaveCount(0);
    await expect(page.getByText('1-Click Fix')).toHaveCount(0);

    const retry = page.getByTestId('retry-unchanged');
    await expect(retry).toHaveCount(1);
    await expect(retry).toHaveText(/Retry unchanged/);
    // The hint names the real semantics: same workdir + original snapshot,
    // so an edit only helps a NEW run (review of #1201).
    await expect(retry).toHaveAttribute('title', /same workflow snapshot in the same workdir/);

    await retry.click();

    // Exactly one mutation: the run-retry POST with the displayed payload.
    // A repair application would appear here as a second request and fail.
    await expect(page).toHaveURL(new RegExp(`/runs/${RETRY_RUN}`));
    expect(unexpectedMutations(captured)).toEqual([]);
    const retries = captured.filter((c) => c.path === `/api/runs/${FAILED_RUN}/retry`);
    expect(retries).toHaveLength(1);
    expect(retries[0].method).toBe('POST');
    expect(JSON.parse(retries[0].body || '{}')).toEqual({ skip_succeeded: true });
  });

  test('no repair payload: no suggestion block, Retry unchanged still sends no repair request', async ({ page }) => {
    const captured: Captured[] = [];

    await page.route('**/api/runs', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ items: [runItem(FAILED_RUN), runItem(RETRY_RUN, 'queued')], next_cursor: null, total: 2 }),
      }),
    );
    await stubRun(page, DIAGNOSTICS_NO_FIX, captured);

    await gotoDiagnostics(page);

    // No fix_action → nothing claims a repair, and no apply control exists.
    await expect(page.getByTestId('fix-suggestion')).toHaveCount(0);
    await expect(page.getByText('Suggested fix (apply manually):')).toHaveCount(0);
    await expect(page.getByText('Apply & Retry')).toHaveCount(0);

    const retry = page.getByTestId('retry-unchanged');
    await expect(retry).toHaveCount(1);
    await expect(retry).toHaveText(/Retry unchanged/);

    await retry.click();

    await expect(page).toHaveURL(new RegExp(`/runs/${RETRY_RUN}`));
    expect(unexpectedMutations(captured)).toEqual([]);
    const retries = captured.filter((c) => c.path === `/api/runs/${FAILED_RUN}/retry`);
    expect(retries).toHaveLength(1);
    expect(retries[0].method).toBe('POST');
    expect(JSON.parse(retries[0].body || '{}')).toEqual({ skip_succeeded: true });
  });

  test('alert card action is relabelled Retry unchanged without the manual-fix caption', async ({ page }) => {
    const captured: Captured[] = [];

    await page.route('**/api/runs', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ items: [runItem(FAILED_RUN), runItem(RETRY_RUN, 'queued')], next_cursor: null, total: 2 }),
      }),
    );
    await stubRun(page, DIAGNOSTICS_NO_FIX, captured);
    // Override the ai-status stub (registered later ⇒ wins) with an
    // auto_fixable alert; the monitor tab is the default view.
    await page.route(`**/api/runs/${FAILED_RUN}/ai-status`, (route) =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(MONITOR_STATUS_WITH_ALERT) }),
    );

    await page.goto(`/runs/${FAILED_RUN}`);
    await expect(page.getByText('Increase the memory allocation for star_align.')).toBeVisible();

    // The alert gets the honest label and the retry action, but not the
    // diagnostics caption (its suggestion is prose, not a structured fix).
    const alertRetry = page.getByRole('button', { name: /Retry unchanged/ });
    await expect(alertRetry).toHaveCount(1);
    await expect(alertRetry).toHaveAttribute('title', /same workflow snapshot in the same workdir/);
    await expect(page.getByText('Fix & Retry')).toHaveCount(0);
    await expect(page.getByText('Suggested fix (apply manually):')).toHaveCount(0);

    await alertRetry.click();

    await expect(page).toHaveURL(new RegExp(`/runs/${RETRY_RUN}`));
    expect(unexpectedMutations(captured)).toEqual([]);
    const retries = captured.filter((c) => c.path === `/api/runs/${FAILED_RUN}/retry`);
    expect(retries).toHaveLength(1);
    expect(retries[0].method).toBe('POST');
    expect(JSON.parse(retries[0].body || '{}')).toEqual({ skip_succeeded: true });
  });
});
