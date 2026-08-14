import { useCallback, useEffect, useState, useRef } from 'react';
import { Play, CheckCircle, AlertCircle, Undo2, Redo2, Save, Wand2, Blocks } from 'lucide-react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { api } from '../api/client';
import type { DagJson, KnowledgeTool } from '../api/types';
import ChatUI from '../components/ChatUI';
import ToolPalette from '../components/ToolPalette';
import WorkflowCanvas from '../components/WorkflowCanvas';
import RuleInspector from '../components/RuleInspector';
import RunDialog from '../components/RunDialog';
import TomlEditor from '../components/TomlEditor';
import GuidedRuleBuilder from '../components/GuidedRuleBuilder';
import { usePipelineSession } from '../context/PipelineSession';

const DEFAULT_TOML = `[workflow]
name = "my-pipeline"
version = "1.0.0"
description = "A sample bioinformatics pipeline"

[[rules]]
name = "fastqc"
input = ["{sample}.fastq.gz"]
output = ["qc/{sample}_fastqc.html"]
shell = "fastqc {input} -o qc/"
threads = 2

[[rules]]
name = "align"
input = ["{sample}.fastq.gz"]
output = ["bam/{sample}.bam"]
shell = "bwa mem ref/genome.fa {input} > {output}"
threads = 8
`;

interface InspectorState {
  ruleName: string;
  rule: Record<string, unknown> | null;
}

type LeftTab = 'assistant' | 'palette' | 'history';

