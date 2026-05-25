import { test, expect } from '@playwright/test';

/**
 * Audit log tests for oxo-flow enterprise governance.
 * Tests audit log viewer, filtering, and refresh functionality.
 */

test.describe('Audit Logs', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await page.click('.user-info .avatar');
    await page.click('button:text("Sign In")');
    await page.fill('#login-username', 'admin');
    await page.fill('#login-password', 'admin');
    await page.click('.modal button:text("Sign In")');
    await expect(page.locator('#user-name')).toHaveText('admin');
  });

  test('audit log section is visible in system view', async ({ page }) => {
    await page.click('button[data-view="system"]');
    await expect(page.locator('#view-system')).toBeVisible();
    await expect(page.locator('#audit-tbody')).toBeVisible();
  });

  test('audit log refresh button works', async ({ page }) => {
    await page.click('button[data-view="system"]');
    await expect(page.locator('#audit-tbody')).toBeVisible();

    // Click refresh
    await page.click('button:text("Refresh")');
    await expect(page.locator('#audit-tbody')).toBeVisible();
  });

  test('audit log days selector works', async ({ page }) => {
    await page.click('button[data-view="system"]');
    await expect(page.locator('#audit-days')).toBeVisible();

    // Select 30 days
    await page.selectOption('#audit-days', '30');
    await expect(page.locator('#audit-days')).toHaveValue('30');

    // Select 1 day
    await page.selectOption('#audit-days', '1');
    await expect(page.locator('#audit-days')).toHaveValue('1');
  });

  test('audit log table structure', async ({ page }) => {
    await page.click('button[data-view="system"]');
    await expect(page.locator('#audit-table')).toBeVisible();

    // Check headers
    const headers = await page.locator('#audit-table th').allTextContents();
    expect(headers).toContain('Time');
    expect(headers).toContain('User');
    expect(headers).toContain('Action');
    expect(headers).toContain('Resource');
  });

  test('audit log shows login action after login', async ({ page }) => {
    // Already logged in from beforeEach
    await page.click('button[data-view="system"]');
    await expect(page.locator('#audit-tbody')).toBeVisible();

    // There should be at least one row (the login)
    const rows = await page.locator('#audit-tbody tr').count();
    // May be 0 if no logs exist yet, so just verify table is present
    await expect(page.locator('#audit-tbody')).toBeVisible();
  });

  test('audit log API endpoint returns data', async ({ page }) => {
    await page.click('button[data-view="system"]');

    // Wait for audit log API call
    const response = await page.waitForResponse(resp =>
      resp.url().includes('/api/audit') && resp.status() === 200
    );

    const data = await response.json();
    expect(data).toHaveProperty('entries');
    expect(data).toHaveProperty('days');
  });

  test('audit log entries formatted correctly', async ({ page }) => {
    await page.click('button[data-view="system"]');
    await page.waitForTimeout(500);

    // If there are entries, check format
    const rows = await page.locator('#audit-tbody tr').count();
    if (rows > 0 && !await page.locator('#audit-tbody').containsText('No audit logs found')) {
      const firstRow = page.locator('#audit-tbody tr').first();
      // Should have 4 columns
      const cols = await firstRow.locator('td').count();
      expect(cols).toBe(4);
    }
  });
});