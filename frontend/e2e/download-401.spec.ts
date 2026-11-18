import { test, expect, type APIRequestContext, type Download, type Page } from '@playwright/test';
import { statSync } from 'node:fs';

// Issue #1139 (WEB-14): workdir downloads go through the authenticated
// fetch helper (`download()` in frontend/src/api/client.ts) and a rejected
// session must SURFACE (login redirect) instead of silently saving an empty
// file. The shipped e2e harness runs `oxo-flow-web` in personal mode, where
// no auth middleware is installed, so the server under test cannot produce
// the team/hpc 401 by itself. The negative cases below therefore install a
// route-level stand-in for the team/hpc `require_auth` gate
// (crates/oxo-flow-web/src/server.rs) that answers with the exact structured
// 401 bodies the middleware emits (AUTH_REQUIRED without a token,
// INVALID_TOKEN with a rejected one). Run creation, the monitor page and its
// report/file tree are REAL — the same real-run fixture core-flows Stage 4 & 5
// uses — so the only simulated part is the auth gate's response.
//
// The authenticated case runs against the real server: the workdir zip comes
// from GET /api/runs/{id}/files?path=. and must arrive through the fetch/Blob
// path with the session's Authorization header attached.

const RUN_WORKFLOW = (name: string) => `[workflow]
name = "${name}"
version = "1.0.0"

[[rules]]
name = "emit_summary"
output = ["results/summary.txt"]
shell = "mkdir -p results && echo e2e_download_auth > results/summary.txt"
threads = 1
`;

/** Create + execute a tiny real run (dry_run: false) and return its id. */
async function createCompletedRun(request: APIRequestContext, name: string): Promise<string> {
  const resp = await request.post('/api/runs', {
    data: { toml_content: RUN_WORKFLOW(name), dry_run: false },
  });
  expect(resp.ok()).toBeTruthy();
  const { run_id } = await resp.json();

  for (let i = 0; i < 80; i++) {
    const status = await (await request.get(`/api/runs/${run_id}`)).json();
    if (status.status === 'completed' || status.status === 'failed') {
      expect(status.status).toBe('completed');
      return run_id as string;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`run ${run_id} did not reach a terminal state`);
}

/**
 * Open the run's report tab and return the "Download Workdir (.zip)" button.
 * The button only renders once the real report carries a non-empty file tree.
 */
async function openReport(page: Page, runId: string) {
  await page.goto(`/runs/${runId}`);
  const reportTab = page.getByRole('tab', { name: /^Report$/ });
  await expect(reportTab).toBeVisible({ timeout: 15000 });
  await reportTab.click();
  const zipBtn = page.getByRole('button', { name: /Download Workdir \(\.zip\)/ });
  await expect(zipBtn).toBeVisible({ timeout: 15000 });
  return zipBtn;
}

/**
 * Simulated team/hpc require_auth gate. Records the Authorization header the
 * browser attached to every download request and answers 401 with the same
 * {code, message, suggestion} body the Rust middleware returns.
 */
async function installTeamModeAuthGate(page: Page): Promise<{ authHeaders: Array<string | undefined> }> {
  const authHeaders: Array<string | undefined> = [];
  await page.route(/\/api\/runs\/[^/]+\/files(\?|$)/, async (route) => {
    const auth = route.request().headers()['authorization'];
    authHeaders.push(auth);
    const body = auth
      ? {
          code: 'INVALID_TOKEN',
          message: 'Invalid or expired session token',
          detail: null,
          suggestion: 'Login again at POST /api/auth/login to obtain a fresh token',
        }
      : {
          code: 'AUTH_REQUIRED',
          message: 'Authentication required in team/hpc mode',
          detail: null,
          suggestion: 'Login at POST /api/auth/login to obtain a session token',
        };
    await route.fulfill({ status: 401, contentType: 'application/json', body: JSON.stringify(body) });
  });
  return { authHeaders };
}

test.describe('Download auth surfacing (WEB-14 / #1139)', () => {
  test('a 401 on an unauthenticated download redirects to login instead of saving a file', async ({ page, request }) => {
    const runId = await createCompletedRun(request, 'e2e-download-401-absent');
    const downloads: Download[] = [];
    page.on('download', (d) => downloads.push(d));
    const gate = await installTeamModeAuthGate(page);

    const zipBtn = await openReport(page, runId);
    expect(await page.evaluate(() => localStorage.getItem('oxo_token'))).toBeNull();
    await zipBtn.click();

    // The 401 must surface as the login page (the client helper's contract),
    // with the "no token at all" reason — not a silent empty-file download.
    await expect(page).toHaveURL(/\/login\?reason=invalid/, { timeout: 15000 });
    await expect(page.getByText('Sign-in is no longer valid — sign in again.')).toBeVisible();
    await page.waitForTimeout(500); // give a would-be silent download a chance to fire
    expect(downloads).toHaveLength(0);
    // The click really did issue the download request, and with no header.
    expect(gate.authHeaders).toHaveLength(1);
    expect(gate.authHeaders[0]).toBeUndefined();
  });

  test('a 401 on a stale-token download clears the session and shows the expiry notice', async ({ page, request }) => {
    const runId = await createCompletedRun(request, 'e2e-download-401-stale');
    const downloads: Download[] = [];
    page.on('download', (d) => downloads.push(d));
    const gate = await installTeamModeAuthGate(page);

    const zipBtn = await openReport(page, runId);
    // Seed a token AFTER load (addInitScript would re-seed it on the login
    // page and hide the clearing assertion below). authHeaders() reads
    // localStorage per request, so this is what the click sends.
    await page.evaluate(() => localStorage.setItem('oxo_token', 'e2e-stale-token'));
    await zipBtn.click();

    await expect(page).toHaveURL(/\/login\?reason=expired/, { timeout: 15000 });
    await expect(page.getByText('Session expired — sign in again.')).toBeVisible();
    await page.waitForTimeout(500);
    expect(downloads).toHaveLength(0);
    expect(gate.authHeaders).toEqual(['Bearer e2e-stale-token']);
    expect(await page.evaluate(() => localStorage.getItem('oxo_token'))).toBeNull();
  });

  test('an authenticated download saves the real workdir zip', async ({ page, request }) => {
    const runId = await createCompletedRun(request, 'e2e-download-401-ok');
    const filesAuth: Array<string | undefined> = [];
    page.on('request', (req) => {
      if (/\/api\/runs\/[^/]+\/files\?/.test(req.url())) {
        filesAuth.push(req.headers()['authorization']);
      }
    });

    const zipBtn = await openReport(page, runId);
    await page.evaluate(() => localStorage.setItem('oxo_token', 'e2e-valid-token'));

    const [download] = await Promise.all([page.waitForEvent('download'), zipBtn.click()]);
    expect(download.suggestedFilename()).toBe(`${runId}.zip`);
    const saved = await download.path();
    expect(saved).toBeTruthy();
    expect(statSync(saved as string).size).toBeGreaterThan(0);
    // A successful download stays on the run page (no login redirect).
    expect(new URL(page.url()).pathname).toBe(`/runs/${runId}`);
    // The request rode the same authenticated fetch path the 401 cases prove:
    // the session header was attached (WEB-14's original defect was a bare
    // <a href> that could not).
    expect(filesAuth).toContain('Bearer e2e-valid-token');
  });
});
