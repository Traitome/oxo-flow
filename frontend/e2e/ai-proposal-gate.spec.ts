import { test, expect, type Page } from '@playwright/test';

// Audit UX-05 (issue #1147): AI-generated workflow output used to be written
// straight into editor state by `onPipelineReady`, so a chat round silently
// overwrote unsaved work. The generated TOML is now staged as a pending
// proposal: a diff against the CURRENT editor content is shown, and the
// editor changes only when the user clicks Apply.
//
// The SSE stream is mocked at the network layer (same approach as
// chat-tool-cards.spec.ts); the real backend contract is covered by the Rust
// suites. These tests drive the real built SPA.

const V1_TOML = [
  '[workflow]',
  'name = "ai-generated-v1"',
  'version = "1.0.0"',
  '',
  '[[rules]]',
  'name = "ai_align_v1"',
  'input = ["{sample}.fastq.gz"]',
  'output = ["bam/{sample}.bam"]',
  'shell = "bwa mem ref/genome.fa {input} > {output}"',
  'threads = 8',
].join('\n');

const V2_TOML = [
  '[workflow]',
  'name = "ai-generated-v2"',
  'version = "1.0.0"',
  '',
  '[[rules]]',
  'name = "ai_align_v2"',
  'input = ["{sample}.fastq.gz"]',
  'output = ["bam/{sample}.bam"]',
  'shell = "bwa-mem2 mem ref/genome.fa {input} > {output}"',
  'threads = 16',
].join('\n');

const UNSAVED_TOML = [
  '[workflow]',
  'name = "my-unsaved-work"',
  '',
  '[[rules]]',
  'name = "unsaved_marker_rule"',
  'shell = "echo unsaved work"',
].join('\n');

function pipelineReadySSE(toml: string): string {
  return [
    'event: action',
    `data: ${JSON.stringify({ action_type: 'pipeline_ready', data: { toml_content: toml, validation: { valid: true } } })}`,
    '',
    'event: done',
    'data: {"session_id":"s1","rounds":1}',
    '',
  ].join('\n');
}

/** Mock the AI provider as configured and the chat stream as a scripted
 *  sequence of `pipeline_ready` payloads (one per send). */
async function mockAi(page: Page, payloads: string[]) {
  let call = 0;
  await page.route('**/api/ai/config', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ provider: 'deepseek', model: 'deepseek-chat', api_url: '', is_configured: true }),
    }),
  );
  await page.route('**/api/chat/send', (route) => {
    const body = payloads[Math.min(call, payloads.length - 1)];
    call++;
    return route.fulfill({ status: 200, contentType: 'text/event-stream', body });
  });
}

/** Exact editor text: CodeMirror renders one `.cm-line` element per document
 *  line (line wrapping is CSS-internal), so joining their textContent is the
 *  document as the app holds it — innerText normalizes blank lines. */
async function editorText(page: Page): Promise<string> {
  return page.locator('.cm-content').evaluate((el) =>
    Array.from(el.querySelectorAll('.cm-line'))
      .map((line) => line.textContent ?? '')
      .join('\n'),
  );
}

async function openAssistant(page: Page) {
  await page.getByRole('tab', { name: /Assistant/ }).click();
  await expect(page.locator('textarea.intent-input')).toBeVisible();
}

async function sendChat(page: Page, prompt: string) {
  await page.locator('textarea.intent-input').fill(prompt);
  const send = page.locator('button[aria-label="Send message"]');
  await expect(send).toBeEnabled();
  await send.click();
}

