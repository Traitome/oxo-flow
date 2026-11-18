import { test, expect } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';

// WCAG scan (ledger UX-09, issue #1151).
//
// Scope: the six primary routes in the default (Guided) editor mode, scanned
// in BOTH color themes, PLUS the canvas editor mode (rule cards + TOML pane)
// in both themes, PLUS the mobile shell at 390x844 (drawer closed and
// open) — the drawer's ARIA association only exists at phone widths. The app
// follows `prefers-color-scheme` when the user has not pinned a theme, so
// `page.emulateMedia` selects the theme under test without touching
// localStorage.
//
// Rule set: axe's WCAG 2.0 + 2.1 A/AA tags. NO rule is disabled — every
// finding this scan surfaced was fixed in the product instead: unlabeled
// quota inputs and select names on Settings (label/select htmlFor), the
// Command Palette's accessible-name/visible-text mismatch, light/dark
// contrast (success/error badges, tertiary text, nav, filter chips), the
// disabled-AI Dashboard card's opacity, and the mobile drawer's stale
// aria-controls. If a future rule must be excluded, list it here with the
// reason; a blanket disable is not acceptable.
//
// The CI e2e job provides no AI provider env, so these scans exercise the
// AI-disabled Dashboard and Settings states (the disabled-AI card and the
// "Not Configured" badge are exactly what the contrast fixes target). A
// developer running locally with provider credentials sees the configured
// variants instead; both states are AA in both themes.
//
// The Canvas + TOML editor surface (`oxo_editor_mode=canvas`) is scanned below
// too. Two product fixes landed with it (issue #1223): the React Flow rule
// cards previously exposed a role="button" whose visible text (rule,
// environment, shell) was wider than its bare "Edit rule <name>" aria-label —
// label-content-name-mismatch — now composed with aria-labelledby over the
// rendered name/meta/shell nodes; and CodeMirror's built-in light-only syntax
// palette was below AA on the dark editor background — replaced with a
// theme-aware AA palette. The Guided editor scanned above is what a new user
// gets by default; the canvas scans exercise the other editor mode.

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

async function expectNoAxeViolations(page: import('@playwright/test').Page, label: string) {
  const results = await new AxeBuilder({ page }).withTags(WCAG_TAGS).analyze();
  const report = results.violations.map((v) => ({
    id: v.id,
    impact: v.impact,
    help: v.help,
    targets: v.nodes.map((n) => n.target.join(' ')).slice(0, 6),
  }));
  expect(report, `axe violations on ${label}: ${JSON.stringify(report, null, 2)}`).toEqual([]);
}

for (const colorScheme of ['light', 'dark'] as const) {
  test.describe(`WCAG 2.1 A/AA — ${colorScheme} theme`, () => {
    test.use({ colorScheme });

    for (const route of ROUTES) {
      test(`${route.path} has no axe violations`, async ({ page }) => {
        await page.goto(route.path);
        await route.ready(page);
        await expectNoAxeViolations(page, `${route.path} (${colorScheme})`);
      });
    }
  });
}

// Canvas mode (issue #1223): the same /editor route in the other editor
// mode. This mode adds two surfaces the Guided scans never mount — the React
// Flow rule cards (role=button) and the CodeMirror TOML pane — both scanned
// in BOTH themes. editMode pins the same localStorage key editor-canvas.spec.ts
// uses.
for (const colorScheme of ['light', 'dark'] as const) {
  test.describe(`WCAG 2.1 A/AA — canvas editor, ${colorScheme} theme`, () => {
    test.use({ colorScheme });

    test('/editor (canvas) has no axe violations', async ({ page }) => {
      await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
      await page.goto('/editor');
      // Both surfaces must have rendered; the rule cards mount once the
      // server returned a verdict for the default workflow.
      await expect(page.locator('.rf-rule-node').first()).toBeVisible({ timeout: 15_000 });
      await expect(page.locator('.cm-content')).toBeVisible({ timeout: 15_000 });
      await expectNoAxeViolations(page, `/editor canvas (${colorScheme})`);
    });
  });
}

// The desktop scans above never render the phone shell. Below 1024px the
// sidebar becomes a drawer that is mounted only while open; a stale
// aria-controls pointed at the absent drawer id is an
// aria-valid-attr-value violation that only the mobile viewport exposes.
test.describe('WCAG 2.1 A/AA — mobile viewport (390x844)', () => {
  test.use({ viewport: { width: 390, height: 844 } });

  for (const colorScheme of ['light', 'dark'] as const) {
    test(`${colorScheme} theme: dashboard with the drawer closed`, async ({ page }) => {
      await page.emulateMedia({ colorScheme });
      await page.goto('/');
      await expect(page.locator('.entry-card').first()).toBeVisible({ timeout: 15_000 });
      // The drawer toggle must not reference a drawer that is not mounted.
      await expect(page.getByRole('button', { name: 'Open navigation menu' })).not.toHaveAttribute(
        'aria-controls',
        /app-sidebar/,
      );
      await expectNoAxeViolations(page, `/ mobile, drawer closed (${colorScheme})`);
    });
  }

  test('drawer open: aria-controls resolves to the mounted dialog', async ({ page }) => {
    await page.goto('/');
    const toggle = page.getByRole('button', { name: 'Open navigation menu' });
    await toggle.click();
    await expect(page.locator('.sidebar.drawer')).toBeVisible();
    await expect(toggle).toHaveAttribute('aria-expanded', 'true');
    await expect(toggle).toHaveAttribute('aria-controls', 'app-sidebar');
    await expectNoAxeViolations(page, '/ mobile, drawer open');
  });
});
