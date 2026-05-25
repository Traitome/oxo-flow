import { test, expect } from '@playwright/test';

/**
 * Template library tests for oxo-flow multi-user web system.
 * Tests template browsing, preview, and loading functionality.
 */

test.describe('Template Library', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await page.click('.user-info .avatar');
    await page.click('button:text("Sign In")');
    await page.fill('#login-username', 'admin');
    await page.fill('#login-password', 'admin');
    await page.click('.modal button:text("Sign In")');
    await expect(page.locator('#user-name')).toHaveText('admin');
  });

  test('template library view shows templates', async ({ page }) => {
    await page.click('button[data-view="templates"]');
    await expect(page.locator('#view-templates')).toBeVisible();

    // Check that templates grid is populated
    await expect(page.locator('#templates-grid')).toBeVisible();
    await expect(page.locator('.template-card').first()).toBeVisible();
  });

  test('template search filters results', async ({ page }) => {
    await page.click('button[data-view="templates"]');
    await page.fill('#template-search', 'genomics');
    await expect(page.locator('.template-card').count()).toBeGreaterThan(0);

    // Clear search
    await page.fill('#template-search', '');
    await expect(page.locator('.template-card').count()).toBeGreaterThan(0);
  });

  test('template preview modal works', async ({ page }) => {
    await page.click('button[data-view="templates"]');
    await expect(page.locator('#templates-grid')).toBeVisible();

    // Click preview on first template
    await page.click('.template-card:first-child button:text("Preview")');
    await expect(page.locator('#template-preview-modal')).toBeVisible();
    await expect(page.locator('#template-preview-content')).toBeVisible();

    // Close modal
    await page.click('#template-preview-modal button:text("Close")');
    await expect(page.locator('#template-preview-modal')).toHaveClass(/hidden/);
  });

  test('load template to editor', async ({ page }) => {
    await page.click('button[data-view="templates"]');
    await expect(page.locator('#templates-grid')).toBeVisible();

    // Click Use on first template
    await page.click('.template-card:first-child button:text("Use")');

    // Should navigate to editor
    await expect(page.locator('#view-editor')).toBeVisible();
    await expect(page.locator('#editor-text')).not.toBeEmpty();
  });

  test('template categories are displayed', async ({ page }) => {
    await page.click('button[data-view="templates"]');
    await expect(page.locator('#templates-grid')).toBeVisible();

    // Check for category labels
    const cards = await page.locator('.template-card').all();
    expect(cards.length).toBeGreaterThan(0);

    // Each card should have a category
    for (const card of cards.slice(0, 3)) {
      const category = await card.locator('div:text-matches("Basic|Genomics|Advanced")').textContent();
      expect(category).toBeTruthy();
    }
  });

  test('template tags are displayed', async ({ page }) => {
    await page.click('button[data-view="templates"]');
    await expect(page.locator('#templates-grid')).toBeVisible();

    // Check for tags
    await expect(page.locator('.template-card').first()).toContainText('Tags:');
  });

  test('use template from preview modal', async ({ page }) => {
    await page.click('button[data-view="templates"]');
    await page.click('.template-card:first-child button:text("Preview")');
    await expect(page.locator('#template-preview-modal')).toBeVisible();

    // Click Use in modal
    await page.click('#template-preview-modal button:text("Use This Template")');

    // Should navigate to editor
    await expect(page.locator('#view-editor')).toBeVisible();
    await expect(page.locator('#editor-text')).not.toBeEmpty();
  });
});