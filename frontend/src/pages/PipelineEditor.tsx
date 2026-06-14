import { useState, useCallback, useEffect, lazy, Suspense } from 'react';
import { Play, CheckCircle, AlertCircle, Undo2, Redo2, Plus, Trash2, Link as LinkIcon } from 'lucide-react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import type { DagJson } from '../api/types';
import ChatUI from '../components/ChatUI';

// Lazy-loaded components for bundle optimization
const TomlEditor = lazy(() => import('../components/TomlEditor'));
const DagView = lazy(() => import('../components/DagView'));

const EditorFallback = () => <div style={{ height: '100%', display: 'flex', alignItems: 'center', justifyContent: 'center', color: 'var(--color-text-tertiary)', fontSize: '0.85rem' }}>Loading editor...</div>;
const DagFallback = () => <div className="empty-state">Loading DAG view...</div>;

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

[[rules]]
name = "call_variants"
input = ["bam/{sample}.bam"]
output = ["vcf/{sample}.vcf.gz"]
shell = "bcftools mpileup -f ref/genome.fa {input} | bcftools call -mv -o {output}"
threads = 4
`;

export default function PipelineEditor() {
  const [toml, setToml] = useState(DEFAULT_TOML);
  const [dagJson, setDagJson] = useState<DagJson | null>(null);
  const [validation, setValidation] = useState<{ valid: boolean; errors: Array<{ code: string; message: string; rule: string | null; suggestion: string | null }> } | null>(null);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<{ runId?: string; message: string } | null>(null);
  const [pipelineId] = useState(() => 'draft-' + Math.random().toString(36).slice(2, 9));
  const navigate = useNavigate();
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);

  const updateDag = useCallback(async (content: string) => {
    try {
      const [dag, val] = await Promise.all([
        api.buildDag(content),
        api.validate(content),
      ]);
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

  const handleRun = async (dryRun = false) => {
    setRunning(true);
    setResult(null);
    try {
      const res = await api.createRun(toml, 4, dryRun);
      setResult({ runId: res.id, message: `${dryRun ? 'Dry ' : ''}Run started: ${res.id.slice(0, 8)}...` });
      if (!dryRun && res.id) {
         navigate(`/runs/${res.id}`);
      }
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Failed to start run';
      setResult({ message: `Error: ${msg}` });
    }
    setRunning(false);
  };

  const handleDagEdit = async (operation: string, payload: any) => {
    try {
      const res = await api.dagCommand(pipelineId, 'dag_editor', operation, payload);
      if (res.success) {
        setToml(res.toml_content);
      } else {
        alert("Edit failed: " + (res.validation_errors || []).join(', '));
      }
    } catch (e: any) { alert("Error: " + e.message); }
  };
  const handleUndo = async () => { try { const res = await api.dagUndo(pipelineId); setToml(res.toml_content); } catch (e: any) { alert(e.message); } };
  const handleRedo = async () => { try { const res = await api.dagRedo(pipelineId); setToml(res.toml_content); } catch (e: any) { alert(e.message); } };

  return (
    <div className="page">
      <h1 className="page-title">Pipeline Editor</h1>

      <div className="editor-layout" style={{ display: 'grid', gridTemplateColumns: '300px 1fr 1fr', gap: '16px', height: '80vh' }}>
        <div className="chat-panel" style={{ overflow: 'hidden' }}>
           <ChatUI onPipelineReady={(data) => {
              if (data.toml_content) setToml(data.toml_content);
           }} />
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
              <button onClick={() => handleRun(true)} disabled={running || !validation?.valid} className="btn-sm" style={{ background: 'transparent', border: '1px solid var(--color-border)' }}>
                <CheckCircle size={14} /> Dry-Run
              </button>
              <button onClick={() => handleRun(false)} disabled={running || !validation?.valid} className="btn-run">
                <Play size={16} /> {running ? 'Starting...' : 'Run'}
              </button>
            </div>
          </div>
          <Suspense fallback={<EditorFallback />}>
            <TomlEditor value={toml} onChange={(v) => setToml(v)} />
          </Suspense>
        </div>
        <div className="dag-panel">
          <div className="panel-header">
            <span>Pipeline DAG</span>
            <div className="panel-actions">
              <button className="btn-sm" onClick={handleUndo} title="Undo"><Undo2 size={14}/></button>
              <button className="btn-sm" onClick={handleRedo} title="Redo"><Redo2 size={14}/></button>
              <button className="btn-sm" onClick={() => {
                const name = prompt("Rule name?");
                if (name) handleDagEdit('add_rule', { rule: { name, shell: 'echo TODO' } });
              }} title="Add Node"><Plus size={14}/></button>
              {selectedNodeId && (
                <>
                  <button className="btn-sm btn-error" onClick={() => {
                    if (confirm("Delete " + selectedNodeId + "?")) handleDagEdit('remove_rule', { name: selectedNodeId });
                  }} title="Delete"><Trash2 size={14}/></button>
                  <button className="btn-sm" onClick={() => {
                    const tgt = prompt("Target node name?");
                    if (tgt) handleDagEdit('connect', { source: selectedNodeId, target: tgt });
                  }} title="Connect"><LinkIcon size={14}/></button>
                </>
              )}
              {dagJson && <span className="dag-counts">{dagJson.nodes.length} nodes, {dagJson.edges.length} edges</span>}
            </div>
          </div>
          <Suspense fallback={<DagFallback />}>
            {dagJson ? (
              <DagView nodes={dagJson.nodes} edges={dagJson.edges} onNodeClick={setSelectedNodeId} />
            ) : (
              <div className="empty-state">Enter valid TOML to see the DAG</div>
            )}
          </Suspense>
        </div>
      </div>

      {result && (
        <div className={`result-bar ${result.runId ? 'success' : result.message.startsWith('Error') ? 'error' : 'info'}`}>
          {result.message}
        </div>
      )}
    </div>
  );
}
