import { test, expect } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';

// WCAG scan (ledger UX-09, issue #1151).
//
// Scope: the six primary routes in the default (Guided) editor mode, scanned
// in BOTH color themes. The app follows `prefers-color-scheme` when the user
// has not pinned a theme, so `page.emulateMedia` selects the theme under
// test without touching localStorage.
//
// Rule set: axe's WCAG 2.0 + 2.1 A/AA tags. NO rule is disabled — the five
// findings this scan surfaced during development were fixed in the product
// instead: unlabeled quota inputs and select names on Settings (label/select
// htmlFor), the Command Palette's accessible-name/visible-text mismatch,
// light-theme success-badge and dark-theme nav/pill contrast. If a future
// rule must be excluded, list it here with the reason; a blanket disable is
// not acceptable.
//
// Deliberately outside this scan: the Canvas + TOML editor surface
// (`oxo_editor_mode=canvas`). It carries known, separate findings — the
// React Flow rule cards expose a role="button" whose visible text (rule,
// environment, shell) is wider than its aria-label, and CodeMirror's dark
// syntax palette is below AA — tracked as remaining UX-09 work rather than
// silently excluded here. The Guided editor scanned below is what a new
// user gets by default.

const WCAG_TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'];

const ROUTES: Array<{ path: string; ready: (page: import('@playwright/test').Page) => Promise<void> }> = [
  {
    path: '/',
    ready: async (page) => {
      await expect(page.locator('.entry-card').first()).toBeVisible({ timeout: 15_000 });
    },
  },
  {
    path: '/pipelines',
    ready: async (page) => {
      await expect(page.locator('main h1')).toBeVisible({ timeout: 15_000 });
      await expect(page.locator('.pill-chip').first()).toBeVisible({ timeout: 15_000 });
    },
  },
  {
    path: '/editor',
    ready: async (page) => {
      // Guided mode renders the Run button only once the server returned a
      // validation verdict for the default workflow.
      await expect(
        page.locator('.action-row').getByRole('button', { name: 'Run', exact: true }),
      ).toBeEnabled({ timeout: 15_000 });
    },
  },
  {
    path: '/runs',
    ready: async (page) => {
      await expect(page.locator('main h1')).toBeVisible({ timeout: 15_000 });
    },
  },
  {
    path: '/monitor',
    ready: async (page) => {
      await expect(page.getByText('Run History')).toBeVisible({ timeout: 15_000 });
    },
  },
  {
    path: '/settings',
    ready: async (page) => {
      await expect(page.getByText('AI Provider Configuration')).toBeVisible({ timeout: 15_000 });
      // The status badges are populated by an async config fetch; scanning
      // before they render would let a contrast regression slip through.
      await expect(page.locator('.status-badge').first()).toBeVisible({ timeout: 15_000 });
    },
  },
];

for (const colorScheme of ['light', 'dark'] as const) {
  test.describe(`WCAG 2.1 A/AA — ${colorScheme} theme`, () => {
    test.use({ colorScheme });

    for (const route of ROUTES) {
      test(`${route.path} has no axe violations`, async ({ page }) => {
        await page.goto(route.path);
        await route.ready(page);

        const results = await new AxeBuilder({ page }).withTags(WCAG_TAGS).analyze();
        const report = results.violations.map((v) => ({
          id: v.id,
          impact: v.impact,
          help: v.help,
          targets: v.nodes.map((n) => n.target.join(' ')).slice(0, 6),
        }));
        expect(report, `axe violations on ${route.path} (${colorScheme}): ${JSON.stringify(report, null, 2)}`).toEqual([]);
      });
    }
  });
}
