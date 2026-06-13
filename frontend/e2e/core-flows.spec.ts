import { test, expect } from '@playwright/test';

test.describe('oxo-flow v0.9 Core User Flows', () => {

  test('Dashboard loads with AI Companion Chat', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('h1')).toContainText('Command Center');
    // Chat UI should be visible
    await expect(page.locator('text=AI Companion')).toBeVisible();
    await expect(page.locator('text=Describe your analysis')).toBeVisible();
  });

  test('Chat input works and sends message', async ({ page }) => {
    await page.goto('/');
    const input = page.locator('.intent-input');
    await input.fill('RNA-seq differential expression');
    const sendBtn = page.locator('button:has(svg.lucide-send)');
    await sendBtn.click();
    // User message should appear
    await expect(page.locator('text=RNA-seq differential expression')).toBeVisible();
  });

  test('Pipeline Editor renders with DAG view', async ({ page }) => {
    await page.goto('/editor');
    await expect(page.locator('text=Pipeline Editor')).toBeVisible();
    await expect(page.locator('text=Generate')).toBeVisible();
    // TOML editor should be visible
    const textarea = page.locator('.toml-editor');
    await expect(textarea).toBeVisible();
    // Default TOML should be loaded
    const content = await textarea.inputValue();
    expect(content).toContain('[workflow]');
  });

  test('Pipeline validation shows status', async ({ page }) => {
    await page.goto('/editor');
    // Wait for DAG validation
    await page.waitForTimeout(2000);
    // Validation badge should appear
    const badge = page.locator('.val-badge');
    await expect(badge).toBeVisible({ timeout: 10000 });
  });

  test('Settings page loads with AI config', async ({ page }) => {
    await page.goto('/settings');
    await expect(page.locator('h1')).toContainText('Settings');
    await expect(page.locator('text=AI Provider Configuration')).toBeVisible();
    await expect(page.locator('text=Reference Genomes')).toBeVisible();
    await expect(page.locator('text=Computing Environments')).toBeVisible();
  });

  test('API Docs page loads', async ({ page }) => {
    await page.goto('/docs');
    await expect(page.locator('text=API Documentation')).toBeVisible();
  });

  test('Monitor page loads with run history', async ({ page }) => {
    await page.goto('/monitor');
    await expect(page.locator('h1')).toContainText('Monitor');
    // Run history table should be present
    await expect(page.locator('text=Run History')).toBeVisible();
  });

  test('Pipelines page loads', async ({ page }) => {
    await page.goto('/pipelines');
    await expect(page.locator('text=Pipelines')).toBeVisible();
  });

  test('Runs page loads with run history', async ({ page }) => {
    await page.goto('/runs');
    await expect(page.locator('h1')).toContainText('Runs');
    await expect(page.locator('text=Run History')).toBeVisible();
  });

  test('DAG view renders nodes in editor', async ({ page }) => {
    await page.goto('/editor');
    // Wait for DAG visualization to render
    await page.waitForTimeout(3000);
    const dagContainer = page.locator('.dag-container');
    await expect(dagContainer).toBeVisible({ timeout: 10000 });
  });

  test('Layout navigation works', async ({ page }) => {
    await page.goto('/');
    // Click on Editor link in sidebar
    await page.click('a[href="/editor"]');
    await expect(page.locator('text=Pipeline Editor')).toBeVisible();
    // Click on Dashboard link
    await page.click('a[href="/"]');
    await expect(page.locator('h1')).toContainText('Command Center');
  });
});
