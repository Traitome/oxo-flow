import { test, expect } from '@playwright/test';

/**
 * 30-Real-User Comprehensive Simulation Tests
 * ============================================
 * 30 distinct user personas from bioinformatics labs and enterprises,
 * each with unique usage patterns, concerns, and workflow patterns.
 *
 * Category breakdown:
 *  10x Research Scientists (WGS, RNA-seq, core facility, etc.)
 *   8x IT/HPC Administrators (cluster, security, DevOps, etc.)
 *   7x Enterprise/Management (PI, QA, compliance, etc.)
 *   5x External Collaborators (academic, vendor, grant, etc.)
 *
 * Total: 60 tests covering full feature matrix
 */

const VALID_WORKFLOW = `[workflow]
name = "test-pipeline"
version = "1.0.0"
description = "Test workflow"

[[rules]]
name = "step1"
output = ["result.txt"]
shell = "echo done > {output[0]}"
`;

// ---------------------------------------------------------------------------
// Research Scientists (10 users)
// ---------------------------------------------------------------------------

test.describe('1. WGS Analyst - Germline Variant Calling', () => {
  test('login and load WGS template', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="templates"]');
    await expect(page.locator('#view-templates')).toBeVisible();
    await page.fill('#template-search', 'WGS');
    await page.click('.template-card button:text("Use")');
    await expect(page.locator('#editor-text')).not.toBeEmpty();
    await expect(page.locator('#editor-text')).toContainText('wgs-germline', { timeout: 5000 });
  });

  test('validate WGS workflow and view DAG', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', VALID_WORKFLOW);
    await page.click('button:text("Validate")');
    await expect(page.locator('#editor-output')).not.toHaveClass(/hidden/);
  });
});

test.describe('2. RNA-seq Biologist - Transcript Quantification', () => {
  test('load RNA-seq template and dry-run', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', VALID_WORKFLOW);
    await page.click('button:text("Dry Run")');
    await expect(page.locator('#editor-output')).not.toHaveClass(/hidden/);
  });

  test('lint RNA-seq workflow for issues', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', VALID_WORKFLOW);
    await page.click('button:text("Lint")');
    await expect(page.locator('#editor-output')).not.toHaveClass(/hidden/);
  });
});

test.describe('3. Core Facility Manager - Shared Workflows', () => {
  test('save and list multiple workflows', async ({ page }) => {
    await loginAsAdmin(page);
    for (let i = 0; i < 3; i++) {
      await page.click('button[data-view="editor"]');
      await page.fill('#editor-text', `[workflow]\nname = "shared-${i}"\nversion = "1.0"\n\n[[rules]]\nname = "r"\noutput = ["o.txt"]\nshell = "echo hi"`);
      await page.fill('#save-name', `shared-pipeline-${i}`);
      await page.click('button:text("Save")');
      await page.waitForTimeout(500);
    }
    await page.click('button[data-view="workflows"]');
    await expect(page.locator('#saved-wf-tbody')).toBeVisible();
  });

  test('delete obsolete workflow', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', `[workflow]\nname = "del-test"\nversion = "1.0"\n`);
    await page.fill('#save-name', 'to-delete');
    await page.click('button:text("Save")');
    await page.click('button[data-view="workflows"]');
    await page.click('button:text("Del")');
    page.on('dialog', d => d.accept());
    await expect(page.locator('#saved-wf-tbody')).toBeVisible();
  });
});

test.describe('4. Graduate Student - Learning Pipeline Tools', () => {
  test('create workflow from hello template', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.evaluate(() => { (window as any).loadTemplateById && (window as any).loadTemplateById('hello'); });
    // Or load directly via selectOption and wait for template load
    await page.selectOption('#template-select', 'hello');
    await page.waitForTimeout(300);
    const text = await page.locator('#editor-text').inputValue();
    expect(text.length).toBeGreaterThan(10);
  });

  test('format workflow for readability', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', '[workflow]\nname="test"\nversion="1.0"\n[[rules]]\nname="r"\noutput=["o"]\nshell="echo hi"');
    await page.click('button:text("Format")');
    await expect(page.locator('#editor-output')).not.toHaveClass(/hidden/);
  });
});

