import { test, expect, type Page } from '@playwright/test';

// UX-06 (issue #1148): the Metro view must be fully keyboard-operable at Rule
// granularity, and the grouped granularities (Process / Module) must be
// READ-ONLY — a grouped station's id is an aggregation key (environment or
// modality), not a rule name, so edit/connect/remove there cannot be proven
// semantics-preserving (and a rule named like the key would be silently
// targeted instead of the group).
//
// The advanced fixture carries semantics the canvas must never lose while the
// grouped views are exercised: `when`, `depends_on`, `[engine]`, `[config]`,
// and a `[[values]]` fan-out.

const ADVANCED_TOML = `[workflow]
name = "advanced-metro"
version = "1.0"

[engine]
strict_placeholders = false

[config]
run_align = true

[[values]]
name = "lane"
values = ["L1", "L2"]

[[rules]]
name = "qc"
output = ["qc/{lane}.txt"]
shell = "echo qc > {output[0]}"

[[rules]]
name = "align"
input = ["qc/{lane}.txt"]
output = ["aln/{lane}.txt"]
shell = "cp {input[0]} {output[0]}"
environment = { docker = "ubuntu:22.04" }
when = "config.run_align"
depends_on = ["qc"]

[[rules]]
name = "call"
input = ["aln/{lane}.txt"]
output = ["vcf/{lane}.vcf"]
shell = "cp {input[0]} {output[0]}"
`;

/** Enter the metro view through the shipped toggle (keyboard activation). */
async function switchToMetro(page: Page) {
  await page.getByRole('button', { name: 'Metro View' }).focus();
  await page.keyboard.press('Enter');
  await expect(page.locator('.rf-metro-toolbar')).toBeVisible();
}

/** Click a granularity button in the metro toolbar. */
async function setGranularity(page: Page, label: 'Rule' | 'Process' | 'Module') {
  await page.locator('.rf-metro-toolbar').getByRole('button', { name: label, exact: true }).click();
}

/** Stations are the single focusable glyph (the React Flow wrapper is not). */
function stations(page: Page) {
  return page.locator('.rf-metro-canvas-container [data-station]');
}

async function expectFocusedStation(page: Page, name: string) {
  await expect
    .poll(() => page.evaluate(() => document.activeElement?.getAttribute('data-station') ?? null))
    .toBe(name);
}

async function tomlText(page: Page): Promise<string> {
  return page.locator('.cm-content').innerText();
}

