import { test, expect, type Page, type Route } from '@playwright/test';

/**
 * Task 7b / I1 + M8: the Settings primary Save is role/state-aware.
 *
 * The tests intercept the AI-config writes (and the identity/mode reads) so
 * the assertions are about the SPA's request choice and messaging, not about
 * mutating the shared e2e server's real settings. Mode/role are mocked for
 * every test so the spec also runs against a team server (review Minor 5).
 */

interface CapturedWrites {
  server: string[];
  user: string[];
  runtime: string[];
}

function captureWrites(page: Page, options: {
  serverResponse?: Record<string, unknown>;
  userResponse?: Record<string, unknown>;
}): CapturedWrites {
  const captured: CapturedWrites = { server: [], user: [], runtime: [] };

  page.route('**/api/ai/config/server', async (route: Route) => {
    if (route.request().method() !== 'PUT') return route.continue();
    captured.server.push(route.request().url());
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(options.serverResponse ?? {
        status: 'updated', provider: 'openai', applied_to_runtime: true, disabled_by_env: false,
      }),
    });
  });

  page.route('**/api/ai/config/user', async (route: Route) => {
    if (route.request().method() !== 'PUT') return route.continue();
    captured.user.push(route.request().url());
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(options.userResponse ?? {
        status: 'updated', provider: 'openai', applied_to_runtime: false, disabled_by_env: false,
      }),
    });
  });

  // The runtime-only route (POST /api/ai/config) must never be called by the
  // primary Save: it is admin-only and refuses under the provider pin.
  page.route('**/api/ai/config', async (route: Route) => {
    if (route.request().method() !== 'POST') return route.continue();
    captured.runtime.push(route.request().url());
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ provider: 'openai', model: null, api_url: null, is_configured: true }),
    });
  });

  return captured;
}

async function mockMode(page: Page, mode: 'personal' | 'team') {
  await page.route('**/api/health', (route) => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({
      status: 'ok', version: 'test', mode, uptime_secs: 1,
      components: {
        database: { status: 'ok', latency_ms: null },
        filesystem: { status: 'ok', latency_ms: null },
        scheduler: null,
        ai_provider: { status: 'disabled', latency_ms: null },
      },
      resources: { cpu_pct: 0, memory_used_pct: 0, disk_used_pct: 0 },
      license: { license_type: 'academic', valid: true, commercial_use: 'no', contact: '', message: '' },
    }),
  }));
}

async function mockRole(page: Page, role: 'admin' | 'user') {
  await page.route('**/api/auth/me', (route) => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({ authenticated: true, username: `e2e-${role}`, role }),
  }));
}

async function chooseProvider(page: Page) {
  const provider = page.getByLabel('Provider').first();
  await provider.selectOption('openai');
  await page.getByPlaceholder('deepseek-v4-pro').fill('settings-save-model');
}

test('personal-mode Save persists the server tier and the personal pseudo-user row', async ({ page }) => {
  await mockMode(page, 'personal');
  const writes = captureWrites(page, {});
  await page.goto('/settings');
  await expect(page.getByRole('button', { name: 'AI Provider Configuration' })).toBeVisible();
  await chooseProvider(page);

  await page.getByRole('button', { name: 'Save', exact: true }).click();

  await expect(page.getByText(/Saved & persisted/)).toBeVisible();
  expect(writes.server, 'primary Save must PUT /api/ai/config/server').toHaveLength(1);
  // The historical Settings row (personal pseudo-user "default") takes
  // precedence over the server tier in provider_for()/restart restore, so the
  // admin/personal Save keeps both tiers in sync (review C1).
  expect(writes.user, 'admin Save must also sync the private row').toHaveLength(1);
  expect(writes.runtime, 'the admin-only runtime POST must not be used').toHaveLength(0);
});

test('team admin Save persists both the shared and private tiers', async ({ page }) => {
  await mockMode(page, 'team');
  await mockRole(page, 'admin');
  const writes = captureWrites(page, {});
  await page.goto('/settings');
  await expect(page.getByRole('button', { name: 'AI Provider Configuration' })).toBeVisible();
  await chooseProvider(page);

  await page.getByRole('button', { name: 'Save', exact: true }).click();

  await expect(page.getByText(/Saved & persisted/)).toBeVisible();
  expect(writes.server).toHaveLength(1);
  expect(writes.user, 'a team admin private row must not stay stale').toHaveLength(1);
  expect(writes.runtime).toHaveLength(0);
});

test('team user Save persists the user tier without the admin-only runtime POST', async ({ page }) => {
  await mockMode(page, 'team');
  await mockRole(page, 'user');
  const writes = captureWrites(page, {
    userResponse: { status: 'updated', provider: 'openai', applied_to_runtime: false, disabled_by_env: false },
  });

  await page.goto('/settings');
  await expect(page.getByRole('button', { name: 'AI Provider Configuration' })).toBeVisible();
  await chooseProvider(page);

  await page.getByRole('button', { name: 'Save', exact: true }).click();

  await expect(page.getByText(/Saved for your account/)).toBeVisible();
  expect(writes.user, 'non-admin Save must PUT /api/ai/config/user').toHaveLength(1);
  expect(writes.server).toHaveLength(0);
  expect(writes.runtime, 'the admin-only runtime POST must be skipped').toHaveLength(0);

  // M8/Minor 6: admin-only webhook section and AI Test control are not
  // rendered for a regular user, so no "Admin role required" banner appears.
  await expect(page.getByText(/Admin role required/)).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Run Notifications (Webhook)' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Test Connection' })).toHaveCount(0);
});

test('pinned-disabled Save persists and reports applied_to_runtime:false', async ({ page }) => {
  await mockMode(page, 'personal');
  const writes = captureWrites(page, {
    serverResponse: { status: 'updated', provider: 'openai', applied_to_runtime: false, disabled_by_env: true },
  });
  await page.goto('/settings');
  await expect(page.getByRole('button', { name: 'AI Provider Configuration' })).toBeVisible();
  await chooseProvider(page);

  await page.getByRole('button', { name: 'Save', exact: true }).click();

  await expect(page.getByText(/OXO_FLOW_AI_PROVIDER=disabled/)).toBeVisible();
  expect(writes.server).toHaveLength(1);
  expect(writes.user).toHaveLength(1);
  expect(writes.runtime).toHaveLength(0);
});