test.describe('AI proposal acceptance gate (UX-05)', () => {
  test.beforeEach(async ({ page }) => {
    // The editor defaults to Guided mode; these tests exercise the
    // canvas + TOML view where the assistant rail lives.
    await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
  });

  test('generated output does not mutate the editor until Apply', async ({ page }) => {
    await mockAi(page, [pipelineReadySSE(V1_TOML)]);
    await page.goto('/editor');
    await expect(page.locator('.cm-content')).toContainText('fastqc');
    const before = await editorText(page);

    await openAssistant(page);
    await sendChat(page, 'Build an alignment pipeline');

    const proposal = page.getByTestId('ai-proposal');
    await expect(proposal).toBeVisible({ timeout: 15_000 });
    // The diff shows the proposed rule...
    await expect(page.getByTestId('ai-proposal-diff')).toContainText('ai_align_v1');
    await expect(page.getByTestId('ai-proposal-diff').locator('[data-kind="added"]')).not.toHaveCount(0);
    // ...while the editor state, the TOML buffer, and the DAG built from it
    // all still hold the original workflow.
    expect(await editorText(page)).toBe(before);
    await expect(page.locator('.cm-content')).not.toContainText('ai_align_v1');
    await expect(page.locator('.rf-rule-node', { hasText: 'ai_align_v1' })).toHaveCount(0);

    await page.getByTestId('ai-proposal-accept').click();
    await expect(proposal).toHaveCount(0);
    // Accept applies exactly the proposed document.
    await expect.poll(() => editorText(page)).toBe(V1_TOML);
    await expect(page.locator('.cm-content')).not.toContainText('fastqc');
    // The canvas rebuilt from the accepted content.
    await expect(page.locator('.rf-rule-node', { hasText: 'ai_align_v1' })).toHaveCount(1, {
      timeout: 10_000,
    });
  });

  test('Discard leaves the editor byte-identical', async ({ page }) => {
    await mockAi(page, [pipelineReadySSE(V1_TOML)]);
    await page.goto('/editor');
    await expect(page.locator('.cm-content')).toContainText('fastqc');
    const before = await editorText(page);

    await openAssistant(page);
    await sendChat(page, 'Build an alignment pipeline');
    await expect(page.getByTestId('ai-proposal')).toBeVisible({ timeout: 15_000 });

    await page.getByTestId('ai-proposal-discard').click();
    await expect(page.getByTestId('ai-proposal')).toHaveCount(0);
    expect(await editorText(page)).toBe(before);
    await expect(page.locator('.cm-content')).not.toContainText('ai_align_v1');
  });

  test('a pending proposal never overwrites unsaved edits without acceptance', async ({ page }) => {
    await mockAi(page, [pipelineReadySSE(V1_TOML)]);
    await page.goto('/editor');
    await expect(page.locator('.cm-content')).toBeVisible();

    // Unsaved, hand-written work that exists only in the editor.
    await page.locator('.cm-content').click();
    await page.keyboard.press('ControlOrMeta+a');
    await page.keyboard.insertText(UNSAVED_TOML);
    await expect(page.locator('.cm-content')).toContainText('unsaved_marker_rule');
    const before = await editorText(page);
    expect(before).toBe(UNSAVED_TOML);

    await openAssistant(page);
    await sendChat(page, 'Build an alignment pipeline');
    await expect(page.getByTestId('ai-proposal')).toBeVisible({ timeout: 15_000 });

    // The unsaved work is still exactly what the editor holds.
    expect(await editorText(page)).toBe(UNSAVED_TOML);
    await expect(page.locator('.cm-content')).toContainText('unsaved_marker_rule');
    await expect(page.locator('.cm-content')).not.toContainText('ai_align_v1');

    await page.getByTestId('ai-proposal-accept').click();
    await expect.poll(() => editorText(page)).toBe(V1_TOML);
    await expect(page.locator('.cm-content')).not.toContainText('unsaved_marker_rule');
  });

  test('a second proposal queues without silently replacing the pending one', async ({ page }) => {
    await mockAi(page, [pipelineReadySSE(V1_TOML), pipelineReadySSE(V2_TOML)]);
    await page.goto('/editor');
    await expect(page.locator('.cm-content')).toContainText('fastqc');
    const before = await editorText(page);

    await openAssistant(page);
    await sendChat(page, 'Build an alignment pipeline');
    const proposal = page.getByTestId('ai-proposal');
    const diff = page.getByTestId('ai-proposal-diff');
    await expect(proposal).toBeVisible({ timeout: 15_000 });
    await expect(diff).toContainText('ai_align_v1');

    // A second generation completes while the first is still pending.
    await sendChat(page, 'Try a different aligner');
    await expect(page.getByTestId('ai-proposal-count')).toContainText('1 of 2', { timeout: 15_000 });

    // The displayed proposal was NOT swapped, and even the newest output did
    // not touch the editor.
    await expect(diff).toContainText('ai_align_v1');
    await expect(diff).not.toContainText('ai_align_v2');
    expect(await editorText(page)).toBe(before);
    await expect(page.locator('.cm-content')).not.toContainText('ai_align_v1');
    await expect(page.locator('.cm-content')).not.toContainText('ai_align_v2');

    // Reviewing the queued proposal is explicit; accepting applies exactly it.
    await page.getByTestId('ai-proposal-next').click();
    await expect(diff).toContainText('ai_align_v2');
    await page.getByTestId('ai-proposal-accept').click();
    await expect.poll(() => editorText(page)).toBe(V2_TOML);
    await expect(page.locator('.cm-content')).not.toContainText('ai_align_v1');
  });

  test('Apply keeps the remaining queue and the next proposal is still reviewable', async ({ page }) => {
    await mockAi(page, [pipelineReadySSE(V1_TOML), pipelineReadySSE(V2_TOML)]);
    await page.goto('/editor');
    await expect(page.locator('.cm-content')).toContainText('fastqc');

    await openAssistant(page);
    await sendChat(page, 'Build an alignment pipeline');
    const proposal = page.getByTestId('ai-proposal');
    await expect(proposal).toBeVisible({ timeout: 15_000 });
    await sendChat(page, 'Try a different aligner');
    await expect(page.getByTestId('ai-proposal-count')).toContainText('1 of 2', { timeout: 15_000 });

    await page.getByTestId('ai-proposal-accept').click();
    // Apply consumes ONLY the accepted proposal. The queued one survives with
    // its stable ordinal and its own diff — it used to disappear silently.
    await expect.poll(() => editorText(page)).toBe(V1_TOML);
    await expect(proposal).toBeVisible();
    await expect(page.getByTestId('ai-proposal-count')).toContainText('2 of 2');
    await expect(page.getByTestId('ai-proposal-diff')).toContainText('ai_align_v2');
    expect(await editorText(page)).toBe(V1_TOML);

    // The surviving proposal can then be applied.
    await page.getByTestId('ai-proposal-accept').click();
    await expect.poll(() => editorText(page)).toBe(V2_TOML);
    await expect(proposal).toHaveCount(0);
  });

  test('Discard in a queued pair removes only the active proposal', async ({ page }) => {
    await mockAi(page, [pipelineReadySSE(V1_TOML), pipelineReadySSE(V2_TOML)]);
    await page.goto('/editor');
    await expect(page.locator('.cm-content')).toContainText('fastqc');
    const before = await editorText(page);

    await openAssistant(page);
    await sendChat(page, 'Build an alignment pipeline');
    await expect(page.getByTestId('ai-proposal')).toBeVisible({ timeout: 15_000 });
    await sendChat(page, 'Try a different aligner');
    await expect(page.getByTestId('ai-proposal-count')).toContainText('1 of 2', { timeout: 15_000 });

    await page.getByTestId('ai-proposal-discard').click();
    // Only the discarded proposal leaves the queue; the editor never changed.
    await expect(page.getByTestId('ai-proposal')).toBeVisible();
    await expect(page.getByTestId('ai-proposal-count')).toContainText('2 of 2');
    await expect(page.getByTestId('ai-proposal-diff')).toContainText('ai_align_v2');
    expect(await editorText(page)).toBe(before);

    await page.getByTestId('ai-proposal-discard').click();
    await expect(page.getByTestId('ai-proposal')).toHaveCount(0);
    expect(await editorText(page)).toBe(before);
  });

  test('a slow format response cannot overwrite an accepted proposal', async ({ page }) => {
    await mockAi(page, [pipelineReadySSE(V1_TOML)]);
    // The format response is delayed and would clobber the editor with stale
    // formatted text if it were not guarded by the edit ticket.
    await page.route('**/api/pipelines/format', async (route) => {
      const body = route.request().postDataJSON() as { toml_content?: string };
      await new Promise((resolve) => setTimeout(resolve, 1500));
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ formatted: `# STALE_FORMAT_MARKER\n${body.toml_content ?? ''}` }),
      });
    });
    await page.goto('/editor');
    await expect(page.locator('.cm-content')).toContainText('fastqc');

    await openAssistant(page);
    await sendChat(page, 'Build an alignment pipeline');
    await expect(page.getByTestId('ai-proposal')).toBeVisible({ timeout: 15_000 });

    // Format is dispatched first; Apply lands while its response is in flight.
    const formatResponse = page.waitForResponse('**/api/pipelines/format');
    await page.locator('button[title="Format"]').click();
    await page.getByTestId('ai-proposal-accept').click();
    await expect.poll(() => editorText(page)).toBe(V1_TOML);

    // The stale formatted text arrives afterwards and is discarded.
    await formatResponse;
    await page.waitForTimeout(300);
    expect(await editorText(page)).toBe(V1_TOML);
    await expect(page.locator('.cm-content')).not.toContainText('STALE_FORMAT_MARKER');
  });

  test('navigating away does not auto-accept a pending proposal', async ({ page }) => {
    await mockAi(page, [pipelineReadySSE(V1_TOML)]);
    await page.goto('/editor');
    await openAssistant(page);
    await sendChat(page, 'Build an alignment pipeline');
    await expect(page.getByTestId('ai-proposal')).toBeVisible({ timeout: 15_000 });

    // Client-side navigation keeps the SPA session (the editor restores its
    // in-memory pipeline TOML). The pending proposal must not be applied.
    await page.getByRole('link', { name: 'Runs' }).click();
    await expect(page).toHaveURL(/\/runs/);
    await page.getByRole('link', { name: 'Pipeline Editor' }).click();
    await expect(page.locator('.cm-content')).toContainText('fastqc', { timeout: 10_000 });
    await expect(page.locator('.cm-content')).not.toContainText('ai_align_v1');
  });
});
