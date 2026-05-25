import { test, expect } from '@playwright/test';

test.describe('System', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await page.click('.user-info .avatar');
    await page.click('button:text("Sign In")');
    await page.fill('#login-username', 'admin');
    await page.fill('#login-password', 'admin');
    await page.click('.modal button:text("Sign In")');
    await expect(page.locator('#user-name')).toHaveText('admin');
  });

  test('system view shows environments', async ({ page }) => {
    await page.click('button[data-view="system"]');
    await expect(page.locator('#view-system')).toBeVisible();

    // Check for environment info
    await expect(page.locator('#view-system .card')).toBeVisible();
  });

  test('system view shows version info', async ({ page }) => {
    await page.click('button[data-view="system"]');
    await expect(page.locator('#view-system')).toContainText('Version');
  });
});