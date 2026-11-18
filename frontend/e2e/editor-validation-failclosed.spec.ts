import { test, expect, type Page } from '@playwright/test';

// Issue #1145 (UX-03): a validation-request failure (network error or 5xx)
// must invalidate the PRIOR successful verdict. The audit baseline caught the
// failure and kept the last known validation, so a transient backend failure
// left a green "Valid" badge and an enabled Run button on content that was
// never judged. The fail-closed contract pinned here:
//   - the badge drops out of "Valid" into "Not validated" (state unknown),
//   - Save refuses to claim a validated save (no lingering success result),
//   - Run/Dry-Run stay disabled until a validation request actually succeeds,
//   - a later successful validation (or a findings response) behaves as before.
//
// The stubs replace ONLY POST /api/pipelines/validate; parsing, DAG build and
// pipeline listing stay real, so the test drives the shipped editor code.

const BADGE = '.val-badge';
const TOOLBAR_RUN = '.panel-actions .btn-run';

async function openEditorWithValidWorkflow(page: Page) {
  await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
  await page.goto('/editor');
  await expect(page.locator(BADGE)).toContainText('Valid', { timeout: 15_000 });
  await expect(page.locator(TOOLBAR_RUN)).toBeEnabled();
  // Let the mount-time 300ms debounced buildDag+validate settle, so the
  // route stub installed afterwards owns every later validation request.
  await page.waitForTimeout(800);
}

async function clickSave(page: Page) {
  await page.locator('.panel-actions button').filter({ hasText: 'Save' }).click();
}

/** The fail-closed state: unknown verdict, no run affordance, no stale badge. */
async function expectFailClosed(page: Page) {
  await expect(page.locator('.val-badge.valid')).toHaveCount(0);
  await expect(page.locator(BADGE)).toContainText('Not validated');
  await expect(page.locator(TOOLBAR_RUN)).toBeDisabled();
  await expect(page.locator('.result-bar')).toContainText('Not saved: validation could not be confirmed');
  await expect(page.locator('.validation-errors')).toContainText('VALIDATION_UNAVAILABLE');
}

test.describe('Failed validation request invalidates the prior success (#1145)', () => {
  test('a 5xx validation response clears the Valid badge and blocks Run until validation succeeds', async ({ page, request }) => {
    await openEditorWithValidWorkflow(page);
    const before = (await (await request.get('/api/pipelines')).json()).length;

    let fail = true;
    await page.route('**/api/pipelines/validate', async (route) => {
      if (!fail) return route.continue();
      await route.fulfill({
        status: 500,
        contentType: 'application/json',
        body: JSON.stringify({ code: 'INTERNAL_ERROR', message: 'validation backend exploded' }),
      });
    });

    await clickSave(page);
    await expectFailClosed(page);
    // A save must not be reported (or recorded) on a never-validated document.
    const after = (await (await request.get('/api/pipelines')).json()).length;
    expect(after).toBe(before);

    // Recovery: a genuinely successful validation restores the run affordance.
    fail = false;
    await clickSave(page);
    await expect(page.locator(BADGE)).toContainText('Valid', { timeout: 10_000 });
    await expect(page.locator(TOOLBAR_RUN)).toBeEnabled();
    await expect(page.locator('.result-bar')).not.toContainText('validation could not be confirmed');
  });

  test('a network-aborted validation request fails closed the same way, then recovers', async ({ page }) => {
    await openEditorWithValidWorkflow(page);

    let fail = true;
    await page.route('**/api/pipelines/validate', async (route) => {
      if (!fail) return route.continue();
      await route.abort('failed');
    });

    await clickSave(page);
    await expectFailClosed(page);

    fail = false;
    await clickSave(page);
    await expect(page.locator(BADGE)).toContainText('Valid', { timeout: 10_000 });
    await expect(page.locator(TOOLBAR_RUN)).toBeEnabled();
  });

  test('a findings response still renders as validation errors and recovers on a clean validation', async ({ page }) => {
    await openEditorWithValidWorkflow(page);

    let findings = true;
    await page.route('**/api/pipelines/validate', async (route) => {
      if (!findings) return route.continue();
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          valid: false,
          errors: [{ code: 'E001', message: 'stubbed finding', rule: 'fastqc', suggestion: null, line: 1 }],
          warnings: [],
          rules: 2,
          dependencies: 0,
          missing_inputs: [],
        }),
      });
    });

    // Unchanged behavior: findings refuse the save with the existing error
    // list and never masquerade as "not validated".
    await clickSave(page);
    await expect(page.locator('.val-badge.invalid')).toContainText('1 error(s)');
    await expect(page.locator(TOOLBAR_RUN)).toBeDisabled();
    await expect(page.locator('.result-bar')).toContainText('Not saved: 1 validation error(s)');
    await expect(page.locator('.validation-errors')).toContainText('stubbed finding');
    await expect(page.locator('.val-badge.unavailable')).toHaveCount(0);

    findings = false;
    await clickSave(page);
    await expect(page.locator(BADGE)).toContainText('Valid', { timeout: 10_000 });
    await expect(page.locator(TOOLBAR_RUN)).toBeEnabled();
  });
});
