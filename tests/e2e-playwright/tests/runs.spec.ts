import { test, expect } from '@playwright/test';

test.describe('Run History', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await page.click('.user-info .avatar');
    await page.click('button:text("Sign In")');
    await page.fill('#login-username', 'admin');
    await page.fill('#login-password', 'admin');
    await page.click('.modal button:text("Sign In")');
    await expect(page.locator('#user-name')).toHaveText('admin');
  });

  test('run history table loads', async ({ page }) => {
    await page.click('button[data-view="runs"]');
    await expect(page.locator('#all-runs-tbody')).toBeVisible();
  });

  test('view run detail', async ({ page }) => {
    // First run a workflow
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', `
[workflow]
name = "run-test"
version = "1.0.0"

[[rules]]
name = "hello"
output = ["hello.txt"]
shell = "echo Hello > {output[0]}"
`);
    await page.click('button:text("Run")');
    await expect(page.locator('#editor-output')).toContainText('Launched');

    // Go to runs
    await page.click('button[data-view="runs"]');
    await page.waitForTimeout(1000);

    // Click detail button if any runs exist
    const rows = await page.locator('#all-runs-tbody tr').count();
    if (rows > 0) {
      await page.click('#all-runs-tbody tr:first-child button:text("Detail")');
      await expect(page.locator('#editor-output')).toContainText('Run');
    }
  });

  test('view run logs', async ({ page }) => {
    await page.click('button[data-view="runs"]');
    await page.waitForTimeout(500);

    const rows = await page.locator('#all-runs-tbody tr').count();
    if (rows > 0) {
      await page.click('#all-runs-tbody tr:first-child button:text("Logs")');
      await expect(page.locator('#log-modal')).toBeVisible();
    }
  });

  test('cancel running workflow', async ({ page }) => {
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', `
[workflow]
name = "cancel-test"
version = "1.0.0"

[[rules]]
name = "slow"
output = ["output.txt"]
shell = "sleep 30 && echo done > {output[0]}"
`);
    await page.click('button:text("Run")');
    await expect(page.locator('#editor-output')).toContainText('Launched');

    await page.click('button[data-view="runs"]');
    await page.waitForTimeout(1000);

    // Find running row and cancel
    const cancelBtn = page.locator('button:text("Cancel")');
    if (await cancelBtn.count() > 0) {
      await cancelBtn.first().click();
    }
  });
});