import { test, expect, type Page } from '@playwright/test';

// UX-09 (issue #1151): the SPA is a published surface on phones. These tests
// pin the two properties a phone user depends on: routed CONTENT does not
// overflow sideways, and the author -> run path is reachable at 390x844
// (iPhone 12/13/14 logical resolution).
//
// Important: below 800px the stylesheet sets `body { overflow-x: hidden }`
// and `.main-content { overflow-x: hidden }`, so `documentElement.scrollWidth`
// is clamped and a 2000px-wide child would still pass that check. The
// authoritative assertion is `.main-content.scrollWidth` (scrollWidth reports
// overflowing content even when `overflow` clips it), backed by an explicit
// negative-control test that injects a wide element.

const MOBILE = { width: 390, height: 844 } as const;

interface Overflow {
  document: { scrollWidth: number; clientWidth: number };
  main: { scrollWidth: number; clientWidth: number };
}

/** Routed-content overflow, immune to body/.main-content overflow clipping. */
async function measureOverflow(page: Page): Promise<Overflow> {
  return page.evaluate(() => {
    const main = document.querySelector<HTMLElement>('.main-content');
    if (!main) throw new Error('.main-content is missing');
    const doc = document.documentElement;
    return {
      document: { scrollWidth: doc.scrollWidth, clientWidth: doc.clientWidth },
      main: { scrollWidth: main.scrollWidth, clientWidth: main.clientWidth },
    };
  });
}

async function expectNoHorizontalOverflow(page: Page, route: string) {
  await expect(page.locator('main')).toBeVisible({ timeout: 15_000 });
  const overflow = await measureOverflow(page);
  expect(
    overflow.main.scrollWidth,
    `${route} content must not overflow at ${MOBILE.width}px (.main-content scrollWidth ${overflow.main.scrollWidth} > clientWidth ${overflow.main.clientWidth})`,
  ).toBeLessThanOrEqual(overflow.main.clientWidth + 1);
  expect(
    overflow.document.scrollWidth,
    `${route} document must not overflow at ${MOBILE.width}px (scrollWidth ${overflow.document.scrollWidth} > clientWidth ${overflow.document.clientWidth})`,
  ).toBeLessThanOrEqual(overflow.document.clientWidth + 1);
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

  test('the overflow detector catches a deliberately wide element', async ({ page }) => {
    // Negative control: without this, a detector that silently reads a
    // clipped ancestor would let real regressions through.
    await page.goto('/');
    await expect(page.locator('main')).toBeVisible();
    const before = await measureOverflow(page);
    expect(before.main.scrollWidth).toBeLessThanOrEqual(before.main.clientWidth + 1);

    await page.evaluate(() => {
      const main = document.querySelector('.main-content');
      if (!main) throw new Error('.main-content is missing');
      const wide = document.createElement('div');
      wide.id = 'overflow-probe';
      wide.style.width = '2000px';
      wide.style.height = '1px';
      main.appendChild(wide);
    });

    const after = await measureOverflow(page);
    expect(
      after.main.scrollWidth,
      'a 2000px child must register as .main-content overflow',
    ).toBeGreaterThan(after.main.clientWidth + 1);
    // Documents why the old assertion was vacuous: the body still clips.
    expect(after.document.scrollWidth).toBeLessThanOrEqual(after.document.clientWidth + 1);
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
    // cards must not widen the routed content.
    await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
    await page.goto('/editor');
    await expect(page.locator('.rf-rule-node').first()).toBeVisible({ timeout: 15_000 });
    await expectNoHorizontalOverflow(page, '/editor (canvas)');
  });
});
