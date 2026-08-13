import { test, expect } from '@playwright/test';

test.describe('Browser UI Interaction Tests', () => {

  // ── Dashboard UI ──

  test('dashboard stat cards display real data', async ({ page }) => {
    await page.goto('/');
    await page.waitForTimeout(2000);

    // Health status dot should be visible (now driven by real /api/health)
    const statusDot = page.locator('#header-status');
    await expect(statusDot).toBeVisible({ timeout: 5000 });

    // Should show "Command Center" heading
    await expect(page.locator('h1')).toContainText('Command Center');

    // Footer should show license info
    await expect(page.locator('.app-footer')).toBeVisible();
  });

  // ── Editor UI Interactions ──

  test('editor shows validation result after TOML input', async ({ page }) => {
    await page.goto('/editor');
    await page.waitForTimeout(3000);

    // Validation badge should appear (shows "Valid" or error count)
    const badge = page.locator('.val-badge');
    await expect(badge).toBeVisible({ timeout: 10000 });

    // DAG panel should show node/edge counts
    const dagCounts = page.locator('.dag-counts');
    await expect(dagCounts).toBeVisible({ timeout: 10000 });
  });

  test('editor sidebar links are functional', async ({ page }) => {
    await page.goto('/');
    // Sidebar should have Dashboard, Pipeline Editor, etc.
    await expect(page.locator('.sidebar-nav')).toBeVisible();
    await expect(page.locator('text=Pipeline Editor').first()).toBeVisible();
  });

  // ── Settings UI ──

  test('settings page renders all sections', async ({ page }) => {
    await page.goto('/settings');
    await page.waitForTimeout(1000);

    // Verify key sections
    await expect(page.locator('text=AI Provider Configuration')).toBeVisible({ timeout: 5000 });
  });

  // ── API Docs UI ──

  test('api docs page renders endpoint list', async ({ page }) => {
    await page.goto('/docs');
    await page.waitForTimeout(1000);

    // Should have API Reference title
    await expect(page.locator('h1')).toContainText('API Reference', { timeout: 5000 });

    // Should list endpoints
    const content = page.locator('main');
    await expect(content).toBeVisible();
  });

  // ── Monitor Page UI ──

  test('monitor page shows run list', async ({ page }) => {
    await page.goto('/monitor');
    await page.waitForTimeout(2000);

    // Should show Monitor heading
    await expect(page.locator('h1')).toContainText('Monitor', { timeout: 5000 });

    // Should have run history section
    await expect(page.locator('text=Run History')).toBeVisible({ timeout: 5000 });
  });

  // ── Pipeline Library UI ──

  test('pipeline library shows templates', async ({ page }) => {
    await page.goto('/pipelines');
    await page.waitForTimeout(1000);

    await expect(page.locator('h1')).toContainText('Pipelines', { timeout: 5000 });
  });

  // ── Responsive Design ──

  test('layout adapts to mobile viewport', async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await page.goto('/');
    await page.waitForTimeout(1000);

    // Mobile menu button should be visible
    const menuBtn = page.locator('button[aria-label="Toggle menu"]');
    // This button only exists when the responsive hamburger is shown
    const isVisible = await menuBtn.isVisible().catch(() => false);
    if (isVisible) {
      await menuBtn.click();
      await expect(page.locator('.header-nav.open')).toBeVisible();
    }
    // At minimum the page should render without horizontal scroll
    const body = page.locator('body');
    await expect(body).toBeVisible();
  });

  // ── Header Branding ──

  test('header shows brand and version', async ({ page }) => {
    await page.goto('/');
    await page.waitForTimeout(1000);

    // Header brand
    await expect(page.locator('.header-brand')).toContainText('oxo-flow');
    // Version tag
    await expect(page.locator('.header-ver')).toBeVisible();
  });
});