test.describe('Metro canvas keyboard access and grouped read-only (UX-06, #1148)', () => {
  test.beforeEach(async ({ page }) => {
    // New users default to Guided mode; these specs exercise the canvas.
    await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
  });

  test('keyboard-only: Tab order reaches stations, Enter/Space open the editor, Escape closes it', async ({
    page,
  }) => {
    await page.goto('/editor');
    await expect(page.locator('.rf-rule-node')).toHaveCount(2);
    await switchToMetro(page);
    await expect(stations(page)).toHaveCount(2);

    // Focus order: the granularity buttons are the last focus stops before
    // the station glyphs in DOM order, so Tab moves station by station in
    // rule-declaration order (fastqc, align) and Shift+Tab reverses it.
    // Retry the first Tab: the view switch commits the node list a beat
    // after the toolbar paints, and a Tab landing in that gap reaches the
    // React Flow controls instead of a station.
    const moduleBtn = page.locator('.rf-metro-toolbar').getByRole('button', { name: 'Module', exact: true });
    await expect(async () => {
      await moduleBtn.focus();
      await page.keyboard.press('Tab');
      expect(await page.evaluate(() => document.activeElement?.getAttribute('data-station') ?? null)).toBe('fastqc');
    }).toPass({ timeout: 10_000 });
    await expectFocusedStation(page, 'fastqc');
    await page.keyboard.press('Tab');
    await expectFocusedStation(page, 'align');
    await page.keyboard.press('Shift+Tab');
    await expectFocusedStation(page, 'fastqc');

    // Enter activates the focused station and opens exactly that rule.
    await page.keyboard.press('Enter');
    await expect(page.locator('#rule-inspector-title')).toContainText('fastqc');
    // Escape closes the editor and returns focus to the station it came from.
    await page.keyboard.press('Escape');
    await expect(page.locator('#rule-inspector-title')).toHaveCount(0);
    await expectFocusedStation(page, 'fastqc');

    // Space is the second documented activation key.
    await page.keyboard.press(' ');
    await expect(page.locator('#rule-inspector-title')).toContainText('fastqc');
    await page.keyboard.press('Escape');
    await expect(page.locator('#rule-inspector-title')).toHaveCount(0);

    // Escape with no interaction pending cancels connect mode elsewhere; it
    // must never mutate the document.
    await expect(page.locator('.cm-content')).toContainText('shell = "fastqc {input} -o qc/"');

    // The view toggle is mouse-reachable too. Pre-fix the toggle buttons were
    // absolutely positioned on top of each other, leaving Metro View buried
    // under Auto layout in the standard view (so the metro canvas itself was
    // mouse-unreachable in the editor).
    await page.getByRole('button', { name: 'DAG View' }).click();
    await expect(page.locator('.rf-metro-toolbar')).toHaveCount(0);
    await page.getByRole('button', { name: 'Metro View' }).click();
    await expect(page.locator('.rf-metro-toolbar')).toBeVisible();
  });

  test('keyboard connect and delete edit exactly the focused rule; Escape cancels a pending connection', async ({
    page,
  }) => {
    await page.goto('/editor');
    await expect(page.locator('.rf-rule-node')).toHaveCount(2);
    await switchToMetro(page);

    const fastqc = page.locator('[data-station="fastqc"]');
    const align = page.locator('[data-station="align"]');
    await expect(fastqc).toBeVisible();

    // 'c' starts a keyboard connection; the live region states the mode and
    // the source station is marked (the keyboard equivalent of the drag).
    // Retried like every focus+key step: the view switch commits the station
    // list a beat after the toolbar, and a focus landing in that gap is lost.
    await expect(async () => {
      if ((await fastqc.getAttribute('data-connect-source')) !== 'true') {
        await fastqc.focus();
        await page.keyboard.press('c');
      }
      await expect(page.locator('.rf-metro-announcer')).toContainText('Connecting from fastqc', {
        timeout: 2_000,
      });
    }).toPass({ timeout: 10_000 });
    await expect(fastqc).toHaveAttribute('data-connect-source', 'true');

    // Tab to the target and Enter completes the same `connect` edit the
    // mouse drag performs — the TOML gains exactly one declared dependency.
    await page.keyboard.press('Tab');
    await expectFocusedStation(page, 'align');
    await page.keyboard.press('Enter');
    await expect(page.locator('.cm-content')).toContainText('depends_on = ["fastqc"]');
    await expect(page.locator('.rf-metro-announcer')).toContainText('Connected fastqc to align');

    // Escape cancels a started connection without touching the document.
    const beforeCancel = await tomlText(page);
    await expect(async () => {
      if ((await align.getAttribute('data-connect-source')) !== 'true') {
        await align.focus();
        await page.keyboard.press('c');
      }
      await expect(align).toHaveAttribute('data-connect-source', 'true', { timeout: 2_000 });
    }).toPass({ timeout: 10_000 });
    await page.keyboard.press('Escape');
    await expect(page.locator('.rf-metro-announcer')).toContainText('Connection cancelled');
    await expect(align).not.toHaveAttribute('data-connect-source', 'true');
    expect(await tomlText(page)).toBe(beforeCancel);

    // Delete removes the focused rule (the metro view had no delete path in
    // any modality before this fix).
    await expect(async () => {
      await align.focus();
      await page.keyboard.press('Delete');
      await expect(page.locator('.cm-content')).not.toContainText('name = "align"', {
        timeout: 2_000,
      });
    }).toPass({ timeout: 10_000 });
    await expect(stations(page)).toHaveCount(1);
    await expect(page.locator('.rf-metro-announcer')).toContainText('Removed rule align');
  });

  test('grouped granularities are read-only; the advanced document survives every grouped interaction', async ({
    page,
  }) => {
    await page.goto('/editor');
    await page.locator('.cm-content').click();
    await page.keyboard.press('ControlOrMeta+a');
    await page.keyboard.insertText(ADVANCED_TOML);
    await expect(page.locator('.val-badge')).toContainText('Valid', { timeout: 10_000 });
    await expect(page.locator('.rf-rule-node')).toHaveCount(3);
    await switchToMetro(page);

    const before = await tomlText(page);

    for (const granularity of ['Process', 'Module'] as const) {
      await setGranularity(page, granularity);

      const groupedStations = stations(page);
      await expect(groupedStations.first()).toBeVisible();
      // Aggregation really happened (3 rules collapse into < 3 stations).
      expect(await groupedStations.count()).toBeLessThan(3);

      // Read-only state is advertised, not silently inert: the toolbar note,
      // a per-station badge, and no connect handles at all (the mouse-only
      // drag path is gone in grouped views).
      await expect(page.getByTestId('metro-grouped-note')).toBeVisible();
      await expect(page.locator('.rf-metro-canvas-container .react-flow__handle')).toHaveCount(0);
      const flags = await groupedStations.evaluateAll((nodes) =>
        nodes.map((n) => ({
          grouped: n.getAttribute('data-grouped'),
          interactive: n.getAttribute('data-interactive'),
          role: n.getAttribute('role'),
        })),
      );
      expect(flags.length).toBeGreaterThan(0);
      for (const flag of flags) {
        expect(flag.grouped).toBe('true');
        expect(flag.interactive).toBeNull();
        expect(flag.role).toBe('group');
      }

      const first = groupedStations.first();

      // Mouse attempt first: double-clicking a group must NOT open a rule
      // editor (the pre-fix path opened it for the group key, e.g.
      // "system (2)"; the wrong-rule hazard is the finding).
      await first.dblclick();
      await expect(page.locator('#rule-inspector-title')).toHaveCount(0);

      // Keyboard attempts: Enter/Space announce the read-only reason; C and
      // Delete/Backspace are inert. None may mutate the document. The
      // focus+key step retries: a granularity switch re-renders the station
      // list, and a focus that lands on a node React replaces is lost.
      await expect(async () => {
        await first.focus();
        await page.keyboard.press('Enter');
        await expect(page.locator('.rf-metro-announcer')).toContainText('read-only', { timeout: 2_000 });
      }).toPass({ timeout: 10_000 });
      await page.keyboard.press(' ');
      await page.keyboard.press('c');
      await page.keyboard.press('Delete');
      await page.keyboard.press('Backspace');
      await expect(page.locator('#rule-inspector-title')).toHaveCount(0);
      expect(await tomlText(page), `grouped ${granularity} view must not edit the document`).toBe(before);
    }

    // Rule granularity restores the editing path on the same document, and
    // the keyboard editor opens the EXACT rule — not a group key.
    await setGranularity(page, 'Rule');
    await expect(stations(page)).toHaveCount(3);
    for (const flag of await stations(page).evaluateAll((nodes) =>
      nodes.map((n) => n.getAttribute('data-interactive')),
    )) {
      expect(flag).toBe('true');
    }
    const align = page.locator('[data-station="align"]');
    // Same focus race as above: the station list is rebuilt by the
    // granularity switch, so retry focus + activation until it sticks.
    await expect(async () => {
      await align.focus();
      expect(await page.evaluate(() => document.activeElement?.getAttribute('data-station') ?? null)).toBe('align');
      await page.keyboard.press('Enter');
      await expect(page.locator('#rule-inspector-title')).toContainText('align', { timeout: 2_000 });
    }).toPass({ timeout: 15_000 });
    await page.keyboard.press('Escape');
    await expect(page.locator('#rule-inspector-title')).toHaveCount(0);
    expect(await tomlText(page)).toBe(before);
  });
});
