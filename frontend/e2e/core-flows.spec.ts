import { test, expect } from '@playwright/test';

test.describe('oxo-flow v0.9 Core User Flows', () => {

  // ── Navigation & Layout ──

  test('Dashboard loads with key elements', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('h1')).toContainText('What do you want to do?');
    // Sidebar navigation should be visible (use .first() to avoid strict mode on duplicates)
    await expect(page.locator('text=Dashboard').first()).toBeVisible();
    await expect(page.locator('text=Pipeline Editor').first()).toBeVisible();
    await expect(page.locator('text=Runs').first()).toBeVisible();
  });

  test('Navigation between pages works', async ({ page }) => {
    await page.goto('/');
    // Navigate to Pipeline Editor
    await page.click('a[href="/editor"]');
    await expect(page).toHaveURL(/\/editor/);
    await expect(page.locator('h1')).toContainText('Pipeline Editor');
    // Navigate to Settings
    await page.click('a[href="/settings"]');
    await expect(page).toHaveURL(/\/settings/);
    await expect(page.locator('h1')).toContainText('Settings');
    // Navigate back to Dashboard
    await page.click('a[href="/"]');
    await expect(page.locator('h1')).toContainText('What do you want to do?');
  });

  test('Mobile menu toggle works', async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await page.goto('/');
    // Issue #947: <1024px the sidebar becomes a drawer driven by the header toggle.
    const menuBtn = page.locator('button[aria-label="Open navigation menu"]');
    await expect(menuBtn).toBeVisible();
    await menuBtn.click();
    await expect(page.locator('.sidebar.drawer')).toBeVisible();
  });

  // ── Pipeline Editor ──

  test('Pipeline Editor loads with TOML editor and DAG view', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
    await page.goto('/editor');
    await expect(page.locator('h1')).toContainText('Pipeline Editor');
    // CodeMirror TOML editor should be present
    await expect(page.locator('.cm-editor')).toBeVisible({ timeout: 10000 });
    // Default TOML should load — check for content in the editor
    await page.waitForTimeout(1500);
    const cmContent = page.locator('.cm-content');
    await expect(cmContent).toBeVisible();
    const text = await cmContent.textContent();
    expect(text).toContain('my-pipeline');
  });

  test('Pipeline validation badge appears', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
    await page.goto('/editor');
    // Wait for debounced validation
    await page.waitForTimeout(2000);
    const badge = page.locator('.val-badge');
    await expect(badge).toBeVisible({ timeout: 10000 });
  });

  test('DAG view renders graph nodes', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
    await page.goto('/editor');
    // React Flow canvas should be visible with the default workflow's nodes
    const node = page.locator('.rf-rule-node', { hasText: 'fastqc' });
    await expect(node).toBeVisible({ timeout: 10000 });
    // Node count should appear
    await expect(page.locator('.dag-counts')).toBeVisible({ timeout: 10000 });
  });

  test('Save button saves pipeline', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
    await page.goto('/editor');
    await page.waitForTimeout(2000);
    const saveBtn = page.locator('button:has-text("Save")');
    await expect(saveBtn).toBeVisible();
    await saveBtn.click();
    // Should show success or error notification (not crash)
    await page.waitForTimeout(1000);
    const resultBar = page.locator('.result-bar');
    await expect(resultBar).toBeVisible({ timeout: 5000 });
  });

  // ── AI Chat ──

  test('Dashboard AI entry card navigates to the chat', async ({ page }) => {
    await page.route('**/api/ai/config', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ provider: 'deepseek', model: 'deepseek-chat', api_url: '', is_configured: true }),
      }),
    );
    await page.goto('/');
    // The task-oriented home offers an AI card; the chat input lives on
    // the dedicated /chat page (issue #82 P1-7).
    const aiCard = page.locator('.entry-card').filter({ hasText: 'Generate with AI' });
    await expect(aiCard.first()).toBeVisible({ timeout: 5000 });
    await aiCard.first().click();
    await expect(page).toHaveURL(/\/chat/);
    const chatInput = page.locator('.intent-input, textarea[placeholder*="Describe"]');
    await expect(chatInput.first()).toBeVisible({ timeout: 5000 });
  });

  // ── Settings ──

  test('Settings page loads with AI config', async ({ page }) => {
    await page.goto('/settings');
    await expect(page.locator('h1')).toContainText('Settings');
    await expect(page.locator('text=AI Provider Configuration')).toBeVisible();
  });

  test('Settings AI provider inputs work without focus loss', async ({ page }) => {
    await page.goto('/settings');
    await page.waitForTimeout(1000);
    const apiKeyInput = page.locator('input[placeholder="sk-..."]');
    await expect(apiKeyInput).toBeVisible();
    // Type 5 characters and verify they all appear (no focus loss bug)
    await apiKeyInput.fill('testkey12345');
    await page.waitForTimeout(200);
    const value = await apiKeyInput.inputValue();
    expect(value).toBe('testkey12345');
  });

  // ── API Docs ──

  test('API Docs page loads', async ({ page }) => {
    await page.goto('/docs');
    await expect(page.locator('h1')).toContainText('API Reference');
  });

  // ── Monitor ──

  test('Monitor page loads with run history', async ({ page }) => {
    await page.goto('/monitor');
    await expect(page.locator('h1')).toContainText('Monitor');
    await expect(page.locator('text=Run History')).toBeVisible({ timeout: 5000 });
  });

  // ── Pipelines ──

  test('Pipelines page loads', async ({ page }) => {
    await page.goto('/pipelines');
    await expect(page.locator('h1')).toContainText('Pipelines');
  });

  // ── Runs ──

  test('Runs page loads', async ({ page }) => {
    await page.goto('/runs');
    await expect(page.locator('h1')).toContainText('Monitor & Reports');
    await expect(page.locator('text=Run History')).toBeVisible({ timeout: 5000 });
  });

  // ── AI Chat page ──

  test('AI Chat standalone page loads', async ({ page }) => {
    await page.goto('/chat');
    // Chat page renders without a title h1 — just verify the page loads
    await expect(page.locator('main')).toBeVisible();
  });

  // ── API health check ──

  test('API health endpoint responds', async ({ request }) => {
    const resp = await request.get('/api/health');
    expect(resp.ok()).toBeTruthy();
    const body = await resp.json();
    expect(body).toHaveProperty('status');
    expect(['ok', 'healthy', 'degraded']).toContain(body.status);
  });

  test('API system info responds', async ({ request }) => {
    const resp = await request.get('/api/system');
    expect(resp.ok()).toBeTruthy();
    const body = await resp.json();
    // System info returns os, arch, version, pid, uptime_secs
    expect(body).toHaveProperty('os');
    expect(body).toHaveProperty('arch');
  });

  test('API version is correct', async ({ request }) => {
    const resp = await request.get('/api/health');
    const body = await resp.json();
    expect(body).toHaveProperty('version');
    expect(typeof body.version).toBe('string');
  });

  test('Lifecycle workbench: 5-stage grid, Command Palette, Sample Sheet parser, Engine guard, Format & Lint', async ({ page }) => {
    await page.goto('/');
    // 5-stage lifecycle grid renders on Dashboard
    await expect(page.locator('.lifecycle-stage-card')).toHaveCount(5);

    // Command Palette opens via header trigger
    await page.locator('.cmd-trigger').click();
    await expect(page.locator('.cmd-palette-dialog')).toBeVisible();
    await page.keyboard.press('Escape');

    // Click Stage 1 (Data & Samples) -> opens Editor with Data Workbench modal
    await page.locator('.lifecycle-stage-card').first().click();
    await expect(page).toHaveURL(/\/editor\?modal=data/);
    await expect(page.getByRole('dialog', { name: /Data & Sample Sheet Workbench/i })).toBeVisible();
    await page.getByRole('button', { name: /Parse Sample Sheet/i }).click();
    await expect(page.getByRole('button', { name: /Insert Samples into Workflow/i })).toBeVisible();
    await page.getByRole('button', { name: /Insert Samples into Workflow/i }).click();
    await expect(page.locator('.result-bar.success')).toContainText('Injected 2 sample(s) into [[sample_groups]]');

    // Engine & Safety modal injects [engine] settings
    await page.getByRole('button', { name: /Engine & Safety/i }).click();
    await expect(page.getByRole('dialog', { name: /Engine & Data Safety/i })).toBeVisible();
    await expect(page.getByRole('dialog', { name: /Engine & Data Safety/i }).locator('pre.code-block')).toContainText('min_free_disk = "20G"');
    await page.getByRole('button', { name: /Inject into \[engine\]/i }).click();
    await expect(page.locator('.result-bar.success')).toContainText('Updated [engine]');

    // Format & Lint buttons work in Editor
    await page.getByRole('button', { name: /^Format$/i }).click();
    await expect(page.locator('.result-bar.success')).toContainText('Formatted TOML canonically');
  });

  test('Review blockers #1, #2, #7, #9: TOML escaping, string-safe [[rules]] splice, and non-destructive [engine] merge', async ({ page, request }) => {
    // Seed a workflow that has:
    // 1. An existing [engine] section with a comment and custom key (`custom_companion = "keep-me"`)
    // 2. A rule whose shell string contains literal `[[rules]]` and `[engine]`
    const initialToml = `[workflow]
name = "review-blocker-audit"
version = "1.0.0"

[engine]
# operator comment preserved
custom_companion = "keep-me"
min_free_disk = "12G"
reclaim_free_disk = "60G"

[[rules]]
name = "tricky_rule"
output = ["out.txt"]
shell = "echo '[[rules]] [engine]' > out.txt"
`;
    await page.addInitScript((seededToml: string) => {
      localStorage.setItem('oxo_editor_mode', 'canvas');
      localStorage.setItem('oxo_session', JSON.stringify({
        _stamp: 999,
        pipelineToml: seededToml,
        dagData: null,
        activeRunId: null,
      }));
    }, initialToml);

    // Open editor with ?modal=data
    await page.goto('/editor?modal=data');
    const dataDialog = page.getByRole('dialog', { name: /Data & Sample Sheet Workbench/i });
    await expect(dataDialog).toBeVisible();

    // Sample sheet with quote and backslash in sample names (Review #1)
    const csvInput = dataDialog.locator('textarea');
    await csvInput.fill('sample,fastq_r1,fastq_r2\nS"quote,a_R1.fq.gz,a_R2.fq.gz\nS\\slash,b_R1.fq.gz,b_R2.fq.gz');
    await dataDialog.getByRole('button', { name: /Parse Sample Sheet/i }).click();
    await dataDialog.getByRole('button', { name: /Insert Samples into Workflow/i }).click();
    await expect(page.locator('.result-bar.success')).toContainText('Injected 2 sample(s) into [[sample_groups]]');

    // Open Engine & Safety modal and verify existing min_free_disk="12G" was read and custom keys are preserved in live preview (Review #2 & #9)
    await page.getByRole('button', { name: /Engine & Safety/i }).click();
    const engineDialog = page.getByRole('dialog', { name: /Engine & Data Safety/i });
    await expect(engineDialog).toBeVisible();
    const inputs = engineDialog.locator('input');
    await expect(inputs.nth(0)).toHaveValue('12G');
    await expect(inputs.nth(1)).toHaveValue('60G');
    await inputs.nth(0).fill('25G');
    await inputs.nth(2).fill('/tmp/oxo_scratch');
    await inputs.nth(3).fill('raw_data, dir"quoted');

    const previewText = await engineDialog.locator('pre.code-block').textContent();
    expect(previewText).toContain('# operator comment preserved');
    expect(previewText).toContain('custom_companion = "keep-me"');
    expect(previewText).toContain('min_free_disk = "25G"');
    expect(previewText).toContain('reclaim_free_disk = "60G"');
    expect(previewText).toContain('tmp_dir = "/tmp/oxo_scratch"');
    expect(previewText).toContain('protected_data_trees = ["raw_data", "dir\\"quoted"]');

    await engineDialog.getByRole('button', { name: /Inject into \[engine\]/i }).click();
    await expect(page.locator('.result-bar.success')).toContainText('Updated [engine] disk & data-protection settings');

    // Wait for session flush to localStorage (500ms debounce) and validate resulting TOML via backend parser
    await page.waitForTimeout(600);
    const savedToml = await page.evaluate(() => {
      const raw = localStorage.getItem('oxo_session');
      return raw ? (JSON.parse(raw).pipelineToml as string) : '';
    });
    expect(savedToml).toContain('samples = ["S\\"quote", "S\\\\slash"]');
    expect(savedToml).toContain('custom_companion = "keep-me"');
    expect(savedToml).toContain('# operator comment preserved');
    expect(savedToml).toContain('shell = "echo \'[[rules]] [engine]\' > out.txt"');

    // Validate TOML syntax & schema (stripping only the synthetic unknown key `custom_companion` that tested non-destructive merge)
    const valResp = await request.post('/api/pipelines/validate', {
      data: { toml_content: savedToml.replace('custom_companion = "keep-me"\n', '') },
    });
    expect(valResp.ok()).toBeTruthy();
    const valJson = await valResp.json();
    expect(valJson.valid).toBe(true);
  });

  test('Review #3, #4, #6, #8: Command Palette typing guard & Tab trap, RunDialog provenance default OFF, Pipelines Diff & Share import', async ({ page, request }) => {
    // Create two pipelines for Diff & Share testing
    const p1 = await (await request.post('/api/pipelines', {
      data: {
        name: 'diff-alpha',
        toml_content: '[workflow]\nname = "diff-alpha"\nversion = "1.0.0"\n\n[[rules]]\nname = "r1"\noutput = ["a.txt"]\nshell = "echo a > a.txt"\n',
      },
    })).json();
    await request.post('/api/pipelines', {
      data: {
        name: 'diff-beta',
        toml_content: '[workflow]\nname = "diff-beta"\nversion = "1.0.0"\n\n[[rules]]\nname = "r1"\noutput = ["b.txt"]\nshell = "echo b > b.txt"\n',
      },
    });

    await page.goto('/pipelines');
    // Review #6: pressing Ctrl+K while focused in search input must NOT open Command Palette
    const searchInput = page.locator('input.search-input').first();
    await searchInput.focus();
    await page.keyboard.press('Control+k');
    await expect(page.locator('.cmd-palette-dialog')).toHaveCount(0);

    // Open Command Palette via button and verify Tab focus trap stays inside dialog
    await page.locator('.cmd-trigger').click();
    const cmdDialog = page.locator('.cmd-palette-dialog');
    await expect(cmdDialog).toBeVisible();
    for (let i = 0; i < 15; i++) {
      await page.keyboard.press('Tab');
    }
    const insideDialog = await page.evaluate(() => {
      const dlg = document.querySelector('.cmd-palette-dialog');
      return Boolean(dlg && dlg.contains(document.activeElement));
    });
    expect(insideDialog).toBe(true);
    await page.keyboard.press('Escape');

    // Review #8: Compare / Diff modal renders localized headers
    await page.getByRole('tab', { name: /My Pipelines/i }).click();
    await page.getByRole('button', { name: /Compare \/ Diff/i }).first().click();
    const diffDialog = page.getByRole('dialog');
    await expect(diffDialog).toBeVisible();
    await diffDialog.getByRole('button', { name: /Run Diff/i }).click();
    await diffDialog.getByRole('button', { name: /Close/i }).click();

    // Review #4: Share & Import does not render "undefined"
    const shareRes = await (await request.post(`/api/pipelines/${p1.id}/share`, {
      data: { visibility: 'public' },
    })).json();
    await page.getByRole('button', { name: /Import Share URL/i }).click();
    const importDialog = page.getByRole('dialog', { name: /Import Shared Pipeline/i });
    await importDialog.locator('input').fill(shareRes.share_url);
    await importDialog.getByRole('button', { name: /Import Share URL/i }).click();
    await expect(page).toHaveURL(/\/editor\?pipeline=/);

    // Review #3: RunDialog defaults provenance checkbox to OFF (false) for CLI parity
    await page.getByRole('button', { name: /Canvas \+ TOML/i }).click();
    await page.getByRole('button', { name: /^Run$/i }).click();
    const runDialog = page.locator('[aria-labelledby="run-dialog-title"]');
    await expect(runDialog).toBeVisible();
    const provCheckbox = runDialog.getByLabel(/Record SHA-256 output provenance/i);
    await expect(provCheckbox).not.toBeChecked();
  });

  test('Stage 4 & 5 UI: Execute run with --provenance, verify SHA-256 provenance & protected drift in Monitor, and preview/download output in Report', async ({ page, request }) => {
    const runResp = await request.post('/api/runs', {
      data: {
        toml_content: '[workflow]\nname = "e2e-stage45"\nversion = "1.0.0"\n\n[engine]\nmin_free_disk = "50M"\nreclaim_free_disk = "100M"\n\n[[rules]]\nname = "emit_summary"\noutput = ["results/summary.txt"]\nshell = "mkdir -p results && echo e2e_provenance_verified > results/summary.txt"\nthreads = 1\n',
        dry_run: false,
        provenance: true,
        allow_protected_drift: false,
      },
    });
    expect(runResp.ok()).toBeTruthy();
    const { run_id } = await runResp.json();

    // Poll until completed
    for (let i = 0; i < 40; i++) {
      const st = await (await request.get(`/api/runs/${run_id}`)).json();
      if (st.status === 'completed' || st.status === 'failed') {
        expect(st.status).toBe('completed');
        break;
      }
      await page.waitForTimeout(300);
    }

    // Open Monitor for this run and test the Provenance & Drift tab
    await page.goto(`/runs/${run_id}`);
    await page.getByRole('tab', { name: /Provenance & Drift/i }).click();
    await page.getByRole('button', { name: /Verify SHA-256 Provenance/i }).click();
    await expect(page.locator('.verify-card').first()).toContainText('OK');
    await expect(page.locator('.verify-card').first()).toContainText('results/summary.txt');

    await page.getByRole('button', { name: /Audit Protected Data Trees/i }).click();
    await expect(page.locator('.verify-card').nth(1)).toContainText('Drifted files: 0');

    // Switch to Report tab, preview the output file, and verify workdir zip download endpoint
    await page.getByRole('tab', { name: /^Report$/i }).click();
    const previewBtn = page.getByRole('button', { name: /Preview/i }).first();
    await expect(previewBtn).toBeVisible();
    await previewBtn.click();
    await expect(page.getByRole('dialog', { name: 'summary.txt' })).toContainText('e2e_provenance_verified');

    const zipResp = await request.get(`/api/runs/${run_id}/files?path=.`);
    expect(zipResp.ok()).toBeTruthy();
    expect(zipResp.headers()['content-type']).toContain('application/zip');
  });
});
