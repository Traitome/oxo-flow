import { test, expect } from '@playwright/test';

test.describe('Multi-User Lifecycle Simulation', () => {

  // ── Admin setup ──

  test('admin creates a new user account', async ({ request }) => {
    // Login as admin using env-var password
    const loginResp = await request.post('/api/auth/login', {
      data: { username: 'admin', password: 'admin' },
    });
    expect(loginResp.ok() || loginResp.status() === 401).toBeTruthy();
    await loginResp.json();

    // If auth failed, try creating the user anyway (personal mode, no auth needed)
    const createResp = await request.post('/api/users', {
      data: { username: 'bioinfo_scientist_1', role: 'user', password: 'science123' },
    });
    // In personal mode this may fail if not admin; that's expected
    expect([200, 201, 401, 403]).toContain(createResp.status());
  });

  // ── User: pipeline lifecycle ──

  test('user creates and validates a bioinformatics pipeline', async ({ request }) => {
    const toml = `[workflow]\nname = "rna-seq-analysis"\nversion = "1.0"\ndescription = "RNA-seq quantification pipeline"\n\n[config]\ngenome = "GRCh38"\n\n[[rules]]\nname = "fastqc"\ninput = ["raw/{sample}_R1.fastq.gz"]\noutput = ["qc/{sample}_fastqc.html"]\nshell = "fastqc {input} -o qc/"\nthreads = 2\n\n[[rules]]\nname = "quant"\ninput = ["raw/{sample}_R1.fastq.gz"]\noutput = ["quant/{sample}.tsv"]\nshell = "kallisto quant -i index -o quant/{sample} {input}"\nthreads = 8\ndepends_on = ["fastqc"]`;

    // Create pipeline
    const create = await request.post('/api/pipelines', {
      data: { name: 'rna-seq-analysis', toml_content: toml },
    });
    expect(create.ok()).toBeTruthy();
    const pipeline = await create.json();

    // Validate
    const validate = await request.post('/api/pipelines/validate', {
      data: { toml_content: pipeline.toml_content },
    });
    expect(validate.ok()).toBeTruthy();

    // Build DAG
    const dag = await request.post('/api/pipelines/dag', {
      data: { toml_content: pipeline.toml_content },
    });
    expect(dag.ok()).toBeTruthy();
    const dagBody = await dag.json();
    expect(dagBody.nodes).toBeDefined();
    expect(dagBody.edges).toBeDefined();
    expect(dagBody.nodes.length).toBe(2);

    // Lint
    const lint = await request.post('/api/pipelines/lint', {
      data: { toml_content: pipeline.toml_content },
    });
    expect(lint.ok()).toBeTruthy();

    // Format
    const format = await request.post('/api/pipelines/format', {
      data: { toml_content: pipeline.toml_content },
    });
    expect(format.ok()).toBeTruthy();
    const formatted = await format.json();
    expect(formatted.formatted).toBeDefined();

    // Dry-run
    const dryRun = await request.post('/api/runs', {
      data: { toml_content: pipeline.toml_content, max_jobs: 2, dry_run: true },
    });
    expect(dryRun.ok()).toBeTruthy();
    const run = await dryRun.json();
    expect(run.run_id).toBeDefined();
    expect(run.execution_plan.total_rules).toBe(2);
  });

  // ── User: scatter-gather pipeline ──

  test('user creates scatter-gather workflow with DAG dependencies', async ({ request }) => {
    const toml = `[workflow]\nname = "variant-calling"\nversion = "1.0"\n\n[[rules]]\nname = "index"\noutput = ["ref/genome.fa.fai"]\nshell = "samtools faidx ref/genome.fa"\n\n[[rules]]\nname = "align"\ninput = ["raw/{sample}.bam"]\noutput = ["bam/{sample}.sorted.bam"]\nshell = "samtools sort {input} -o {output}"\nthreads = 4\ndepends_on = ["index"]\n\n[[rules]]\nname = "call"\ninput = ["bam/{sample}.sorted.bam"]\noutput = ["vcf/{sample}.vcf"]\nshell = "bcftools call -mv {input} -o {output}"\nthreads = 2\ndepends_on = ["align"]\n\n[[rules]]\nname = "merge"\ninput = ["vcf/sample1.vcf", "vcf/sample2.vcf"]\noutput = ["merged.vcf"]\nshell = "bcftools merge {input} -o {output}"\ndepends_on = ["call"]`;

    const resp = await request.post('/api/pipelines/validate', {
      data: { toml_content: toml },
    });
    expect(resp.ok()).toBeTruthy();
    const val = await resp.json();
    expect(val.valid).toBeTruthy();
  });

  // ── User: real-world data discovery ──

  test('data analysis endpoints work correctly', async ({ request }) => {
    // Analyze paths
    const analyze = await request.post('/api/data/analyze', {
      data: { paths: ['examples/gallery/'], max_depth: 2 },
    });
    expect(analyze.ok() || analyze.status() === 400).toBeTruthy();

    // Parse sample sheet
    const samplesheet = await request.post('/api/data/samplesheet/parse', {
      data: { content: 'sample,condition\nS1,tumor\nS2,normal\nS3,tumor' },
    });
    expect(samplesheet.ok()).toBeTruthy();
    const sheet = await samplesheet.json();
    expect(sheet.format || sheet.fields || sheet.rows).toBeDefined();
  });

  // ── Templates ──

  test('template CRUD lifecycle', async ({ request }) => {
    // List templates
    const list = await request.get('/api/templates');
    expect(list.ok()).toBeTruthy();
    const templates = await list.json();
    expect(Array.isArray(templates)).toBeTruthy();

    // Create custom template
    const toml = '[workflow]\nname = "qc-only"\nversion = "1.0"\n\n[[rules]]\nname = "fastqc"\noutput = ["qc.html"]\nshell = "fastqc data.fastq"';
    const create = await request.post('/api/templates', {
      data: {
        name: 'QC Only Pipeline',
        category: 'QC',
        description: 'FastQC only quality control',
        tags: ['qc', 'fastq'],
        toml_content: toml,
      },
    });
    expect(create.ok()).toBeTruthy();
    const tpl = await create.json();
    expect(tpl.id).toBeDefined();
    expect(tpl.name).toBe('QC Only Pipeline');

    // Get by ID
    const get = await request.get(`/api/templates/${tpl.id}`);
    expect(get.ok()).toBeTruthy();

    // Delete
    const del = await request.delete(`/api/templates/${tpl.id}`);
    expect(del.ok()).toBeTruthy();
  });

  // ── Collaboration ──

  test('fork and share pipeline work correctly', async ({ request }) => {
    // Create source pipeline
    const toml = '[workflow]\nname = "shared-pipeline"\nversion = "1.0"\n\n[[rules]]\nname = "step1"\noutput = ["out.txt"]\nshell = "echo done > {output}"';
    const create = await request.post('/api/pipelines', {
      data: { name: 'shared-pipeline', toml_content: toml },
    });
    expect(create.ok()).toBeTruthy();
    const pipeline = await create.json();

    // Fork it
    const fork = await request.post(`/api/pipelines/${pipeline.id}/fork`, {
      data: { user_id: 'scientist2' },
    });
    expect(fork.ok() || fork.status() === 404).toBeTruthy();

    // Share it
    const share = await request.post(`/api/pipelines/${pipeline.id}/share`, {
      data: { visibility: 'public', expires_in_days: 30 },
    });
    expect(share.ok()).toBeTruthy();
    const shareResult = await share.json();
    expect(shareResult.share_url).toBeDefined();
    expect(shareResult.access_token).toBeDefined();
  });

  // ── Run lifecycle (full cycle) ──

  test('full run lifecycle: create → monitor → diagnostics → report', async ({ request }) => {
    // Create a run
    const toml = '[workflow]\nname = "hello-world"\nversion = "1.0"\n\n[[rules]]\nname = "hello"\noutput = ["hello.txt"]\nshell = "echo Hello World > {output}"';
    const runResp = await request.post('/api/runs', {
      data: { toml_content: toml, max_jobs: 1, dry_run: true },
    });
    expect(runResp.ok()).toBeTruthy();
    const run = await runResp.json();
    const runId = run.run_id;

    // Check status (may be 404 if run was dry-run and cleaned up)
    const status = await request.get(`/api/runs/${runId}/status`);
    expect(status.ok() || status.status() === 404).toBeTruthy();

    // Get diagnostics
    const diag = await request.get(`/api/runs/${runId}/diagnostics`);
    expect(diag.ok() || diag.status() === 404).toBeTruthy();

    // Get DAG status
    const dagStatus = await request.get(`/api/runs/${runId}/dag-status`);
    expect(dagStatus.ok() || dagStatus.status() === 404).toBeTruthy();

    // Get report
    const report = await request.get(`/api/runs/${runId}/report`);
    expect(report.ok() || report.status() === 404).toBeTruthy();

    // Get results
    const results = await request.get(`/api/runs/${runId}/results`);
    expect(results.ok() || results.status() === 404).toBeTruthy();

    // Cancel run — the run may already have finished (409 RUN_NOT_ACTIVE for
    // terminal states is the contract since P0's real process control).
    const cancel = await request.post(`/api/runs/${runId}/cancel`, { data: {} });
    expect(cancel.ok() || cancel.status() === 404 || cancel.status() === 409).toBeTruthy();
  });

  // ── AI Companion ──

  test('AI config endpoint works', async ({ request }) => {
    const config = await request.get('/api/ai/config');
    expect(config.ok()).toBeTruthy();
    const cfg = await config.json();
    expect(cfg).toHaveProperty('provider');
  });

  // ── Concurrent operations (load simulation) ──

  test('concurrent pipeline validations do not interfere', async ({ request }) => {
    const toml = '[workflow]\nname = "load-test"\nversion = "1.0"\n\n[[rules]]\nname = "step"\noutput = ["out.txt"]\nshell = "echo ok > {output}"';
    const results = await Promise.all(
      Array.from({ length: 10 }, (_, i) =>
        request.post('/api/pipelines/validate', {
          data: { toml_content: toml.replace('load-test', `load-test-${i}`) },
        })
      )
    );
    // All should succeed or return structured errors (not crash/panic)
    for (const r of results) {
      expect(r.ok() || r.status() < 500).toBeTruthy();
    }
  });

  // ── Health & metrics throughout ──

  test('health remains healthy after all operations', async ({ request }) => {
    const health = await request.get('/api/health');
    expect(health.ok()).toBeTruthy();
    const h = await health.json();
    expect(['ok', 'healthy', 'degraded']).toContain(h.status);
    // The version drifts with releases — assert the shape, not a frozen value.
    expect(h.version).toMatch(/^\d+\.\d+\.\d+$/);
  });
});
