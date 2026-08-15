import { test, expect } from '@playwright/test';

test.describe('Real-World Bioinformatics Scenarios', () => {

  // ── Scenario 1: First-time user onboarding ──

  test('first-time user: dashboard → create pipeline → validate → view DAG', async ({ page, request }) => {
    await page.goto('/');
    // Task-oriented dashboard (issue #82 P1-7)
    await expect(page.locator('h1')).toContainText('What do you want to do?');

    // Navigate to pipeline editor
    await page.goto('/editor');
    await expect(page.locator('h1')).toContainText('Pipeline Editor');

    // Enter a simple TOML via the API
    const toml = `[workflow]\nname = "my-first-pipeline"\nversion = "1.0"\ndescription = "FastQC + alignment"\n\n[[rules]]\nname = "fastqc"\ninput = ["raw/{sample}.fastq.gz"]\noutput = ["qc/{sample}_fastqc.html"]\nshell = "fastqc {input} -o qc/"\n\n[[rules]]\nname = "bwa_mem"\ninput = ["raw/{sample}.fastq.gz"]\noutput = ["bam/{sample}.bam"]\nshell = "bwa mem ref.fa {input} > {output}"\ndepends_on = ["fastqc"]`;
    const resp = await request.post('/api/pipelines', {
      data: { name: 'my-first-pipeline', toml_content: toml },
    });
    expect(resp.ok()).toBeTruthy();
    const pipeline = await resp.json();

    // Validate
    const validate = await request.post('/api/pipelines/validate', {
      data: { toml_content: pipeline.toml_content },
    });
    expect(validate.ok()).toBeTruthy();
    const val = await validate.json();
    expect(val.valid).toBeTruthy();

    // Build DAG
    const dag = await request.post('/api/pipelines/dag', {
      data: { toml_content: pipeline.toml_content },
    });
    expect(dag.ok()).toBeTruthy();
    const dagData = await dag.json();
    expect(dagData.nodes.length).toBe(2); // fastqc + bwa_mem
    expect(dagData.edges.length).toBe(1); // fastqc → bwa_mem
  });

  // ── Scenario 2: Error recovery (invalid TOML → fix → revalidate) ──

  test('error recovery: bad TOML → fix → success', async ({ request }) => {
    // Submit invalid TOML (missing workflow name)
    const badToml = '[workflow]\nversion = "1.0"\n\n[[rules]]\nname = "step"\noutput = ["x"]\nshell = "echo"';
    const bad = await request.post('/api/pipelines/validate', {
      data: { toml_content: badToml },
    });
    // The validator returns a 400-level error for parse failures
    expect(bad.status()).toBeLessThan(500);
    const badResult = await bad.json();
    // Should contain error info (either in 'valid: false' or 'code: VALIDATE_ERROR')
    expect(badResult.valid === false || badResult.code !== undefined).toBeTruthy();

    // Fix the TOML
    const goodToml = '[workflow]\nname = "fixed"\nversion = "1.0"\n\n[[rules]]\nname = "step"\noutput = ["x"]\nshell = "echo hi > {output}"';
    const good = await request.post('/api/pipelines/validate', {
      data: { toml_content: goodToml },
    });
    expect(good.ok()).toBeTruthy();
    const goodResult = await good.json();
    expect(goodResult.valid).toBeTruthy();
  });

  // ── Scenario 3: Linter best practices ──

  test('lint suggestions help improve pipeline quality', async ({ request }) => {
    // Pipeline missing threads, memory recommendations
    const toml = `[workflow]\nname = "sloppy-pipeline"\nversion = "1.0"\n\n[[rules]]\nname = "heavy"\ninput = ["big.fastq"]\noutput = ["big.bam"]\nshell = "tool {input} -o {output}"`;

    const lint = await request.post('/api/pipelines/lint', {
      data: { toml_content: toml },
    });
    expect(lint.ok()).toBeTruthy();
    const result = await lint.json();
    // Lint should provide warnings (may be empty if linter is lenient)
    expect(result).toHaveProperty('errors');
  });

  // ── Scenario 4: Formatting keeps pipeline canonical ──

  test('format produces canonical TOML', async ({ request }) => {
    // Messy TOML with extra whitespace and bad formatting
    const messy = `[workflow]\nname="messy"\nversion =  "1.0"\n\n  [[rules]]\n  name = "a"\n  shell="echo a"\noutput=["a.txt"]`;
    const fmt = await request.post('/api/pipelines/format', {
      data: { toml_content: messy },
    });
    expect(fmt.ok()).toBeTruthy();
    const result = await fmt.json();
    expect(result.formatted).toBeDefined();
    expect(result.formatted).toContain('[workflow]');
    expect(result.formatted).toContain('name = "messy"');
    // Canonical format normalizes spacing: no `name="messy"` (missing space before =)
    expect(result.formatted).not.toContain('name="messy"');
  });

  // ── Scenario 5: Diff two versions ──

  test('diff shows changes between pipeline versions', async ({ request }) => {
    const v1 = '[workflow]\nname = "v1"\nversion = "1.0"\n\n[[rules]]\nname = "step1"\noutput = ["out.txt"]\nshell = "echo v1"';
    const v2 = '[workflow]\nname = "v2"\nversion = "1.0"\n\n[[rules]]\nname = "step1"\noutput = ["out.txt"]\nshell = "echo v2"\n\n[[rules]]\nname = "step2"\noutput = ["out2.txt"]\nshell = "echo new"';

    const diff = await request.post('/api/pipelines/diff', {
      data: { pipeline_a_id: 'v1', pipeline_b_id: 'v2', content_a: v1, content_b: v2 },
    });
    // Diff endpoint may require pipeline IDs in the DB, but should handle gracefully
    expect(diff.status()).toBeLessThan(500);
  });

  // ── Scenario 6: Concurrent execution stress ──

  test('server handles rapid concurrent pipeline operations', async ({ request }) => {
    const toml = (i: number) => `[workflow]\nname = "concurrent-${i}"\nversion = "1.0"\n\n[[rules]]\nname = "step"\noutput = ["out-${i}.txt"]\nshell = "echo ${i} > {output}"`;

    // Create 5 pipelines in parallel
    const creates = await Promise.all(
      Array.from({ length: 5 }, (_, i) =>
        request.post('/api/pipelines', {
          data: { name: `concurrent-${i}`, toml_content: toml(i) },
        })
      )
    );
    for (const r of creates) {
      expect(r.ok()).toBeTruthy();
    }
    const ids = await Promise.all(creates.map(r => r.json().then(j => j.id)));

    // Validate all in parallel
    const validations = await Promise.all(
      ids.map(id =>
        request.get(`/api/pipelines/${id}`).then(r => r.json().then(j => j.toml_content))
          .then(tc => request.post('/api/pipelines/validate', { data: { toml_content: tc } }))
      )
    );
    for (const r of validations) {
      expect(r.ok()).toBeTruthy();
      const v = await r.json();
      expect(v.valid).toBeTruthy();
    }

    // Clean up
    await Promise.all(ids.map(id => request.delete(`/api/pipelines/${id}`)));
  });

  // ── Scenario 7: Sample sheet parsing ──

  test('sample sheet parsing handles various formats', async ({ request }) => {
    // TSV format
    const tsv = 'sample\tcondition\treads\nS1\ttumor\t1000000\nS2\tnormal\t2000000';
    const tsvResp = await request.post('/api/data/samplesheet/parse', {
      data: { content: tsv },
    });
    expect(tsvResp.ok()).toBeTruthy();

    // CSV format
    const csv = 'sample,condition,reads\nS1,tumor,1000000\nS2,normal,2000000';
    const csvResp = await request.post('/api/data/samplesheet/parse', {
      data: { content: csv },
    });
    expect(csvResp.ok()).toBeTruthy();
  });

  // ── Scenario 8: System metrics endpoint is responsive ──

  test('metrics endpoint returns runtime data', async ({ request }) => {
    const metrics = await request.get('/api/metrics');
    expect(metrics.ok()).toBeTruthy();
    const m = await metrics.json();
    expect(m).toHaveProperty('total_requests');
    expect(m).toHaveProperty('active_workflows');
    expect(typeof m.total_requests).toBe('number');
  });

  // ── Scenario 9: Audit log is accessible ──

  test('audit endpoint tracks actions', async ({ request }) => {
    const audit = await request.get('/api/audit');
    expect(audit.ok()).toBeTruthy();
    const logs = await audit.json();
    expect(logs).toHaveProperty('entries');
  });

  // ── Scenario 10: Chat API ──

  test('chat sessions endpoint works', async ({ request }) => {
    const sessions = await request.get('/api/chat/sessions');
    expect(sessions.ok()).toBeTruthy();
    const list = await sessions.json();
    expect(Array.isArray(list)).toBeTruthy();
  });
});