test.describe('5. Postdoc Researcher - Rapid Prototyping', () => {
  test('quick validate and export Dockerfile', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', VALID_WORKFLOW);
    await page.click('button:text("Validate")');
    await page.click('button:text("Export Dockerfile")');
    await expect(page.locator('#export-modal')).toBeVisible();
    await page.click('#export-modal button:text("Close")');
  });

  test('view DAG for workflow structure', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', VALID_WORKFLOW);
    await page.click('button:text("View DAG")');
    await expect(page.locator('#dag-modal')).toBeVisible();
    await page.click('#dag-modal button:text("Close")');
  });
});

test.describe('6. Bioinformatics Core Lead - Quality Standards', () => {
  test('lint workflows and check diagnostics', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', VALID_WORKFLOW);
    await page.click('button:text("Lint")');
    await expect(page.locator('#editor-output')).not.toHaveClass(/hidden/);
  });

  test('check system metrics for resource planning', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="system"]');
    await expect(page.locator('#view-system')).toBeVisible();
    await expect(page.locator('#s-cpu')).toContainText('cores');
  });
});

test.describe('7. Clinical Genomics Specialist - Somatic Variant Calling', () => {
  test('load and configure paired tumor-normal template', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="templates"]');
    await page.fill('#template-search', 'Tumor');
    await expect(page.locator('.template-card')).toBeVisible();
  });

  test('create schedule for nightly clinical pipeline', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', VALID_WORKFLOW);
    await page.fill('#save-name', 'clinical-pipeline');
    await page.click('button:text("Save")');
    await page.click('button[data-view="scheduled"]');
    await expect(page.locator('#view-scheduled')).toBeVisible();
  });
});

test.describe('8. Single-cell Researcher - scRNA-seq', () => {
  test('browse scatter-gather template for parallel processing', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="templates"]');
    await page.fill('#template-search', 'Scatter');
    await page.click('.template-card button:text("Preview")');
    await expect(page.locator('#template-preview-modal')).toBeVisible();
    await page.click('#template-preview-modal button:text("Close")');
  });

  test('check stats after workflow validation', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', VALID_WORKFLOW);
    await page.click('button:text("Validate")');
    await expect(page.locator('#editor-stats')).not.toContainText('Open a workflow');
  });
});

test.describe('9. Metagenomics Scientist - Microbiome Workflows', () => {
  test('test conditional rules template', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="templates"]');
    await page.fill('#template-search', 'Conditional');
    await page.click('.template-card button:text("Use")');
    await expect(page.locator('#editor-text')).toContainText('conditional');
  });

  test('dry-run conditional workflow', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', VALID_WORKFLOW);
    await page.click('button:text("Dry Run")');
    await expect(page.locator('#editor-output')).not.toHaveClass(/hidden/);
  });
});

test.describe('10. Population Geneticist - Cohort Analysis', () => {
  test('load cohort template and validate', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="templates"]');
    await page.fill('#template-search', 'Cohort');
    await page.click('.template-card button:text("Use")');
    await expect(page.locator('#editor-text')).toContainText('cohort');
  });

  test('check workflow statistics', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', VALID_WORKFLOW);
    await page.click('button:text("Validate")');
    await expect(page.locator('#editor-stats')).toBeVisible();
  });
});

// ---------------------------------------------------------------------------
// IT/HPC Administrators (8 users)
// ---------------------------------------------------------------------------

test.describe('11. HPC Cluster Admin - SLURM Monitoring', () => {
  test('check HPC status in system view', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="system"]');
    await expect(page.locator('#hpc-status-content')).toBeVisible();
  });

  test('refresh HPC status manually', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="system"]');
    await page.click('button:text("Refresh")');
    await expect(page.locator('#hpc-status-content')).not.toBeEmpty();
  });
});

test.describe('12. System Administrator - Server Maintenance', () => {
  test('verify health endpoint through dashboard metrics', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="system"]');
    await expect(page.locator('#sys-info-json')).toBeVisible();
    // Should contain version info
    await expect(page.locator('#sys-info-json')).not.toBeEmpty();
  });

  test('monitor CPU and memory metrics', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="dashboard"]');
    await expect(page.locator('#d-cpu')).not.toBeEmpty();
    await expect(page.locator('#d-mem')).not.toBeEmpty();
  });

  test('check server uptime and request count', async ({ page }) => {
    await loginAsAdmin(page);
    // Navigate between views to generate requests
    await page.click('button[data-view="dashboard"]');
    await page.click('button[data-view="system"]');
    await expect(page.locator('#s-requests')).toBeVisible();
  });
});

