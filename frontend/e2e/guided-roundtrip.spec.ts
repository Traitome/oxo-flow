import { test, expect, type Page } from '@playwright/test';

// Issue #1143 (audit UX-01): guided mode must never write a lossy document.
// The guided rule cards model only [workflow] name/version plus per-rule
// name/shell/input/output/threads/memory/environment. A document carrying
// anything else ([[values]], [engine], when, depends_on, [rules.*] …) is
// refused: the form is replaced by a refusal naming the constructs, the
// guided Save path rejects without calling the API, and the TOML is left
// untouched. A basic workflow still saves through guided mode.

const ADVANCED_TOML = [
  '[workflow]',
  'name = "advanced-semantics"',
  'version = "1.0.0"',
  '',
  '[config]',
  'align = true',
  '',
  '[[values]]',
  'name = "assembler"',
  'values = ["spades", "megahit"]',
  '',
  '[engine]',
  'min_free_disk = "10G"',
  '',
  '[[rules]]',
  'name = "align"',
  'input = ["{sample}.fq"]',
  'output = ["bam/{sample}.bam"]',
  'shell = "aligner {input} > {output}"',
  'when = "config.align"',
  'threads = 4',
  '',
  '[[rules]]',
  'name = "call_variants"',
  'input = ["bam/{sample}.bam"]',
  'output = ["vcf/{sample}.vcf"]',
  'shell = "caller {input} > {output}"',
  'depends_on = ["align"]',
].join('\n');

async function pasteToml(page: Page, toml: string) {
  await page.locator('.cm-content').click();
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.insertText(toml);
  await expect(page.locator('.val-badge')).toContainText('Valid', { timeout: 10_000 });
}

test.describe('Guided mode refuses lossy saves (#1143)', () => {
  test('advanced semantics: refusal names the loss, save makes no API call, TOML unchanged', async ({ page }) => {
    // Enter the raw TOML editor first so the fixture can be loaded, then
    // switch to guided mode exactly as a user would.
    await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
    await page.goto('/editor');
    await pasteToml(page, ADVANCED_TOML);

    await page.getByRole('button', { name: 'Guided', exact: true }).click();

    const refusal = page.getByTestId('guided-refusal');
    await expect(refusal).toBeVisible({ timeout: 10_000 });
    // The message is specific: every unmodeled construct is named.
    await expect(refusal).toContainText('[[values]]');
    await expect(refusal).toContainText('[engine]');
    await expect(refusal).toContainText('rule "align" → when');
    await expect(refusal).toContainText('rule "call_variants" → depends_on');
    // No editable form is rendered at all.
    await expect(page.locator('.guided-meta')).toHaveCount(0);
    await expect(page.getByLabel('Rule 1 name')).toHaveCount(0);

    // The guided Save path must refuse before any persistence call.
    const pipelineCreates: string[] = [];
    page.on('request', (req) => {
      if (req.method() === 'POST' && new URL(req.url()).pathname === '/api/pipelines') {
        pipelineCreates.push(req.postData() ?? '');
      }
    });
    await page.getByTestId('guided-save').click();
    const errorBar = page.locator('.result-bar.error');
    await expect(errorBar).toBeVisible();
    await expect(errorBar).toContainText('Guided save refused');
    await expect(errorBar).toContainText('[engine]');
    expect(pipelineCreates, 'a refused guided save must not POST a pipeline').toEqual([]);

    // The switch affordance lands in the TOML editor with the document intact.
    await page.getByTestId('guided-switch-toml').click();
    const tomlPane = page.locator('.cm-content');
    await expect(tomlPane).toBeVisible();
    await expect(tomlPane).toContainText('name = "advanced-semantics"');
    await expect(tomlPane).toContainText('[[values]]');
    await expect(tomlPane).toContainText('[engine]');
    await expect(tomlPane).toContainText('when = "config.align"');
    await expect(tomlPane).toContainText('depends_on = ["align"]');
    await expect(tomlPane).toContainText('name = "call_variants"');
  });

  test('spaced [[ rules ]] / [ engine ] headers cannot bypass the refusal', async ({ page }) => {
    // Review finding (2026-11-18): the old header lexer only accepted
    // `[[rules]]` with no inner whitespace, so `[[ rules ]]` was swallowed
    // into the [workflow] span and the detector returned [] — a card edit
    // then appended a duplicate rule. TOML-legal header spellings must be
    // read identically and still trigger the refusal.
    const spacedToml = [
      '[workflow]',
      'name = "spaced-headers"',
      '',
      '[[ rules ]]',
      'name = "align"',
      'input = ["{sample}.fq"]',
      'output = ["bam/{sample}.bam"]',
      'shell = "aligner {input} > {output}"',
      'when = "config.align"',
      'threads = 4',
      '',
      '[ engine ]',
      'min_free_disk = "10G"',
      '',
      '[config]',
      'align = true',
    ].join('\n');

    await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
    await page.goto('/editor');
    await pasteToml(page, spacedToml);

    await page.getByRole('button', { name: 'Guided', exact: true }).click();
    const refusal = page.getByTestId('guided-refusal');
    await expect(refusal).toBeVisible({ timeout: 10_000 });
    await expect(refusal).toContainText('rule "align" → when');
    await expect(refusal).toContainText('[engine]');
    await expect(page.getByLabel('Rule 1 name')).toHaveCount(0);

    await page.getByTestId('guided-switch-toml').click();
    const tomlPane = page.locator('.cm-content');
    await expect(tomlPane).toBeVisible();
    await expect(tomlPane).toContainText('[[ rules ]]');
    await expect(tomlPane).toContainText('when = "config.align"');
    await expect(tomlPane).toContainText('[ engine ]');
  });

  test('basic workflow still saves through guided mode', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'guided'));
    await page.goto('/editor');

    // Guided is the form's home; the default (basic) workflow imports into
    // editable cards with no refusal.
    const nameInput = page.locator('.guided-meta input').first();
    await expect(nameInput).toBeVisible({ timeout: 10_000 });
    await expect(page.getByLabel('Rule 1 name')).toHaveValue('fastqc');
    await expect(page.getByTestId('guided-refusal')).toHaveCount(0);

    await nameInput.fill('basic-guided-save');

    let created: { name?: string; toml_content?: string } | null = null;
    page.on('request', (req) => {
      if (req.method() === 'POST' && new URL(req.url()).pathname === '/api/pipelines') {
        created = req.postDataJSON() as { name?: string; toml_content?: string };
      }
    });

    await page.getByTestId('guided-save').click();
    await expect(page.locator('.result-bar.success')).toContainText('Pipeline "basic-guided-save" created', {
      timeout: 15_000,
    });
    expect(created, 'guided save posts the pipeline').not.toBeNull();
    const payload = created as { name?: string; toml_content?: string };
    expect(payload.name).toBe('basic-guided-save');
    expect(payload.toml_content).toContain('name = "fastqc"');
    expect(payload.toml_content).toContain('threads = 2');
    expect(payload.toml_content).toContain('name = "align"');
  });
});
