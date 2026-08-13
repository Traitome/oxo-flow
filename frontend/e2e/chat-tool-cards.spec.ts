import { test, expect } from '@playwright/test';

// The backend contract (typed SSE from the real agent loop) is covered by
// crates/oxo-flow-web/tests/chat_agent_integration.rs with a ScriptedBackend.
// This spec verifies the FRONTEND renders tool-call cards and streamed text
// from that protocol — the SSE body is mocked at the network layer.

const SSE_BODY = [
  'event: status',
  'data: {"message":"planning"}',
  '',
  'event: tool_call',
  'data: {"name":"lookup_tool","args":"{\\"query\\":\\"fastp\\"}"}',
  '',
  'event: tool_result',
  'data: {"name":"lookup_tool","summary":"fastp 1.3.2 — A ultra-fast FASTQ preprocessor"}',
  '',
  'event: text',
  'data: {"chunk":"I found fastp in Bioconda."}',
  '',
  'event: done',
  'data: {"session_id":"s1","rounds":1}',
  '',
].join('\n');

test('chat renders grounded tool-call cards', async ({ page }) => {
  await page.route('**/api/chat/send', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'text/event-stream',
      body: SSE_BODY,
    }),
  );

  await page.goto('/chat');
  const input = page.locator('textarea.intent-input');
  await input.fill('What is fastp?');
  await page.locator('button[aria-label="Send message"]').click();

  // The tool card shows the grounded lookup with its summary.
  const card = page.locator('.chat-tool-card');
  await expect(card).toBeVisible({ timeout: 10_000 });
  await expect(card.locator('.chat-tool-name')).toHaveText('lookup_tool');
  await card.click(); // expand <details>
  await expect(card.locator('.chat-tool-summary')).toContainText('fastp 1.3.2');
  // The streamed text landed in the assistant bubble.
  await expect(page.locator('.chat-container')).toContainText('I found fastp in Bioconda.', { timeout: 10_000 });
});