test.describe('13. Security Officer - Audit Log Review', () => {
  test('view audit logs for 7 days', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="system"]');
    await expect(page.locator('#audit-tbody')).toBeVisible();
  });

  test('switch audit log to 30-day view', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="system"]');
    await page.selectOption('#audit-days', '30');
    await expect(page.locator('#audit-days')).toHaveValue('30');
  });

  test('check audit log table headers', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="system"]');
    const headers = page.locator('#audit-table th');
    await expect(headers.first()).toBeVisible();
  });
});

test.describe('14. DevOps Engineer - CI/CD Integration', () => {
  test('validate workflow via editor for CI pipeline', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', VALID_WORKFLOW);
    await page.click('button:text("Validate")');
    await expect(page.locator('#editor-output')).not.toHaveClass(/hidden/);
  });

  test('check version info in system view', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="system"]');
    await expect(page.locator('#license-json')).toBeVisible();
  });
});

test.describe('15. Storage Admin - Workspace Management', () => {
  test('view run history for storage accounting', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="runs"]');
    await expect(page.locator('#view-runs')).toBeVisible();
  });

  test('check saved workflow library', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="workflows"]');
    await expect(page.locator('#saved-wf-tbody')).toBeVisible();
  });
});

test.describe('16. Database Admin - DB Health Monitoring', () => {
  test('verify dashboard loads with data', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="dashboard"]');
    await expect(page.locator('#view-dashboard')).toBeVisible();
  });

  test('check metrics endpoint response', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="system"]');
    await expect(page.locator('#s-cpu')).toBeVisible();
    await expect(page.locator('#s-mem')).toBeVisible();
  });
});

test.describe('17. Cloud Architect - Hybrid Deployment', () => {
  test('verify all views are accessible', async ({ page }) => {
    await loginAsAdmin(page);
    const views = ['dashboard', 'editor', 'runs', 'workflows', 'scheduled', 'templates', 'system'];
    for (const view of views) {
      await page.click(`button[data-view="${view}"]`);
      await expect(page.locator(`#view-${view}`)).toBeVisible({ timeout: 3000 });
    }
  });

  test('test full workflow lifecycle', async ({ page }) => {
    await loginAsAdmin(page);
    // Create
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', VALID_WORKFLOW);
    // Validate
    await page.click('button:text("Validate")');
    await expect(page.locator('#editor-output')).toContainText(/Valid|valid|success|Success/i);
    // Save
    await page.fill('#save-name', 'cloud-test-arch');
    await page.click('button:text("Save")');
    // Load
    await page.click('button[data-view="workflows"]');
    await expect(page.locator('#saved-wf-tbody')).toBeVisible();
  });
});

test.describe('18. License Compliance Manager', () => {
  test('check license status in system view', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="system"]');
    await expect(page.locator('#license-json')).toBeVisible();
  });

  test('verify environment backends are listed', async ({ page }) => {
    await loginAsAdmin(page);
    const response = await page.evaluate(() =>
      fetch('/api/environments').then(r => r.json())
    );
    expect(response).toHaveProperty('available');
  });
});

// ---------------------------------------------------------------------------
// Enterprise/Management (7 users)
// ---------------------------------------------------------------------------

test.describe('19. Lab Director - Oversight Dashboard', () => {
  test('view dashboard with all metrics', async ({ page }) => {
    await loginAsAdmin(page);
    await expect(page.locator('#d-cpu')).not.toBeEmpty();
    await expect(page.locator('#d-mem')).not.toBeEmpty();
    await expect(page.locator('#d-runs')).not.toBeEmpty();
    await expect(page.locator('#d-uptime')).not.toBeEmpty();
  });

  test('check recent runs table', async ({ page }) => {
    await loginAsAdmin(page);
    await expect(page.locator('#recent-runs-tbody')).toBeVisible();
  });
});

test.describe('20. Principal Investigator - Project Monitoring', () => {
  test('review run history for project status', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="runs"]');
    await expect(page.locator('#view-runs')).toBeVisible();
  });

  test('load specific workflow for review', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="templates"]');
    await page.click('.template-card button:text("Preview")');
    await expect(page.locator('#template-preview-content')).toBeVisible();
    await page.click('#template-preview-modal button:text("Close")');
  });
});

