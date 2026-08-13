import { useCallback, useEffect, useState } from 'react';
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

type LeftTab = 'assistant' | 'palette';

export default function PipelineEditor() {
  const session = usePipelineSession();
  const [toml, setToml] = useState(() => session.state.pipelineToml || DEFAULT_TOML);
  const [dagJson, setDagJson] = useState<DagJson | null>(() => session.state.dagData);
  const [validation, setValidation] = useState<{ valid: boolean; errors: Array<{ code: string; message: string; rule: string | null; suggestion: string | null }> } | null>(null);
  const [running, setRunning] = useState(false);
  const [pipelineId] = useState(() => 'draft-' + Math.random().toString(36).slice(2, 9));
  const [leftTab, setLeftTab] = useState<LeftTab>('palette');
  const [inspector, setInspector] = useState<InspectorState | null>(null);
  const [showRunDialog, setShowRunDialog] = useState(false);
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();

  // Load a template when navigated with ?template=<id> (dashboard quick-start).
  useEffect(() => {
    const templateId = searchParams.get('template');
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
    try {
      const [dag, val] = await Promise.all([api.buildDag(content), api.validate(content)]);
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
      const res = await api.createRun(toml, {
        max_jobs: options.maxJobs,
        dry_run: dryRun,
        keep_going: options.keepGoing,
        samples: options.samples,
        targets: options.targets,
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
    try {
      const res = await api.dagCommand(pipelineId, toml, operation, payload);
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
      const res = await api.dagUndo(pipelineId);
      setToml(res.toml_content);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Nothing to undo';
      session.setRunResult({ message: msg, type: 'error' });
    }
  };
  const handleRedo = async () => {
    try {
      const res = await api.dagRedo(pipelineId);
      setToml(res.toml_content);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Nothing to redo';
      session.setRunResult({ message: msg, type: 'error' });
    }
  };

  const handleSave = async () => {
    try {
      const name = toml.match(/name\s*=\s*"([^"]+)"/)?.[1] || 'untitled-pipeline';
      const res = await api.createPipeline({ name, toml_content: toml });
      session.setRunResult({ message: `Pipeline "${name}" saved (ID: ${res.id.slice(0, 8)}...)`, type: 'success' });
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Failed to save';
      session.setRunResult({ message: `Save failed: ${msg}`, type: 'error' });
    }
  };

  return (
    <div className="page">
      <h1 className="page-title">Pipeline Editor</h1>

      <div className="editor-layout">
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
          </div>
          <div className="left-rail-body">
            {leftTab === 'palette' ? (
              <ToolPalette onAddTool={handleAddTool} />
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
                <span className={`val-badge ${validation.valid ? 'valid' : 'invalid'}`}>
                  {validation.valid ? <CheckCircle size={14} /> : <AlertCircle size={14} />}
                  {validation.valid ? ' Valid' : `${validation.errors.length} error(s)`}
                </span>
              )}
              <button onClick={handleSave} className="btn-sm" style={{ background: 'transparent', border: '1px solid var(--color-border)' }}>
                <Save size={14} /> Save
              </button>
              <button onClick={() => setShowRunDialog(true)} disabled={running || !validation?.valid} className="btn-run">
                <Play size={16} /> {running ? 'Starting...' : 'Run'}
              </button>
            </div>
          </div>
          <TomlEditor value={toml} onChange={(v) => setToml(v)} />
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
