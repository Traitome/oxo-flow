import { test, expect } from '@playwright/test';

test.describe('Authentication', () => {
  test('login with valid credentials', async ({ page }) => {
    await page.goto('/');

    // Open login modal
    await page.click('.user-info .avatar');
    await page.click('button:text("Sign In")');

    // Fill login form
    await page.fill('#login-username', 'admin');
    await page.fill('#login-password', 'admin');
    await page.click('button:text("Sign In")');

    // Verify logged in
    await expect(page.locator('#user-name')).toHaveText('admin');
    await expect(page.locator('#conn-dot')).toHaveClass(/ok/);
  });

  test('login with invalid credentials shows error', async ({ page }) => {
    await page.goto('/');

    await page.click('.user-info .avatar');
    await page.click('button:text("Sign In")');

    await page.fill('#login-username', 'wrong');
    await page.fill('#login-password', 'wrong');
    await page.click('.modal button:text("Sign In")');

    await expect(page.locator('#login-error')).toBeVisible();
  });

  test('login form supports Enter key', async ({ page }) => {
    await page.goto('/');

    await page.click('.user-info .avatar');
    await page.click('button:text("Sign In")');

    await page.fill('#login-username', 'admin');
    await page.fill('#login-password', 'admin');
    await page.press('#login-password', 'Enter');

    await expect(page.locator('#user-name')).toHaveText('admin');
  });
});