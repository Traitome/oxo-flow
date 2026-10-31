import { test, expect } from '@playwright/test';

test.describe('oxo-flow v0.9 Core User Flows', () => {

  // ── Navigation & Layout ──

  test('Dashboard loads with key elements', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('h1')).toContainText('What do you want to do?');
    // Sidebar navigation should be visible (use .first() to avoid strict mode on duplicates)
    await expect(page.locator('text=Dashboard').first()).toBeVisible();
    await expect(page.locator('text=Pipeline Editor').first()).toBeVisible();
    await expect(page.locator('text=Runs').first()).toBeVisible();
  });

  test('Navigation between pages works', async ({ page }) => {
    await page.goto('/');
    // Navigate to Pipeline Editor
    await page.click('a[href="/editor"]');
    await expect(page).toHaveURL(/\/editor/);
    await expect(page.locator('h1')).toContainText('Pipeline Editor');
    // Navigate to Settings
    await page.click('a[href="/settings"]');
    await expect(page).toHaveURL(/\/settings/);
    await expect(page.locator('h1')).toContainText('Settings');
    // Navigate back to Dashboard
    await page.click('a[href="/"]');
    await expect(page.locator('h1')).toContainText('What do you want to do?');
  });

  test('Mobile menu toggle works', async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await page.goto('/');
    // Issue #947: <1024px the sidebar becomes a drawer driven by the header toggle.
    const menuBtn = page.locator('button[aria-label="Open navigation menu"]');
    await expect(menuBtn).toBeVisible();
    await menuBtn.click();
    await expect(page.locator('.sidebar.drawer')).toBeVisible();
  });

  // ── Pipeline Editor ──

  test('Pipeline Editor loads with TOML editor and DAG view', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
    await page.goto('/editor');
    await expect(page.locator('h1')).toContainText('Pipeline Editor');
    // CodeMirror TOML editor should be present
    await expect(page.locator('.cm-editor')).toBeVisible({ timeout: 10000 });
    // Default TOML should load — check for content in the editor
    await page.waitForTimeout(1500);
    const cmContent = page.locator('.cm-content');
    await expect(cmContent).toBeVisible();
    const text = await cmContent.textContent();
    expect(text).toContain('my-pipeline');
  });

  test('Pipeline validation badge appears', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
    await page.goto('/editor');
    // Wait for debounced validation
    await page.waitForTimeout(2000);
    const badge = page.locator('.val-badge');
    await expect(badge).toBeVisible({ timeout: 10000 });
  });

  test('DAG view renders graph nodes', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
    await page.goto('/editor');
    // React Flow canvas should be visible with the default workflow's nodes
    const node = page.locator('.rf-rule-node', { hasText: 'fastqc' });
    await expect(node).toBeVisible({ timeout: 10000 });
    // Node count should appear
    await expect(page.locator('.dag-counts')).toBeVisible({ timeout: 10000 });
  });

  test('Save button saves pipeline', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
    await page.goto('/editor');
    await page.waitForTimeout(2000);
    const saveBtn = page.locator('button:has-text("Save")');
    await expect(saveBtn).toBeVisible();
    await saveBtn.click();
    // Should show success or error notification (not crash)
    await page.waitForTimeout(1000);
    const resultBar = page.locator('.result-bar');
    await expect(resultBar).toBeVisible({ timeout: 5000 });
  });

  // ── AI Chat ──

  test('Dashboard AI entry card navigates to the chat', async ({ page }) => {
    await page.route('**/api/ai/config', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ provider: 'deepseek', model: 'deepseek-chat', api_url: '', is_configured: true }),
      }),
    );
    await page.goto('/');
    // The task-oriented home offers an AI card; the chat input lives on
    // the dedicated /chat page (issue #82 P1-7).
    const aiCard = page.locator('.entry-card').filter({ hasText: 'Generate with AI' });
    await expect(aiCard.first()).toBeVisible({ timeout: 5000 });
    await aiCard.first().click();
    await expect(page).toHaveURL(/\/chat/);
    const chatInput = page.locator('.intent-input, textarea[placeholder*="Describe"]');
    await expect(chatInput.first()).toBeVisible({ timeout: 5000 });
  });

  // ── Settings ──

  test('Settings page loads with AI config', async ({ page }) => {
    await page.goto('/settings');
    await expect(page.locator('h1')).toContainText('Settings');
    await expect(page.locator('text=AI Provider Configuration')).toBeVisible();
  });

  test('Settings AI provider inputs work without focus loss', async ({ page }) => {
    await page.goto('/settings');
    await page.waitForTimeout(1000);
    const apiKeyInput = page.locator('input[placeholder="sk-..."]');
    await expect(apiKeyInput).toBeVisible();
    // Type 5 characters and verify they all appear (no focus loss bug)
    await apiKeyInput.fill('testkey12345');
    await page.waitForTimeout(200);
    const value = await apiKeyInput.inputValue();
    expect(value).toBe('testkey12345');
  });

  // ── API Docs ──

  test('API Docs page loads', async ({ page }) => {
    await page.goto('/docs');
    await expect(page.locator('h1')).toContainText('API Reference');
  });

  // ── Monitor ──

  test('Monitor page loads with run history', async ({ page }) => {
    await page.goto('/monitor');
    await expect(page.locator('h1')).toContainText('Monitor');
    await expect(page.locator('text=Run History')).toBeVisible({ timeout: 5000 });
  });

  // ── Pipelines ──

  test('Pipelines page loads', async ({ page }) => {
    await page.goto('/pipelines');
    await expect(page.locator('h1')).toContainText('Pipelines');
  });

  // ── Runs ──

  test('Runs page loads', async ({ page }) => {
    await page.goto('/runs');
    await expect(page.locator('h1')).toContainText('Monitor & Reports');
    await expect(page.locator('text=Run History')).toBeVisible({ timeout: 5000 });
  });

  // ── AI Chat page ──

  test('AI Chat standalone page loads', async ({ page }) => {
    await page.goto('/chat');
    // Chat page renders without a title h1 — just verify the page loads
    await expect(page.locator('main')).toBeVisible();
  });

  // ── API health check ──

  test('API health endpoint responds', async ({ request }) => {
    const resp = await request.get('/api/health');
    expect(resp.ok()).toBeTruthy();
    const body = await resp.json();
    expect(body).toHaveProperty('status');
    expect(['ok', 'healthy', 'degraded']).toContain(body.status);
  });

  test('API system info responds', async ({ request }) => {
    const resp = await request.get('/api/system');
    expect(resp.ok()).toBeTruthy();
    const body = await resp.json();
    // System info returns os, arch, version, pid, uptime_secs
    expect(body).toHaveProperty('os');
    expect(body).toHaveProperty('arch');
  });

  test('API version is correct', async ({ request }) => {
    const resp = await request.get('/api/health');
    const body = await resp.json();
    expect(body).toHaveProperty('version');
    expect(typeof body.version).toBe('string');
  });

  test('Lifecycle workbench: 5-stage grid, Command Palette, Sample Sheet parser, Engine guard, Format & Lint', async ({ page }) => {
    await page.goto('/');
    // 5-stage lifecycle grid renders on Dashboard
    await expect(page.locator('.lifecycle-stage-card')).toHaveCount(5);

    // Command Palette opens via header trigger
    await page.locator('.cmd-trigger').click();
    await expect(page.locator('.cmd-palette-dialog')).toBeVisible();
    await page.keyboard.press('Escape');

    // Click Stage 1 (Data & Samples) -> opens Editor with Data Workbench modal
    await page.locator('.lifecycle-stage-card').first().click();
    await expect(page).toHaveURL(/\/editor\?modal=data/);
    await expect(page.getByRole('dialog', { name: /Data & Sample Sheet Workbench/i })).toBeVisible();
    await page.getByRole('button', { name: /Parse Sample Sheet/i }).click();
    await expect(page.getByRole('button', { name: /Insert Samples into Workflow/i })).toBeVisible();
    await page.getByRole('button', { name: /Insert Samples into Workflow/i }).click();
    await expect(page.locator('.result-bar.success')).toContainText('Injected 2 sample(s) into [[sample_groups]]');

    // Engine & Safety modal injects [engine] settings
    await page.getByRole('button', { name: /Engine & Safety/i }).click();
    await expect(page.getByRole('dialog', { name: /Engine & Data Safety/i })).toBeVisible();
    await expect(page.getByRole('dialog', { name: /Engine & Data Safety/i }).locator('pre.code-block')).toContainText('min_free_disk = "20G"');
    await page.getByRole('button', { name: /Inject into \[engine\]/i }).click();
    await expect(page.locator('.result-bar.success')).toContainText('Updated [engine]');

    // Format & Lint buttons work in Editor
    await page.getByRole('button', { name: /^Format$/i }).click();
    await expect(page.locator('.result-bar.success')).toContainText('Formatted TOML canonically');
  });
});
