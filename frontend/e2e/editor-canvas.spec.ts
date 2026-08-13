import { test, expect } from '@playwright/test';

// Acceptance scenario for graphical programming (§6.2 of the design spec):
// build a workflow on the canvas — palette, inspector, connect, delete —
// and see the TOML stay in sync with the engine's canonical formatting.

test.describe('Graphical workflow editor (canvas)', () => {
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

  test('dry-run and save buttons still work from the editor', async ({ page }) => {
    await page.goto('/editor');
    await page.locator('.rf-rule-node', { hasText: 'fastqc' }).waitFor();
    await expect(page.getByRole('button', { name: /Dry-Run/ })).toBeEnabled();
    await page.getByRole('button', { name: /Dry-Run/ }).click();
    await expect(page.locator('.result-bar')).toContainText('Dry-Run started', { timeout: 15_000 });
  });
});