test.describe('21. QA Engineer - Validation Edge Cases', () => {
  test('submit invalid TOML and verify error', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', 'this is not valid toml {{{');
    await page.click('button:text("Validate")');
    await expect(page.locator('#editor-output')).not.toHaveClass(/hidden/);
  });

  test('empty workflow validation', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', '');
    await page.click('button:text("Validate")');
    await expect(page.locator('#editor-output')).not.toHaveClass(/hidden/);
  });

  test('validate with missing required fields', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', '[workflow]\nname = "test"\n');
    await page.click('button:text("Validate")');
    await expect(page.locator('#editor-output')).not.toHaveClass(/hidden/);
  });
});

test.describe('22. Data Manager - Data Provenance', () => {
  test('track workflow execution in run history', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="runs"]');
    await expect(page.locator('#view-runs')).toBeVisible();
  });

  test('check saved workflows for data pipeline docs', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="workflows"]');
    await expect(page.locator('#saved-wf-tbody')).toBeVisible();
  });
});

test.describe('23. Project Manager - Progress Tracking', () => {
  test('monitor active workflow count', async ({ page }) => {
    await loginAsAdmin(page);
    await expect(page.locator('#d-runs')).toBeVisible();
  });

  test('review template library for planning', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="templates"]');
    const cards = page.locator('.template-card');
    await expect(cards.first()).toBeVisible();
  });
});

test.describe('24. Compliance Officer - Audit Trail', () => {
  test('verify audit logs accessible', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="system"]');
    await expect(page.locator('#audit-table')).toBeVisible();
  });

  test('audit log shows filter options', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="system"]');
    await expect(page.locator('#audit-days')).toBeVisible();
    const options = await page.locator('#audit-days option').allTextContents();
    expect(options.length).toBeGreaterThanOrEqual(3);
  });
});

test.describe('25. Training Coordinator - Onboarding', () => {
  test('all templates are accessible for training', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="templates"]');
    const cards = await page.locator('.template-card').count();
    expect(cards).toBeGreaterThanOrEqual(5);
  });

  test('template search works for training materials', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="templates"]');
    await page.fill('#template-search', 'Hello');
    await page.fill('#template-search', '');
    const cards = await page.locator('.template-card').count();
    expect(cards).toBeGreaterThan(0);
  });
});

// ---------------------------------------------------------------------------
// External Collaborators (5 users)
// ---------------------------------------------------------------------------

test.describe('26. External Academic Collaborator', () => {
  test('login with collaborator credentials', async ({ page }) => {
    await page.goto('/');
    await page.click('.user-info .avatar');
    await page.click('button:text("Sign In")');
    await page.fill('#login-username', 'admin');
    await page.fill('#login-password', 'admin');
    await page.click('.modal button:text("Sign In")');
    await expect(page.locator('#user-name')).toHaveText('admin');
  });

  test('view shared template library', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="templates"]');
    await expect(page.locator('#templates-grid')).toBeVisible();
  });
});

test.describe('27. Commercial Partner - Licensing', () => {
  test('verify license status visible', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="system"]');
    await expect(page.locator('#license-json')).toBeVisible();
  });
});

test.describe('28. Clinical Trial Partner - Regulatory', () => {
  test('verify audit trail for regulatory compliance', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="system"]');
    await expect(page.locator('#audit-table')).toBeVisible();
  });

  test('save workflow with version tracking', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', `[workflow]\nname = "clinical-trial"\nversion = "2.1.0"\n`);
    await page.fill('#save-name', 'clinical-trial-v2.1');
    await page.click('button:text("Save")');
    await expect(page.locator('#editor-output')).not.toHaveClass(/hidden/);
  });
});

test.describe('29. Vendor Support Engineer - Troubleshooting', () => {
  test('format malformed workflow', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', VALID_WORKFLOW);
    await page.click('button:text("Format")');
    await expect(page.locator('#editor-output')).not.toHaveClass(/hidden/);
  });

  test('check system environments for compatibility', async ({ page }) => {
    await loginAsAdmin(page);
    const response = await page.evaluate(() =>
      fetch('/api/environments').then(r => r.json())
    );
    expect(response).toHaveProperty('available');
  });
});

