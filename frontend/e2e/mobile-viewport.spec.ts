import { test, expect, type Page } from '@playwright/test';

// UX-09 (issue #1151): the SPA is a published surface on phones, but the
// only pre-existing mobile check (browser-ui-interaction.spec.ts) asserted
// that the page merely rendered. These tests pin the two properties a
// phone user actually depends on: NOTHING makes the document scroll
// sideways, and the author -> run path is reachable at 390x844 (iPhone
// 12/13/14 logical resolution).

const MOBILE = { width: 390, height: 844 } as const;

/** Document-level horizontal overflow, allowed 1px for sub-pixel rounding. */
async function horizontalOverflow(page: Page): Promise<{ scrollWidth: number; clientWidth: number }> {
  return page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
}

async function expectNoHorizontalOverflow(page: Page, route: string) {
  await expect(page.locator('main')).toBeVisible({ timeout: 15_000 });
  const { scrollWidth, clientWidth } = await horizontalOverflow(page);
  expect(
    scrollWidth,
    `${route} must not scroll horizontally at ${MOBILE.width}px (documentElement.scrollWidth ${scrollWidth} > clientWidth ${clientWidth})`,
  ).toBeLessThanOrEqual(clientWidth + 1);
}

test.describe('Mobile viewport (390x844, UX-09 #1151)', () => {
  test.use({ viewport: MOBILE });

  test('every primary route fits 390x844 without horizontal scroll', async ({ page }) => {
    // The routes a phone user hits in the core author -> validate -> run ->
    // monitor loop, plus the two list surfaces most likely to grow wide
    // (pipelines, settings forms). Each is checked independently so a
    // regression names the offending page.
    for (const route of ['/', '/pipelines', '/editor', '/runs', '/monitor', '/settings']) {
      await page.goto(route);
      await expectNoHorizontalOverflow(page, route);
    }
  });

  test('author -> run is reachable from the mobile drawer', async ({ page }) => {
    await page.goto('/');

    // Below 1024px the sidebar is a drawer (issue #947); open it through the
    // same control a user taps.
    await page.getByRole('button', { name: 'Open navigation menu' }).click();
    const drawer = page.locator('.sidebar.drawer');
    await expect(drawer).toBeVisible();
    await drawer.getByText('Pipeline Editor').first().click();

    await expect(page).toHaveURL(/\/editor$/);
    await expectNoHorizontalOverflow(page, '/editor');

    // Guided mode is the default for a new user: the Run button is the
    // primary entry to the run dialog and is enabled only after the server
    // validated the document, so waiting for enabled covers author ->
    // validate as well.
    const runButton = page.locator('.action-row').getByRole('button', { name: 'Run', exact: true });
    await expect(runButton).toBeEnabled({ timeout: 15_000 });
    await runButton.click();

    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible();
    // The dialog's close control must stay inside the viewport — an
    // off-screen close button traps a touch user in the modal.
    const closeBox = await dialog
      .getByRole('button', { name: /close/i })
      .first()
      .boundingBox();
    expect(closeBox, 'run dialog close button must be laid out').not.toBeNull();
    expect(closeBox!.x).toBeGreaterThanOrEqual(0);
    expect(closeBox!.x + closeBox!.width).toBeLessThanOrEqual(MOBILE.width + 1);
    await expectNoHorizontalOverflow(page, '/editor (run dialog open)');
  });

  test('canvas view stays inside the viewport on a phone', async ({ page }) => {
    // Canvas mode is the dense surface: React Flow controls and the node
    // cards must not widen the document.
    await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
    await page.goto('/editor');
    await expect(page.locator('.rf-rule-node').first()).toBeVisible({ timeout: 15_000 });
    await expectNoHorizontalOverflow(page, '/editor (canvas)');
  });
});
