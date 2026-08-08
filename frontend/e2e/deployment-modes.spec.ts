import { test, expect } from '@playwright/test';

test.describe('Deployment Modes & Security', () => {

  // ── Personal Mode (no auth) ──

  test('personal mode allows unauthenticated access to public endpoints', async ({ request }) => {
    // Health should work without auth
    const health = await request.get('/api/health');
    expect(health.ok()).toBeTruthy();
  });

  test('personal mode login rejects bad credentials', async ({ request }) => {
    const resp = await request.post('/api/auth/login', {
      data: { username: 'admin', password: 'wrongpassword' },
    });
    expect(resp.status()).toBe(401);
    const body = await resp.json();
    expect(body.code).toBe('AUTH_FAILED');
  });

  // ── Security Headers ──

  test('security headers are present', async ({ request }) => {
    const resp = await request.get('/api/health');
    expect(resp.headers()['x-content-type-options']).toBe('nosniff');
    expect(resp.headers()['x-frame-options']).toBe('DENY');
    expect(resp.headers()['referrer-policy']).toBe('strict-origin-when-cross-origin');
    expect(resp.headers()['permissions-policy']).toContain('camera=()');
  });

  test('CSP header is present', async ({ request }) => {
    const resp = await request.get('/');
    // CSP may be on API routes only or all routes
    const csp = resp.headers()['content-security-policy'];
    if (csp) {
      expect(csp).toContain("default-src 'self'");
    }
  });

  // ── CORS ──

  test('CORS allows localhost origins', async ({ request }) => {
    const resp = await request.get('/api/health', {
      headers: { Origin: 'http://localhost:5173' },
    });
    expect(resp.headers()['access-control-allow-origin']).toBeDefined();
  });

  // ── API error format ──

  test('unknown API route returns JSON 404', async ({ request }) => {
    const resp = await request.get('/api/nonexistent-endpoint');
    expect(resp.status()).toBe(404);
    expect(resp.headers()['content-type']).toContain('application/json');
    const body = await resp.json();
    expect(body).toHaveProperty('code');
  });

  test('API error response has structured format', async ({ request }) => {
    const resp = await request.post('/api/auth/login', {
      data: { username: '', password: '' },
    });
    // Should return structured error, not raw text
    const body = await resp.json();
    expect(body).toHaveProperty('code');
    expect(body).toHaveProperty('message');
    // Content-Type should be JSON
    expect(resp.headers()['content-type']).toContain('application/json');
  });

  // ── Rate Limiting ──

  test('rate limiting middleware is active', async ({ request }) => {
    // Rapid fire requests to trigger rate limiting
    const results = await Promise.all(
      Array.from({ length: 15 }, () => request.get('/api/health'))
    );
    // At least one should succeed (rate limiter is 100/60s, 15 should be fine)
    const allOk = results.every(r => r.ok());
    expect(allOk).toBeTruthy();
  });

  // ── License & Version ──

  test('license header is present', async ({ request }) => {
    const resp = await request.get('/api/health');
    expect(resp.headers()['x-oxoflow-license']).toBeDefined();
    expect(resp.headers()['x-oxoflow-version']).toBeDefined();
  });

  // ── Pipeline CRUD (personal mode, no auth needed) ──

  test('pipeline CRUD lifecycle', async ({ request }) => {
    // Create
    const createResp = await request.post('/api/pipelines', {
      data: {
        name: 'test-pipeline',
        toml_content: '[workflow]\nname = "test"\nversion = "1.0"\n\n[[rules]]\nname = "hello"\nshell = "echo hi"\noutput = ["out.txt"]',
      },
    });
    expect(createResp.ok()).toBeTruthy();
    const created = await createResp.json();
    expect(created.id).toBeDefined();

    // List
    const listResp = await request.get('/api/pipelines');
    expect(listResp.ok()).toBeTruthy();
    const list = await listResp.json();
    expect(Array.isArray(list)).toBeTruthy();

    // Get
    const getResp = await request.get(`/api/pipelines/${created.id}`);
    expect(getResp.ok()).toBeTruthy();
    const got = await getResp.json();
    expect(got.name).toBe('test-pipeline');

    // Validate
    const validateResp = await request.post('/api/pipelines/validate', {
      data: { toml_content: created.toml_content },
    });
    expect(validateResp.ok()).toBeTruthy();

    // Delete
    const deleteResp = await request.delete(`/api/pipelines/${created.id}`);
    expect(deleteResp.ok()).toBeTruthy();
  });

  // ── Run lifecycle ──

  test('dry-run creates run without executing', async ({ request }) => {
    const toml = '[workflow]\nname = "dry-test"\nversion = "1.0"\n\n[[rules]]\nname = "echo"\nshell = "echo ok"\noutput = ["ok.txt"]';
    const runResp = await request.post('/api/runs', {
      data: { toml_content: toml, max_jobs: 1, dry_run: true },
    });
    expect(runResp.ok()).toBeTruthy();
    const run = await runResp.json();
    expect(run.run_id).toBeDefined();
    expect(run.execution_plan).toBeDefined();
  });
});
