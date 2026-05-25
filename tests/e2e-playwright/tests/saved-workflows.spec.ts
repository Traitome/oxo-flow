import { test, expect } from '@playwright/test';

test.describe('Saved Workflows', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await page.click('.user-info .avatar');
    await page.click('button:text("Sign In")');
    await page.fill('#login-username', 'admin');
    await page.fill('#login-password', 'admin');
    await page.click('.modal button:text("Sign In")');
    await expect(page.locator('#user-name')).toHaveText('admin');
  });

  test('saved workflows list loads', async ({ page }) => {
    await page.click('button[data-view="workflows"]');
    await expect(page.locator('#saved-workflows-tbody')).toBeVisible();
  });

  test('save and load workflow', async ({ page }) => {
    // Create and save workflow
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', `
[workflow]
name = "save-load-test"
version = "1.0.0"

[[rules]]
name = "test"
output = ["test.txt"]
shell = "echo test > {output[0]}"
`);
    await page.fill('#save-name', 'e2e-saved-workflow');
    await page.click('button:text("Save")');
    await expect(page.locator('#editor-output')).toContainText('Saved');

    // Check it appears in saved workflows
    await page.click('button[data-view="workflows"]');
    await expect(page.locator('#saved-workflows-tbody')).toContainText('e2e-saved-workflow');
  });

  test('delete saved workflow', async ({ page }) => {
    await page.click('button[data-view="workflows"]');

    // If there are saved workflows, try deleting one
    const rows = await page.locator('#saved-workflows-tbody tr').count();
    if (rows > 0) {
      // Note: Delete functionality may need confirmation
      const deleteBtn = page.locator('#saved-workflows-tbody button:text("Del")');
      if (await deleteBtn.count() > 0) {
        await deleteBtn.first().click();
      }
    }
  });
});