import { useState, useCallback, useEffect } from 'react';
import { Play, CheckCircle, AlertCircle, Wand2 } from 'lucide-react';
import { api } from '../api/client';
import DagView from '../components/DagView';
import type { DagJson, GenerateResponse } from '../api/types';

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
  const [validation, setValidation] = useState<{ valid: boolean; errors: string[] } | null>(null);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<{ runId?: string; message: string } | null>(null);
  const [intent, setIntent] = useState('');

  const updateDag = useCallback(async (content: string) => {
    try {
      const [dag, val] = await Promise.all([
        api.buildDagJson(content),
        api.validate(content),
      ]);
      setDagJson(dag);
      setValidation(val);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Unknown error';
      setValidation({ valid: false, errors: [msg] });
    }
  }, []);

  useEffect(() => {
    const timer = setTimeout(() => updateDag(toml), 300);
    return () => clearTimeout(timer);
  }, [toml, updateDag]);

  const handleRun = async () => {
    setRunning(true);
    setResult(null);
    try {
      const res = await api.run(toml);
      setResult({ runId: res.run_id, message: `Run started: ${res.run_id.slice(0, 8)}...` });
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Failed to start run';
      setResult({ message: `Error: ${msg}` });
    }
    setRunning(false);
  };

  const handleGenerate = async () => {
    if (!intent.trim()) return;
    setRunning(true);
    try {
      const gen: GenerateResponse = await api.generate(intent);
      setToml(gen.toml_content);
      setResult({ message: `Generated: ${gen.workflow_name} (${gen.rules_count} rules)` });
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Generation failed';
      setResult({ message: `Error: ${msg}` });
    }
    setRunning(false);
  };

  return (
    <div className="page">
      <h1 className="page-title">Pipeline Editor</h1>

      <div className="generate-bar">
        <input
          type="text"
          placeholder="Describe your pipeline (e.g., 'RNA-seq differential expression')"
          value={intent}
          onChange={(e) => setIntent(e.target.value)}
          className="generate-input"
          onKeyDown={(e) => e.key === 'Enter' && handleGenerate()}
        />
        <button onClick={handleGenerate} disabled={running || !intent.trim()} className="btn-gen">
          <Wand2 size={16} /> Generate
        </button>
      </div>

      <div className="editor-layout">
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
              <button onClick={handleRun} disabled={running || !validation?.valid} className="btn-run">
                <Play size={16} /> {running ? 'Starting...' : 'Run'}
              </button>
            </div>
          </div>
          <textarea
            className="toml-editor"
            value={toml}
            onChange={(e) => setToml(e.target.value)}
            spellCheck={false}
          />
        </div>
        <div className="dag-panel">
          <div className="panel-header">
            <span>Pipeline DAG</span>
            {dagJson && <span className="dag-counts">{dagJson.nodes.length} nodes, {dagJson.edges.length} edges</span>}
          </div>
          {dagJson ? (
            <DagView nodes={dagJson.nodes} edges={dagJson.edges} />
          ) : (
            <div className="empty-state">Enter valid TOML to see the DAG</div>
          )}
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