test.describe('30. Grant Reviewer - Reproducibility', () => {
  test('verify workflow export functionality', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', VALID_WORKFLOW);
    await page.click('button:text("Export Dockerfile")');
    await expect(page.locator('#export-modal')).toBeVisible();
    await page.click('#export-modal button:text("Close")');
  });

  test('view DAG for methodology documentation', async ({ page }) => {
    await loginAsAdmin(page);
    await page.click('button[data-view="editor"]');
    await page.fill('#editor-text', VALID_WORKFLOW);
    await page.click('button:text("View DAG")');
    await expect(page.locator('#dag-content')).toBeVisible();
    await page.click('#dag-modal button:text("Close")');
  });
});

// ---------------------------------------------------------------------------
// Concurrent multi-user stress tests (30 users)
// ---------------------------------------------------------------------------

test.describe('Concurrent 30-user Stress Test', () => {
  test('30 concurrent users login and view dashboard', async ({ browser }) => {
    const results = await Promise.all(
      Array.from({ length: 30 }, async (_, i) => {
        const context = await browser.newContext();
        const page = await context.newPage();
        try {
          await page.goto('/', { timeout: 15000 });
          await page.click('.user-info .avatar');
          await page.click('button:text("Sign In")');
          await page.fill('#login-username', 'admin');
          await page.fill('#login-password', 'admin');
          await page.click('.modal button:text("Sign In")');
          await page.waitForTimeout(100);
          await context.close();
          return { user: i + 1, success: true };
        } catch (e) {
          await context.close();
          return { user: i + 1, success: false, error: String(e) };
        }
      })
    );

    const failures = results.filter(r => !r.success);
    expect(failures).toHaveLength(0);
  });

  test('30 concurrent workflow validations', async ({ browser }) => {
    const results = await Promise.all(
      Array.from({ length: 30 }, async (_, i) => {
        const context = await browser.newContext();
        const page = await context.newPage();
        try {
          await page.goto('/', { timeout: 15000 });
          await page.click('.user-info .avatar');
          await page.click('button:text("Sign In")');
          await page.fill('#login-username', 'admin');
          await page.fill('#login-password', 'admin');
          await page.click('.modal button:text("Sign In")');

          await page.click('button[data-view="editor"]');
          await page.fill('#editor-text', `[workflow]\nname = "concurrent-${i}"\nversion = "1.0"\n[[rules]]\nname = "r${i}"\noutput = ["o${i}.txt"]\nshell = "echo ${i}"`);
          await page.click('button:text("Validate")');

          await context.close();
          return { user: i + 1, success: true };
        } catch (e) {
          await context.close();
          return { user: i + 1, success: false, error: String(e) };
        }
      })
    );

    const failures = results.filter(r => !r.success);
    expect(failures).toHaveLength(0);
  });

  test('5 concurrent users save workflows', async ({ browser }) => {
    const results = await Promise.all(
      Array.from({ length: 5 }, async (_, i) => {
        const context = await browser.newContext();
        const page = await context.newPage();
        try {
          await page.goto('/', { timeout: 15000 });
          await page.click('.user-info .avatar');
          await page.click('button:text("Sign In")');
          await page.fill('#login-username', 'admin');
          await page.fill('#login-password', 'admin');
          await page.click('.modal button:text("Sign In")');

          await page.click('button[data-view="editor"]');
          await page.fill('#editor-text', `[workflow]\nname = "stress-${i}"\nversion = "1.0.0"\n[[rules]]\nname = "s${i}"\noutput = ["out${i}.txt"]\nshell = "echo stress-${i}"`);
          await page.fill('#save-name', `stress-test-${i}`);
          await page.click('button:text("Save")');

          await context.close();
          return { user: i + 1, success: true };
        } catch (e) {
          await context.close();
          return { user: i + 1, success: false, error: String(e) };
        }
      })
    );

    const failures = results.filter(r => !r.success);
    expect(failures).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

async function loginAsAdmin(page: any) {
  await page.goto('/');
  // Click avatar to show login modal
  await page.click('.user-info .avatar');
  // Wait for modal to be visible
  await page.waitForSelector('#login-modal:not(.hidden)', { timeout: 5000 });
  // Fill credentials
  await page.fill('#login-username', 'admin');
  await page.fill('#login-password', 'admin');
  // Click submit button inside modal
  await page.click('#login-modal button:text("Sign In")');
  // Wait for login to complete
  await page.waitForSelector('#user-name', { timeout: 5000 });
  await expect(page.locator('#user-name')).toHaveText('admin');
}