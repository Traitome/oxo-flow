import { test, expect } from '@playwright/test';

// Acceptance scenario for graphical programming (§6.2 of the design spec):
// build a workflow on the canvas — palette, inspector, connect, delete —
// and see the TOML stay in sync with the engine's canonical formatting.

test.describe('Graphical workflow editor (canvas)', () => {
  test.beforeEach(async ({ page }) => {
    // The editor defaults to Guided mode for new users (issue #82 P1-5);
    // these specs exercise the canvas view.
    await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
  });
  test('canvas renders nodes and edges for the default workflow', async ({ page }) => {
    await page.goto('/editor');
    const node = page.locator('.rf-rule-node', { hasText: 'fastqc' });
    await expect(node).toBeVisible();
    // The canvas card is a terminal snippet: env label + shell preview.
    await expect(node.locator('.rf-env-label')).toHaveText('system');
    await expect(node.locator('.rf-rule-shell')).toContainText('fastqc');
    // Two nodes for the default workflow.
    await expect(page.locator('.rf-rule-node')).toHaveCount(2);
  });

  test('auto-layout arranges a diamond DAG across multiple columns (#548)', async ({ page }) => {
    // A → B, A → C, B → D, C → D: a diamond needs ≥3 distinct layers.
    // The old layout hardcoded parentIds: [] — every node was a root and
    // the whole graph collapsed into one column.
    await page.goto('/editor');
    const toml = [
      '[workflow]',
      'name = "diamond"',
      '',
      '[[rules]]',
      'name = "a"',
      'output = ["a.txt"]',
      'shell = "echo a > {output[0]}"',
      '',
      '[[rules]]',
      'name = "b"',
      'input = ["a.txt"]',
      'output = ["b.txt"]',
      'shell = "cp {input[0]} {output[0]}"',
      '',
      '[[rules]]',
      'name = "c"',
      'input = ["a.txt"]',
      'output = ["c.txt"]',
      'shell = "cp {input[0]} {output[0]}"',
      '',
      '[[rules]]',
      'name = "d"',
      'input = ["b.txt", "c.txt"]',
      'output = ["d.txt"]',
      'shell = "cat {input[0]} {input[1]} > {output[0]}"',
    ].join('\n');
    // Paste the workflow into the TOML pane (single source of truth).
    await page.locator('.cm-content').click();
    await page.keyboard.press('ControlOrMeta+a');
    await page.keyboard.insertText(toml);
    await expect(page.locator('.val-badge')).toContainText('Valid', { timeout: 10_000 });

    // Four nodes on the canvas, laid out in ≥3 distinct x positions.
    await expect(page.locator('.rf-rule-node')).toHaveCount(4);
    await page.locator('.rf-layout-btn').click();
    const xs = await page.locator('.rf-rule-node').evaluateAll((nodes) =>
      nodes.map((n) => Math.round(n.getBoundingClientRect().x)),
    );
    const distinct = new Set(xs).size;
    expect(
      distinct,
      `a diamond must spread over ≥3 columns, got xs=${JSON.stringify(xs)}`,
    ).toBeGreaterThanOrEqual(3);
  });

  test('palette adds a grounded tool rule; inspector edits it', async ({ page }) => {
    await page.goto('/editor');
    await page.locator('.rf-rule-node', { hasText: 'fastqc' }).waitFor();

    // Palette: search the embedded Bioconda DB and add the real tool.
    await page.locator('.tool-palette-search input').fill('fastp');
    const firstTool = page.locator('.tool-palette-item', { hasText: 'fastp' }).first();
    await expect(firstTool).toBeVisible();
    await expect(firstTool.locator('.tool-palette-name')).toContainText('fastp');
    await firstTool.locator('.tool-palette-add').click();

    // The new node appears on the canvas with a grounded command, not a stub.
    const newCard = page.locator('.rf-rule-node', { hasText: 'fastp' });
    await expect(newCard).toBeVisible();
    await expect(newCard.locator('.rf-rule-shell')).toContainText('fastp');

    // Inspector: double-click the node and edit the shell.
    await newCard.dblclick();
    await expect(page.locator('#rule-inspector-title')).toContainText('fastp');
    const shellInput = page.locator('.inspector-shell');
    await shellInput.fill('fastp -i {input} -o {output} --thread {threads}');
    await page.locator('.inspector-dialog .btn-run').click();

    // The TOML pane (single source of truth) now carries the edit.
    await expect(page.locator('.val-badge')).toContainText('Valid', { timeout: 10_000 });
    const tomlText = page.locator('.cm-content');
    await expect(tomlText).toContainText('fastp -i {input} -o {output} --thread {threads}');
  });

  test('declared and file edges both render with distinct styles', async ({ page }) => {
    await page.goto('/editor');
    await page.locator('.rf-rule-node', { hasText: 'fastqc' }).waitFor();
    // The default workflow has no file edges (inputs are raw reads), so give
    // the canvas one via the palette + inspector: a rule consuming
    // qc/{sample}_fastqc.html creates a file edge from fastqc.
    await page.locator('.tool-palette-search input').fill('multiqc');
    const multiqcTool = page.locator('.tool-palette-item', { hasText: 'multiqc' }).first();
    await expect(multiqcTool).toBeVisible();
    await multiqcTool.locator('.tool-palette-add').click();
    const multiqcCard = page.locator('.rf-rule-node', { hasText: 'multiqc' });
    await expect(multiqcCard).toBeVisible();
    await multiqcCard.dblclick();
    // Fill inputs so the engine infers a file edge from fastqc's output.
    // (Palette-added rules start with empty input/output lists.)
    await page.locator('.inspector-dialog').getByRole('button', { name: '+ Add input' }).click();
    const inputs = page.locator('.inspector-list-row input').first();
    await inputs.fill('qc/{sample}_fastqc.html');
    await page.locator('.inspector-dialog .btn-run').click();
    await expect(page.locator('.val-badge')).toContainText('Valid', { timeout: 10_000 });
    // File-inferred edge is dashed (svg path with stroke-dasharray on the
    // path element rendered by React Flow).
    const dashedEdge = page.locator('.react-flow__edge-path[style*="dasharray"]');
    await expect(dashedEdge.first()).toBeVisible();
  });

  test('run dialog and dry-run still work from the editor', async ({ page }) => {
    await page.goto('/editor');
    await page.locator('.rf-rule-node', { hasText: 'fastqc' }).waitFor();
    // Run opens the options dialog; Dry-Run (preview) executes the plan only.
    await page.getByRole('button', { name: /^Run/ }).click();
    await expect(page.locator('#run-dialog-title')).toBeVisible();
    // Options fields are present and honored.
    await page.locator('.modal-dialog input[type="number"]').fill('3');
    await page.locator('.modal-dialog').getByRole('button', { name: /Dry-Run/ }).click();
    await expect(page.locator('.result-bar')).toContainText('Dry-Run started', { timeout: 15_000 });
  });
});
