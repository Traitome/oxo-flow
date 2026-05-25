import { test, expect } from '@playwright/test';

test.describe('Workflow Editor', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await page.click('.user-info .avatar');
    await page.click('button:text("Sign In")');
    await page.fill('#login-username', 'admin');
    await page.fill('#login-password', 'admin');
    await page.click('.modal button:text("Sign In")');
    await expect(page.locator('#user-name')).toHaveText('admin');
    await page.click('button[data-view="editor"]');
  });

  const validWorkflow = `
[workflow]
name = "test-workflow"
version = "1.0.0"

[[rules]]
name = "hello"
output = ["hello.txt"]
shell = "echo Hello > {output[0]}"
`;

  test('validate workflow', async ({ page }) => {
    await page.fill('#editor-text', validWorkflow);
    await page.click('button:text("Validate")');
    await expect(page.locator('#editor-output')).toContainText('Valid');
  });

  test('dry-run workflow', async ({ page }) => {
    await page.fill('#editor-text', validWorkflow);
    await page.click('button:text("Dry Run")');
    await expect(page.locator('#editor-output')).toContainText('Dry-run');
  });

  test('format workflow', async ({ page }) => {
    await page.fill('#editor-text', validWorkflow);
    await page.click('button:text("Format")');
    await expect(page.locator('#editor-output')).toContainText('Formatted');
  });

  test('lint workflow', async ({ page }) => {
    await page.fill('#editor-text', validWorkflow);
    await page.click('button:text("Lint")');
    await expect(page.locator('#editor-output')).toContainText('Lint');
  });

  test('show DAG', async ({ page }) => {
    await page.fill('#editor-text', validWorkflow);
    await page.click('button:text("View DAG")');
    await expect(page.locator('#dag-modal')).toBeVisible();
    await expect(page.locator('#dag-content')).not.toBeEmpty();
  });

  test('export Dockerfile', async ({ page }) => {
    await page.fill('#editor-text', validWorkflow);
    await page.click('button:text("Export Dockerfile")');
    await expect(page.locator('#export-modal')).toBeVisible();
    await expect(page.locator('#export-content')).toContainText('FROM');
  });

  test('run workflow', async ({ page }) => {
    await page.fill('#editor-text', validWorkflow);
    await page.click('button:text("Run")');
    await expect(page.locator('#editor-output')).toContainText('Launched');
  });

  test('save workflow to library', async ({ page }) => {
    await page.fill('#editor-text', validWorkflow);
    await page.fill('#save-name', 'e2e-test-workflow');
    await page.click('button:text("Save")');
    await expect(page.locator('#editor-output')).toContainText('Saved');
  });

  test('invalid workflow shows error', async ({ page }) => {
    await page.fill('#editor-text', 'invalid toml content');
    await page.click('button:text("Validate")');
    await expect(page.locator('#editor-output')).toContainText('failed');
  });
});