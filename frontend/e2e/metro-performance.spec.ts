import { test, expect } from '@playwright/test';

const TOML = `[workflow]
name = "metro-perf"
version = "1.0.0"

[[rules]]
name = "fastqc_raw"
output = ["out/qc.txt"]
shell = "echo qc > out/qc.txt"

[[rules]]
name = "rna_align"
input = ["out/qc.txt"]
output = ["out/aligned.txt"]
shell = "cp {input[0]} {output[0]}"

[[rules]]
name = "deseq2_stats"
input = ["out/aligned.txt"]
output = ["out/stats.txt"]
shell = "cp {input[0]} {output[0]}"

[[rules]]
name = "chip_peak"
input = ["out/stats.txt"]
output = ["out/peaks.txt"]
shell = "cp {input[0]} {output[0]}"

[[rules]]
name = "kraken_prof"
input = ["out/peaks.txt"]
output = ["out/profile.txt"]
shell = "cp {input[0]} {output[0]}"

[[rules]]
name = "final_report"
input = ["out/profile.txt"]
output = ["out/report.txt"]
shell = "cp {input[0]} {output[0]}"
`;

test.describe('Metro frame-time (UX-09 #1151)', () => {
  test.beforeEach(async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
  });

  test('granularity switches repaint within a bounded frame budget', async ({ page }) => {
    await page.goto('/editor');
    await page.locator('.cm-content').click();
    await page.keyboard.press('ControlOrMeta+a');
    await page.keyboard.insertText(TOML);
    await expect(page.locator('.val-badge')).toContainText('Valid', { timeout: 15_000 });
    await expect(page.locator('.rf-rule-node')).toHaveCount(6, { timeout: 15_000 });

    // Keyboard activation, mirroring the shipped Metro helper: UX-06
    // (#1148 / PR #1211) fixed the stacked view-toggle buttons and owns the
    // click-through regression. Enter on the focused native button exercises
    // the same state transition while this spec stays focused on layout
    // cost.
    const metroToggle = page.locator('.rf-metro-btn', { hasText: 'Metro View' });
    await metroToggle.focus();
    await page.keyboard.press('Enter');
    await expect(page.locator('.rf-metro-canvas-container')).toBeVisible();

    const samples = await page.evaluate(async () => {
      const buttons = Array.from(
        document.querySelectorAll<HTMLButtonElement>('.rf-metro-granularity-group button'),
      );
      const byLabel = (label: string) => {
        const button = buttons.find((b) => (b.textContent ?? '').trim() === label);
        if (!button) throw new Error(`granularity button "${label}" missing`);
        return button;
      };
      const nextPaint = () =>
        new Promise<void>((resolve) =>
          requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
        );
      const stationCount = () =>
        document.querySelectorAll('.rf-metro-station-card, .rf-metro-station-compact').length;

      // Warm-up: one full cycle so one-time layout/style work is not billed
      // to the measured samples.
      for (const label of ['Process', 'Module', 'Rule']) {
        byLabel(label).click();
        await nextPaint();
      }

      const out: Array<{ target: string; ms: number; stations: number }> = [];
      for (const label of ['Process', 'Module', 'Rule', 'Process', 'Rule', 'Module']) {
        const before = stationCount();
        const t0 = performance.now();
        byLabel(label).click();
        await nextPaint();
        out.push({ target: label, ms: performance.now() - t0, stations: stationCount(), before });
      }
      return out;
    });

    console.log(`METRO_SAMPLES ${JSON.stringify(samples)}`);

    // Anti-vacuity: each switch really regrouped the 6 rules.
    for (const sample of samples) {
      if (sample.target === 'Rule') expect(sample.stations).toBe(6);
      else if (sample.target === 'Process') expect(sample.stations).toBe(1);
      else expect(sample.stations).toBeGreaterThan(1);
      expect(sample.stations).not.toBe(sample.before);
    }

    // Generous budget: this catches a pathological re-render/layout
    // regression, not CI scheduling jitter. Locally the switches measure
    // well under 100 ms; CI headless is allowed an order of magnitude.
    const worst = Math.max(...samples.map((s) => s.ms));
    const median = [...samples.map((s) => s.ms)].sort((a, b) => a - b)[
      Math.floor(samples.length / 2)
    ];
    expect(worst, `worst interaction-to-paint ${worst.toFixed(1)}ms`).toBeLessThan(750);
    expect(median, `median interaction-to-paint ${median.toFixed(1)}ms`).toBeLessThan(300);
  });
});
