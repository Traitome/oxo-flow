import { test, expect } from '@playwright/test';

test.describe('Dashboard', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    // Login first
    await page.click('.user-info .avatar');
    await page.click('button:text("Sign In")');
    await page.fill('#login-username', 'admin');
    await page.fill('#login-password', 'admin');
    await page.click('.modal button:text("Sign In")');
    await expect(page.locator('#user-name')).toHaveText('admin');
  });

  test('displays system metrics', async ({ page }) => {
    // Navigate to dashboard
    await page.click('button[data-view="dashboard"]');

    // Check metrics are displayed
    await expect(page.locator('#d-cpu')).not.toBeEmpty();
    await expect(page.locator('#d-mem')).not.toBeEmpty();
    await expect(page.locator('#d-runs')).not.toBeEmpty();
    await expect(page.locator('#d-uptime')).not.toBeEmpty();
  });

  test('topbar shows live metrics', async ({ page }) => {
    await expect(page.locator('#live-cpu')).not.toBeEmpty();
    await expect(page.locator('#live-mem')).not.toBeEmpty();
    await expect(page.locator('#live-runs')).not.toBeEmpty();
  });

  test('recent runs table loads', async ({ page }) => {
    await page.click('button[data-view="dashboard"]');
    await expect(page.locator('#recent-runs-tbody')).toBeVisible();
  });

  test('refresh dashboard button works', async ({ page }) => {
    await page.click('button[data-view="dashboard"]');
    await page.click('#view-dashboard .btn:text("Refresh")');
    // Should still show metrics after refresh
    await expect(page.locator('#d-cpu')).not.toBeEmpty();
  });
});