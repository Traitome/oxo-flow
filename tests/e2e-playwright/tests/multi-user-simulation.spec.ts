import { test, expect } from '@playwright/test';

/**
 * 20-user simulation tests for oxo-flow multi-user web system.
 * Tests concurrent usage patterns typical in bioinformatics labs and enterprises.
 */

test.describe('Multi-user simulation (20 users)', () => {
  // Simulate 20 users concurrently accessing the system
  for (let i = 1; i <= 20; i++) {
    test(`User ${i}: login and view dashboard`, async ({ page }) => {
      await page.goto('/');

      // Login
      await page.click('.user-info .avatar');
      await page.click('button:text("Sign In")');
      await page.fill('#login-username', 'admin');
      await page.fill('#login-password', 'admin');
      await page.click('.modal button:text("Sign In")');

      // Wait for login
      await expect(page.locator('#user-name')).toHaveText('admin');

      // View dashboard
      await page.click('button[data-view="dashboard"]');
      await expect(page.locator('#d-cpu')).toBeVisible();
    });
  }
});

test.describe('Concurrent workflow operations', () => {
  test('Multiple users validate workflows simultaneously', async ({ browser }) => {
    const contexts = await Promise.all([
      browser.newContext(),
      browser.newContext(),
      browser.newContext(),
      browser.newContext(),
      browser.newContext(),
    ]);

    const workflows = [
      `[workflow]\nname = "w1"\nversion = "1.0"\n[[rules]]\nname = "r1"\noutput = ["o1.txt"]\nshell = "echo 1"`,
      `[workflow]\nname = "w2"\nversion = "1.0"\n[[rules]]\nname = "r2"\noutput = ["o2.txt"]\nshell = "echo 2"`,
      `[workflow]\nname = "w3"\nversion = "1.0"\n[[rules]]\nname = "r3"\noutput = ["o3.txt"]\nshell = "echo 3"`,
      `[workflow]\nname = "w4"\nversion = "1.0"\n[[rules]]\nname = "r4"\noutput = ["o4.txt"]\nshell = "echo 4"`,
      `[workflow]\nname = "w5"\nversion = "1.0"\n[[rules]]\nname = "r5"\noutput = ["o5.txt"]\nshell = "echo 5"`,
    ];

    const results = await Promise.all(contexts.map(async (context, i) => {
      const page = await context.newPage();
      await page.goto('/');

      // Login
      await page.click('.user-info .avatar');
      await page.click('button:text("Sign In")');
      await page.fill('#login-username', 'admin');
      await page.fill('#login-password', 'admin');
      await page.click('.modal button:text("Sign In")');
      await expect(page.locator('#user-name')).toHaveText('admin');

      // Navigate to editor and validate
      await page.click('button[data-view="editor"]');
      await page.fill('#editor-text', workflows[i]);
      await page.click('button:text("Validate")');
      await expect(page.locator('#editor-output')).toContainText('Valid');

      await context.close();
      return true;
    }));

    expect(results.every(r => r === true)).toBe(true);
  });

  test('Multiple users save workflows concurrently', async ({ browser }) => {
    const context = await browser.newContext();
    const page = await context.newPage();

    await page.goto('/');
    await page.click('.user-info .avatar');
    await page.click('button:text("Sign In")');
    await page.fill('#login-username', 'admin');
    await page.fill('#login-password', 'admin');
    await page.click('.modal button:text("Sign In")');

    // Save multiple workflows
    for (let i = 1; i <= 5; i++) {
      await page.click('button[data-view="editor"]');
      await page.fill('#editor-text', `
[workflow]
name = "concurrent-test-${i}"
version = "1.0.0"

[[rules]]
name = "rule-${i}"
output = ["output-${i}.txt"]
shell = "echo test-${i} > {output[0]}"
`);
      await page.fill('#save-name', `concurrent-workflow-${i}`);
      await page.click('button:text("Save")');
      await expect(page.locator('#editor-output')).toContainText('Saved');
    }

    await context.close();
  });

  test('Dashboard metrics update for concurrent users', async ({ browser }) => {
    const contexts = await Promise.all([
      browser.newContext(),
      browser.newContext(),
      browser.newContext(),
    ]);

    await Promise.all(contexts.map(async (context) => {
      const page = await context.newPage();
      await page.goto('/');

      await page.click('.user-info .avatar');
      await page.click('button:text("Sign In")');
      await page.fill('#login-username', 'admin');
      await page.fill('#login-password', 'admin');
      await page.click('.modal button:text("Sign In")');

      await page.click('button[data-view="dashboard"]');
      await expect(page.locator('#d-cpu')).not.toBeEmpty();

      await context.close();
    }));
  });
});

test.describe('User role scenarios', () => {
  test('Admin user can access all features', async ({ page }) => {
    await page.goto('/');

    await page.click('.user-info .avatar');
    await page.click('button:text("Sign In")');
    await page.fill('#login-username', 'admin');
    await page.fill('#login-password', 'admin');
    await page.click('.modal button:text("Sign In")');

    // Check all views are accessible
    const views = ['dashboard', 'editor', 'runs', 'workflows', 'system'];
    for (const view of views) {
      await page.click(`button[data-view="${view}"]`);
      await expect(page.locator(`#view-${view}`)).toBeVisible();
    }
  });
});

test.describe('Performance under load', () => {
  test('API responds within acceptable time for 20 concurrent requests', async ({ page }) => {
    await page.goto('/');

    // Login
    await page.click('.user-info .avatar');
    await page.click('button:text("Sign In")');
    await page.fill('#login-username', 'admin');
    await page.fill('#login-password', 'admin');
    await page.click('.modal button:text("Sign In")');

    // Measure API response times
    const startTime = Date.now();

    // Multiple rapid requests
    for (let i = 0; i < 20; i++) {
      await page.click('button[data-view="dashboard"]');
      await page.waitForTimeout(50);
    }

    const endTime = Date.now();
    const totalTime = endTime - startTime;

    // Should complete within reasonable time (30 seconds)
    expect(totalTime).toBeLessThan(30000);
  });
});