export default function PipelineEditor() {
  const session = usePipelineSession();
  const [toml, setToml] = useState(() => session.state.pipelineToml || DEFAULT_TOML);
  const [dagJson, setDagJson] = useState<DagJson | null>(() => session.state.dagData);
  const [validation, setValidation] = useState<{ valid: boolean; errors: Array<{ code: string; message: string; rule: string | null; suggestion: string | null; line?: number | null }> } | null>(null);
  const [showErrors, setShowErrors] = useState(false);
  // Monotonic edit sequence: debounced validation and canvas edits resolve
  // out of order — only the latest request may apply its result (issue #79
  // P1-09: stale responses interleaved content into garbled state).
  const editSeq = useRef(0);
  const [running, setRunning] = useState(false);
  const [pipelineId] = useState(() => 'draft-' + Math.random().toString(36).slice(2, 9));
  const [leftTab, setLeftTab] = useState<LeftTab>('palette');
  const [inspector, setInspector] = useState<InspectorState | null>(null);
  const [showRunDialog, setShowRunDialog] = useState(false);
  // Guided vs Power modes (issue #82 P1-5): form-based rule cards by
  // default; the canvas + TOML view for power users. The choice persists.
  const [highlightLine, setHighlightLine] = useState<number | null>(null);
  const [viewMode, setViewMode] = useState<'guided' | 'canvas'>(() =>
    localStorage.getItem('oxo_editor_mode') === 'canvas' ? 'canvas' : 'guided',
  );
  const switchViewMode = (mode: 'guided' | 'canvas') => {
    localStorage.setItem('oxo_editor_mode', mode);
    setViewMode(mode);
  };
  const [revisions, setRevisions] = useState<Array<{ id: string; version: string; actor: string; created_at: string }> | null>(null);
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();

  // Load a template or a saved pipeline when navigated with
  // ?template=<id> / ?pipeline=<id> (dashboard quick-start, My Pipelines).
  useEffect(() => {
    const templateId = searchParams.get('template');
    const pipelineId = searchParams.get('pipeline');
    if (pipelineId) {
      let cancelled = false;
      api
        .getPipeline(pipelineId)
        .then((pl) => {
          if (cancelled) return;
          setToml(pl.toml_content);
          session.setRunResult({
            message: `Opened saved pipeline "${pl.name}"`,
            type: 'success',
          });
        })
        .catch((err: unknown) => {
          const msg = err instanceof Error ? err.message : 'Pipeline not found';
          session.setRunResult({ message: `Pipeline load failed: ${msg}`, type: 'error' });
        });
      return () => {
        cancelled = true;
      };
    }
    if (!templateId) return;
    let cancelled = false;
    api
      .getTemplate(templateId)
      .then((tpl) => {
        if (cancelled || !tpl.toml_content) return;
        setToml(tpl.toml_content);
        session.setRunResult({
          message: `Loaded template "${tpl.name}" — edit it on the canvas, then dry-run`,
          type: 'success',
        });
      })
      .catch((err: unknown) => {
        const msg = err instanceof Error ? err.message : 'Template not found';
        session.setRunResult({ message: `Template load failed: ${msg}`, type: 'error' });
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchParams]);

  // Sync TOML and DAG to the session context.
  useEffect(() => {
    if (session.state.pipelineToml !== toml) session.setPipelineToml(toml);
  }, [toml, session]);
  useEffect(() => {
    if (dagJson && session.state.dagData !== dagJson) session.setDagData(dagJson);
  }, [dagJson, session]);

  const updateDag = useCallback(async (content: string) => {
    const seq = ++editSeq.current;
    try {
      const [dag, val] = await Promise.all([api.buildDag(content), api.validate(content)]);
      if (seq !== editSeq.current) return; // a newer edit superseded this one
      setDagJson(dag);
      setValidation(val);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Unknown error';
      setValidation({ valid: false, errors: [{ code: 'ERROR', message: msg, rule: null, suggestion: null }] });
    }
  }, []);

  useEffect(() => {
    const timer = setTimeout(() => updateDag(toml), 300);
    return () => clearTimeout(timer);
  }, [toml, updateDag]);

  const handleRun = async (dryRun = false, options: { maxJobs: number; keepGoing: boolean; samples: string[]; targets: string[] } = { maxJobs: 4, keepGoing: false, samples: [], targets: [] }) => {
    setRunning(true);
    try {
      // Issue #79 P1-12: runs launched from an opened saved pipeline must
      // carry its pipeline_id so the backend reuses the persistent workdir —
      // checkpoint invalidation then skips up-to-date rules (incremental
      // reruns) instead of rebuilding everything from a fresh sandbox.
      const pipelineId = searchParams.get('pipeline') ?? undefined;
      const res = await api.createRun(toml, {
        max_jobs: options.maxJobs,
        dry_run: dryRun,
        keep_going: options.keepGoing,
        samples: options.samples,
        targets: options.targets,
        pipeline_id: pipelineId,
      });
      session.setRunResult({
        runId: res.run_id,
        message: `${dryRun ? 'Dry-Run' : 'Run'} started: ${res.run_id.slice(0, 8)}... | ${res.execution_plan.total_rules} rules, est. ${res.estimated_resources.estimated_duration_secs}s`,
        type: 'success',
      });
      if (!dryRun && res.run_id) {
        session.setActiveRunId(res.run_id);
        navigate(`/runs/${res.run_id}`);
      }
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Failed to start run';
      session.setRunResult({ message: `Error: ${msg}`, type: 'error' });
    }
    setRunning(false);
  };

  // Every canvas/inspector/palette edit runs through the backend command API;
  // the returned canonical TOML replaces the local state (single source of truth).
  const runEdit = async (operation: string, payload: Record<string, unknown>) => {
    const seq = ++editSeq.current;
    try {
      const res = await api.dagCommand(pipelineId, toml, operation, payload);
      if (seq !== editSeq.current) return; // superseded by a newer edit
      setToml(res.toml_content);
      const errors = res.validation_errors ?? [];
      if (!res.success && errors.length > 0) {
        session.setRunResult({ message: `Validation: ${errors.join('; ')}`, type: 'error' });
      }
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Edit failed';
      session.setRunResult({ message: `Edit failed: ${msg}`, type: 'error' });
    }
  };

  const handleConnect = (from: string, to: string) => runEdit('connect', { from, to });
  const handleRemove = (names: string[]) => {
    for (const name of names) runEdit('remove_rule', { name });
  };

  const handleAddTool = (tool: KnowledgeTool) => {
    const safeName = tool.name.replace(/[^a-zA-Z0-9_-]/g, '_');
    void runEdit('add_rule', {
      rule: {
        name: safeName,
        description: `${tool.name} ${tool.version} — ${tool.summary}`,
        input: [],
        output: [],
        shell: `${tool.name} {input} -o {output}`,
      },
    });
  };

  const handleEditRule = (name: string) => {
    const node = dagJson?.nodes.find((n) => n.id === name);
    setInspector({ ruleName: name, rule: node ? node.rule : null });
  };

  const handleInspectorSave = async (patch: Record<string, unknown>) => {
    if (!inspector) return;
    await runEdit('update_rule', { name: inspector.ruleName, patch });
    setInspector(null);
  };

  const handleUndo = async () => {
    try {
      const res = await api.dagUndo(pipelineId, toml);
      setToml(res.toml_content);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Nothing to undo';
      session.setRunResult({ message: msg, type: 'error' });
    }
  };
  const handleRedo = async () => {
    try {
      const res = await api.dagRedo(pipelineId, toml);
      setToml(res.toml_content);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Nothing to redo';
      session.setRunResult({ message: msg, type: 'error' });
    }
  };

  // ── Version history (issue #82 P1-14): snapshots of every save/update,
  // loadable into the editor and restorable via rollback. ──
  const savedPipelineId = searchParams.get('pipeline');
  useEffect(() => {
    if (leftTab === 'history' && savedPipelineId) {
      api
        .listRevisions(savedPipelineId)
        .then(setRevisions)
        .catch(() => setRevisions([]));
    }
  }, [leftTab, savedPipelineId]);

  const renderHistory = () => {
    if (!savedPipelineId) {
      return (
        <div className="empty-state" style={{ padding: '1rem 0' }}>
          Save the pipeline to enable version history — every save and
          update snapshots the previous version, and you can load or roll
          back to any of them here.
        </div>
      );
    }
    if (revisions === null) return <div className="empty-state">Loading history…</div>;
    if (revisions.length === 0) {
      return <div className="empty-state">No revisions recorded yet.</div>;
    }
    return (
      <div>
        <div style={{ fontSize: '0.8rem', color: 'var(--color-text-secondary)', marginBottom: '8px' }}>
          {revisions.length} snapshot{revisions.length === 1 ? '' : 's'} (newest first)
        </div>
        {revisions.map((r) => (
          <div key={r.id} className="dash-card" style={{ marginBottom: '8px', padding: '8px' }}>
            <div style={{ fontSize: '0.82rem', fontWeight: 600 }}>
              v{r.version} <span className="mono" style={{ fontWeight: 400, fontSize: '0.72rem' }}>{r.id.slice(0, 8)}</span>
            </div>
            <div style={{ fontSize: '0.72rem', color: 'var(--color-text-tertiary)' }}>
              {new Date(r.created_at).toLocaleString()} · {r.actor}
            </div>
            <div style={{ display: 'flex', gap: '6px', marginTop: '6px' }}>
              <button className="btn-sm" onClick={async () => {
                try {
                  const snap = await api.getRevision(savedPipelineId, r.id);
                  setToml(snap.toml_content);
                  session.setRunResult({ message: `Loaded snapshot ${r.id.slice(0, 8)} into the editor — Save to keep it`, type: 'success' });
                } catch { /* ignore */ }
              }}>Load</button>
              <button className="btn-sm" onClick={async () => {
                if (!window.confirm(`Roll this pipeline back to the snapshot from ${new Date(r.created_at).toLocaleString()}? A new revision will record the current version first.`)) return;
                try {
                  await api.rollbackPipeline(savedPipelineId, r.id);
                  setToml((await api.getPipeline(savedPipelineId)).toml_content);
                  api.listRevisions(savedPipelineId).then(setRevisions);
                  session.setRunResult({ message: 'Rolled back — the current version was preserved as a revision', type: 'success' });
                } catch { /* ignore */ }
              }}>Rollback</button>
            </div>
          </div>
        ))}
      </div>
    );
  };

  const handleSave = async () => {
    // Issue #79 P1-09: Save reported success even for TOML with 23 errors
    // or cycles. Invalid content is refused with the error list instead.
    if (validation && !validation.valid) {
      setShowErrors(true);
      session.setRunResult({
        message: `Not saved: ${validation.errors.length} validation error(s) — see the panel below`,
        type: 'error',
      });
      return;
    }
    try {
      const name = toml.match(/name\s*=\s*"([^"]+)"/)?.[1] || 'untitled-pipeline';
      const savedId = searchParams.get('pipeline');
      if (savedId) {
        // Editing an existing saved pipeline updates it in place (the old
        // behavior always created a duplicate row).
        await api.updatePipeline(savedId, { name, toml_content: toml });
        session.setRunResult({ message: `Pipeline "${name}" updated`, type: 'success' });
        return;
      }
      const res = await api.createPipeline({ name, toml_content: toml });
      session.setRunResult({ message: `Pipeline "${name}" saved (ID: ${res.id.slice(0, 8)}...)`, type: 'success' });
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Failed to save';
      session.setRunResult({ message: `Save failed: ${msg}`, type: 'error' });
    }
  };

  return (
    <div className="page">
      <div style={{ display: 'flex', alignItems: 'center', gap: '0.75rem', flexWrap: 'wrap' }}>
        <h1 className="page-title" style={{ margin: 0 }}>Pipeline Editor</h1>
        <div style={{ display: 'flex', gap: '4px' }}>
          <button
            className={viewMode === 'guided' ? 'btn-run' : 'btn-sm'}
            onClick={() => switchViewMode('guided')}
            title="Form-based rule cards — no TOML needed"
          >
            🧭 Guided
          </button>
          <button
            className={viewMode === 'canvas' ? 'btn-run' : 'btn-sm'}
            onClick={() => switchViewMode('canvas')}
            title="Canvas graph + raw TOML"
          >
            ⚡ Canvas + TOML
          </button>
        </div>
      </div>

      {viewMode === 'guided' && (
        <div style={{ marginTop: '1rem' }}>
          <GuidedRuleBuilder toml={toml} onChange={(v) => setToml(v)} />
        </div>
      )}

      <div className="editor-layout" style={{ display: viewMode === 'guided' ? 'none' : undefined }}>
        <div className="left-rail">
          <div className="left-rail-tabs" role="tablist">
            <button
              role="tab"
              aria-selected={leftTab === 'palette'}
              className={`left-rail-tab ${leftTab === 'palette' ? 'active' : ''}`}
              onClick={() => setLeftTab('palette')}
            >
              <Blocks size={14} /> Tools
            </button>
            <button
              role="tab"
              aria-selected={leftTab === 'assistant'}
              className={`left-rail-tab ${leftTab === 'assistant' ? 'active' : ''}`}
              onClick={() => setLeftTab('assistant')}
            >
              <Wand2 size={14} /> Assistant
            </button>
            <button
              role="tab"
              aria-selected={leftTab === 'history'}
              className={`left-rail-tab ${leftTab === 'history' ? 'active' : ''}`}
              onClick={() => {
                setLeftTab('history');
                setRevisions(null);
              }}
            >
              🕘 History
            </button>
          </div>
          <div className="left-rail-body">
            {leftTab === 'palette' ? (
              <ToolPalette onAddTool={handleAddTool} />
            ) : leftTab === 'history' ? (
              <div style={{ padding: '0 12px' }}>
                {renderHistory()}
              </div>
            ) : (
              <ChatUI
                context="editor"
                onPipelineReady={(data) => {
                  if (data.toml_content) setToml(data.toml_content);
                }}
              />
            )}
          </div>
        </div>

        <div className="dag-panel">
          <div className="panel-header">
            <span>Pipeline DAG</span>
            <div className="panel-actions">
              <button className="btn-sm" onClick={handleUndo} title="Undo">
                <Undo2 size={14} />
              </button>
              <button className="btn-sm" onClick={handleRedo} title="Redo">
                <Redo2 size={14} />
              </button>
              {dagJson && (
                <span className="dag-counts">
                  {dagJson.nodes.length} nodes, {dagJson.edges.length} edges
                </span>
              )}
            </div>
          </div>
          <WorkflowCanvas
            dag={dagJson}
            editable
            scopeKey={pipelineId}
            onEditRule={handleEditRule}
            onConnectRules={handleConnect}
            onRemoveRules={handleRemove}
          />
          <div className="canvas-legend">
            <span className="legend-item">
              <span className="legend-line legend-declared" /> depends_on (editable)
            </span>
            <span className="legend-item">
              <span className="legend-line legend-file" /> file-inferred (edit input/output paths to change)
            </span>
            <span className="legend-hint">Double-click a node to edit it · drag handles to connect · Del removes</span>
          </div>
        </div>

        <div className="editor-panel">
          <div className="panel-header">
            <span>Workflow TOML</span>
            <div className="panel-actions">
              {validation && (
                <button
                  className={`val-badge ${validation.valid ? 'valid' : 'invalid'}`}
                  style={{ background: 'transparent', border: 'none', cursor: validation.valid ? 'default' : 'pointer' }}
                  onClick={() => setShowErrors((v) => !v)}
                  title={validation.valid ? 'Workflow is valid' : 'Show validation errors'}
                >
                  {validation.valid ? <CheckCircle size={14} /> : <AlertCircle size={14} />}
                  {validation.valid ? ' Valid' : `${validation.errors.length} error(s)`}
                </button>
              )}
              <button onClick={handleSave} className="btn-sm" style={{ background: 'transparent', border: '1px solid var(--color-border)' }}>
                <Save size={14} /> Save
              </button>
              <button onClick={() => setShowRunDialog(true)} disabled={running || !validation?.valid} className="btn-run">
                <Play size={16} /> {running ? 'Starting...' : 'Run'}
              </button>
            </div>
          </div>
          {showErrors && validation && !validation.valid && (
            <div className="validation-errors">
              {validation.errors.map((e, i) => (
                <div key={i} className="validation-error-row">
                  <span className="validation-error-code">{e.code}</span>
                  {e.line != null && (
                    <button className="btn-sm" style={{ fontSize: '0.7rem' }}
                      title="Jump to this line in the editor"
                      onClick={() => setHighlightLine(e.line!)}>
                      line {e.line}
                    </button>
                  )}
                  <div className="validation-error-body">
                    <div>
                      {e.rule ? <strong>{e.rule}: </strong> : null}
                      {e.message}
                    </div>
                    {e.suggestion && <div className="validation-error-suggestion">→ {e.suggestion}</div>}
                  </div>
                </div>
              ))}
            </div>
          )}
          <TomlEditor value={toml} onChange={(v) => setToml(v)} highlightLine={highlightLine} />
        </div>
      </div>

      {session.state.lastRunResult && (
        <div
          className={`result-bar ${session.state.lastRunResult.type}`}
          style={{ cursor: 'pointer' }}
          onClick={() => session.setRunResult(null)}
        >
          {session.state.lastRunResult.message}
          <span style={{ marginLeft: 'auto', fontSize: '0.7rem', opacity: 0.7 }}>click to dismiss</span>
        </div>
      )}

      {showRunDialog && (
        <RunDialog
          onClose={() => setShowRunDialog(false)}
          onSubmit={(dryRun, options) => {
            setShowRunDialog(false);
            void handleRun(dryRun, options);
          }}
        />
      )}

      {inspector && (
        <RuleInspector
          key={inspector.ruleName}
          ruleName={inspector.ruleName}
          rule={inspector.rule}
          onSave={handleInspectorSave}
          onClose={() => setInspector(null)}
        />
      )}
    </div>
  );
}
