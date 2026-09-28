import { test, expect } from '@playwright/test';

// Issue #580: a past CodeMirror regression shipped with duplicated
// @codemirror/@lezer packages and crashed the TOML editor as a silent
// console error — the UI looked alive while typing did nothing. These
// specs pin the healthy path: a template-load + edit + mode-switch
// session must complete with ZERO console errors or page exceptions.
//
// One listener pair drives every assertion in this file. Playwright's
// `test.step` keeps failures attributable while the error list keeps the
// whole session under observation, including async work that fires after
// the last interaction.

test.describe('Editor session is console-error free (#580)', () => {
  // The editor defaults to Guided mode for new users (issue #82 P1-5);
  // the canvas assertions below need the canvas view mounted.
  test.beforeEach(async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
  });

  test('template load + TOML edit + guided/canvas switches emit no console errors', async ({ page }) => {
    const consoleErrors: string[] = [];
    const pageErrors: string[] = [];
    page.on('console', (msg) => {
      if (msg.type() === 'error') consoleErrors.push(msg.text());
    });
    page.on('pageerror', (err) => pageErrors.push(err.message));

    await test.step('load the seeded hello-world template', async () => {
      await page.goto('/editor?template=hello-world');
      await expect(page.locator('.result-bar.success')).toContainText('Loaded template "hello-world"', {
        timeout: 15_000,
      });
    });

    await test.step('canvas mode renders the template DAG', async () => {
      await expect(page.locator('.rf-rule-node', { hasText: 'greet' })).toBeVisible({ timeout: 10_000 });
    });

    await test.step('edit TOML and see validation stay green', async () => {
      await page.locator('.cm-content').click();
      await page.keyboard.press('ControlOrMeta+a');
      await page.keyboard.insertText(
        [
          '[workflow]',
          'name = "hello-world"',
          '',
          '[[rules]]',
          'name = "greet"',
          'output = ["hello.txt"]',
          'shell = "echo Edited, oxo-flow! > {output[0]}"',
        ].join('\n'),
      );
      await expect(page.locator('.val-badge')).toContainText('Valid', { timeout: 10_000 });
      // The debounce (300ms) plus buildDag/validate round-trips from the
      // edit must land before we trust the console capture below.
      await page.waitForTimeout(1_000);
    });

    await test.step('switch to guided mode and back', async () => {
      // Mode switches remount CodeMirror and React Flow — the exact
      // mount/unmount cycle where mixed package copies used to crash.
      await page.getByRole('button', { name: /Guided/ }).click();
      await expect(page.locator('.guided-meta')).toBeVisible();
      await page.getByRole('button', { name: /Canvas \+ TOML/ }).click();
      await expect(page.locator('.cm-content')).toBeVisible();
      await page.waitForTimeout(1_000);
    });

    await test.step('assert the session stayed clean', async () => {
      expect(consoleErrors, `console errors: ${consoleErrors.join(' | ')}`).toEqual([]);
      expect(pageErrors, `page exceptions: ${pageErrors.join(' | ')}`).toEqual([]);
    });
  });
});
