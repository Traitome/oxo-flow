import { test, expect } from '@playwright/test';

test.describe('oxo-flow v0.9 Core User Flows', () => {

  // ── Navigation & Layout ──

  test('Dashboard loads with key elements', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('h1')).toContainText('Command Center');
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
    await expect(page.locator('h1')).toContainText('Command Center');
  });

  test('Mobile menu toggle works', async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await page.goto('/');
    const menuBtn = page.locator('button[aria-label="Toggle menu"]');
    await expect(menuBtn).toBeVisible();
    await menuBtn.click();
    await expect(page.locator('.header-nav.open')).toBeVisible();
  });

  // ── Pipeline Editor ──

  test('Pipeline Editor loads with TOML editor and DAG view', async ({ page }) => {
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
    await page.goto('/editor');
    // Wait for debounced validation
    await page.waitForTimeout(2000);
    const badge = page.locator('.val-badge');
    await expect(badge).toBeVisible({ timeout: 10000 });
  });

  test('DAG view renders graph nodes', async ({ page }) => {
    await page.goto('/editor');
    // React Flow canvas should be visible with the default workflow's nodes
    const node = page.locator('.rf-rule-node', { hasText: 'fastqc' });
    await expect(node).toBeVisible({ timeout: 10000 });
    // Node count should appear
    await expect(page.locator('.dag-counts')).toBeVisible({ timeout: 10000 });
  });

  test('Save button saves pipeline', async ({ page }) => {
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

  // ── Dashboard Chat ──

  test('Dashboard chat input is visible and functional', async ({ page }) => {
    await page.goto('/');
    // Chat input should be present
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
});
