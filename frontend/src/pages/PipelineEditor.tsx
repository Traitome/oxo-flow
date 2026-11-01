import { useCallback, useEffect, useState, useRef, lazy, Suspense } from 'react';
import { Play, CheckCircle, AlertCircle, Undo2, Redo2, Save, Wand2, Blocks, History, Maximize2, Minimize2, PanelLeftOpen, PanelLeftClose, Sparkles, Shield, FileSpreadsheet, AlignLeft, X, Upload } from 'lucide-react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { api, describeApiError } from '../api/client';
import type { DagJson, KnowledgeTool, SamplesheetParseResponse, UploadedFileEntry, VerifyDataResponse } from '../api/types';
import ChatUI from '../components/ChatUI';
import ErrorBoundary from '../components/ErrorBoundary';
import ToolPalette from '../components/ToolPalette';
import RuleInspector from '../components/RuleInspector';
import RunDialog, { type RunOptions } from '../components/RunDialog';
import GuidedRuleBuilder from '../components/GuidedRuleBuilder';
import { usePipelineSession } from '../context/PipelineSession';
import { useI18n, getLocale } from '../context/I18n';

// Lazy: guided mode (the default) never mounts these, so @xyflow/react,
// d3-dag and CodeMirror stay out of the initial editor load (issue #79).
const WorkflowCanvas = lazy(() => import('../components/WorkflowCanvas'));
const TomlEditor = lazy(() => import('../components/TomlEditor'));

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

interface EngineFormState {
  minFreeDisk: string;
  reclaimFreeDisk: string;
  tmpDir: string;
  protectedTrees: string;
}

// Escape a value for safe interpolation into a TOML basic string ("...").
function escapeTomlBasicString(value: string): string {
  let out = '';
  for (let i = 0; i < value.length; i++) {
    const ch = value[i];
    const code = value.charCodeAt(i);
    if (ch === '\\') out += '\\\\';
    else if (ch === '"') out += '\\"';
    else if (code === 0x08) out += '\\b';
    else if (ch === '\t') out += '\\t';
    else if (ch === '\n') out += '\\n';
    else if (code === 0x0c) out += '\\f';
    else if (ch === '\r') out += '\\r';
    else if (code < 0x20 || code === 0x7f) out += `\\u${code.toString(16).padStart(4, '0')}`;
    else out += ch;
  }
  return out;
}

function hasControlOrNewline(value: string): boolean {
  for (let i = 0; i < value.length; i++) {
    const code = value.charCodeAt(i);
    if (code === 0x0a || code === 0x0d || (code < 0x20 && code !== 0x09) || code === 0x7f) {
      return true;
    }
  }
  return false;
}

interface TomlSectionSpan {
  name: string;
  isArrayTable: boolean;
  headerLineStart: number;
  bodyStart: number;
  end: number;
}

// Lexically scan TOML top-level section headers (`[table]` and `[[array]]`),
// ignoring anything inside basic/literal single- or multi-line strings,
// `#` comments, or multi-line inline arrays/tables.
function scanTomlTopLevelSections(toml: string): TomlSectionSpan[] {
  const sections: TomlSectionSpan[] = [];
  let inMultiBasic = false;
  let inMultiLiteral = false;
  let bracketDepth = 0;

  let lineStart = 0;
  while (lineStart <= toml.length) {
    const nlIdx = toml.indexOf('\n', lineStart);
    const lineEnd = nlIdx === -1 ? toml.length : nlIdx;
    const rawLine = toml.slice(lineStart, lineEnd);

    if (!inMultiBasic && !inMultiLiteral && bracketDepth === 0) {
      const arrayMatch = rawLine.match(/^[ \t]*\[\[([A-Za-z0-9_.-]+)\]\][ \t]*(?:#.*)?\r?$/);
      const tableMatch = !arrayMatch ? rawLine.match(/^[ \t]*\[([A-Za-z0-9_.-]+)\][ \t]*(?:#.*)?\r?$/) : null;
      if (arrayMatch || tableMatch) {
        if (sections.length > 0) {
          sections[sections.length - 1].end = lineStart;
        }
        sections.push({
          name: (arrayMatch ?? tableMatch)![1],
          isArrayTable: Boolean(arrayMatch),
          headerLineStart: lineStart,
          bodyStart: nlIdx === -1 ? toml.length : nlIdx + 1,
          end: toml.length,
        });
        if (nlIdx === -1) break;
        lineStart = nlIdx + 1;
        continue;
      }
    }

    let i = 0;
    let inBasic = false;
    let inLiteral = false;
    while (i < rawLine.length) {
      const ch = rawLine[i];
      if (inMultiBasic) {
        if (ch === '\\') { i += 2; continue; }
        if (rawLine.startsWith('"""', i)) { inMultiBasic = false; i += 3; continue; }
        i++;
      } else if (inMultiLiteral) {
        if (rawLine.startsWith("'''", i)) { inMultiLiteral = false; i += 3; continue; }
        i++;
      } else if (inBasic) {
        if (ch === '\\') { i += 2; continue; }
        if (ch === '"') inBasic = false;
        i++;
      } else if (inLiteral) {
        if (ch === "'") inLiteral = false;
        i++;
      } else {
        if (ch === '#') break;
        if (rawLine.startsWith('"""', i)) { inMultiBasic = true; i += 3; continue; }
        if (rawLine.startsWith("'''", i)) { inMultiLiteral = true; i += 3; continue; }
        if (ch === '"') { inBasic = true; i++; continue; }
        if (ch === "'") { inLiteral = true; i++; continue; }
        if (ch === '[' || ch === '{') bracketDepth++;
        else if ((ch === ']' || ch === '}') && bracketDepth > 0) bracketDepth--;
        i++;
      }
    }

    if (nlIdx === -1) break;
    lineStart = nlIdx + 1;
  }

  return sections;
}

function insertBeforeFirstRuleSection(toml: string, snippet: string): string {
  const sections = scanTomlTopLevelSections(toml);
  const firstRule = sections.find((s) => s.isArrayTable && s.name === 'rules');
  if (!firstRule) {
    return `${toml.trimEnd()}\n\n${snippet}\n`;
  }
  const before = toml.slice(0, firstRule.headerLineStart).trimEnd();
  const after = toml.slice(firstRule.headerLineStart);
  return before ? `${before}\n\n${snippet}\n\n${after}` : `${snippet}\n\n${after}`;
}

function splitSectionStatements(body: string): string[] {
  const lines = body.replace(/\r\n/g, '\n').split('\n');
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop();
  const statements: string[] = [];
  let current: string[] = [];
  let inMultiBasic = false;
  let inMultiLiteral = false;
  let bracketDepth = 0;

  for (const line of lines) {
    current.push(line);
    let i = 0;
    let inBasic = false;
    let inLiteral = false;
    while (i < line.length) {
      const ch = line[i];
      if (inMultiBasic) {
        if (ch === '\\') { i += 2; continue; }
        if (line.startsWith('"""', i)) { inMultiBasic = false; i += 3; continue; }
        i++;
      } else if (inMultiLiteral) {
        if (line.startsWith("'''", i)) { inMultiLiteral = false; i += 3; continue; }
        i++;
      } else if (inBasic) {
        if (ch === '\\') { i += 2; continue; }
        if (ch === '"') inBasic = false;
        i++;
      } else if (inLiteral) {
        if (ch === "'") inLiteral = false;
        i++;
      } else {
        if (ch === '#') break;
        if (line.startsWith('"""', i)) { inMultiBasic = true; i += 3; continue; }
        if (line.startsWith("'''", i)) { inMultiLiteral = true; i += 3; continue; }
        if (ch === '"') { inBasic = true; i++; continue; }
        if (ch === "'") { inLiteral = true; i++; continue; }
        if (ch === '[' || ch === '{') bracketDepth++;
        else if ((ch === ']' || ch === '}') && bracketDepth > 0) bracketDepth--;
        i++;
      }
    }
    if (!inMultiBasic && !inMultiLiteral && bracketDepth === 0) {
      statements.push(current.join('\n'));
      current = [];
    }
  }
  if (current.length > 0) statements.push(current.join('\n'));
  return statements;
}

function unescapeTomlBasicString(raw: string): string {
  return raw.replace(/\\(["\\btnfr])/g, (_, ch: string) => {
    switch (ch) {
      case '"': return '"';
      case '\\': return '\\';
      case 'b': return '\b';
      case 't': return '\t';
      case 'n': return '\n';
      case 'f': return '\f';
      case 'r': return '\r';
      default: return ch;
    }
  });
}

function readEngineFormFromToml(toml: string): EngineFormState {
  const sections = scanTomlTopLevelSections(toml);
  const engineSec = sections.find((s) => !s.isArrayTable && s.name === 'engine');
  if (!engineSec) {
    return { minFreeDisk: '20G', reclaimFreeDisk: '50G', tmpDir: '', protectedTrees: '' };
  }
  const body = toml.slice(engineSec.bodyStart, engineSec.end);
  const stmts = splitSectionStatements(body);
  let minFreeDisk = '';
  let reclaimFreeDisk = '';
  let tmpDir = '';
  let protectedTrees = '';
  for (const stmt of stmts) {
    const m = stmt.match(/^[ \t]*(min_free_disk|reclaim_free_disk|tmp_dir|protected_data_trees)[ \t]*=[ \t]*([\s\S]*)$/);
    if (!m) continue;
    const key = m[1];
    const rhs = m[2].trim();
    if (key === 'protected_data_trees') {
      const items: string[] = [];
      const strRe = /"((?:\\.|[^"\\])*)"/g;
      let sm: RegExpExecArray | null;
      while ((sm = strRe.exec(rhs)) !== null) {
        items.push(unescapeTomlBasicString(sm[1]));
      }
      protectedTrees = items.join(', ');
    } else {
      const sm = rhs.match(/^"((?:\\.|[^"\\])*)"/);
      const val = sm ? unescapeTomlBasicString(sm[1]) : '';
      if (key === 'min_free_disk') minFreeDisk = val;
      else if (key === 'reclaim_free_disk') reclaimFreeDisk = val;
      else if (key === 'tmp_dir') tmpDir = val;
    }
  }
  return { minFreeDisk, reclaimFreeDisk, tmpDir, protectedTrees };
}

function mergeEngineSettingsIntoToml(toml: string, form: EngineFormState): { nextToml: string; engineBlock: string } {
  const trees = form.protectedTrees.split(',').map((s) => s.trim()).filter(Boolean);
  const managedOrder = ['min_free_disk', 'reclaim_free_disk', 'tmp_dir', 'protected_data_trees'] as const;
  const managedValues: Record<(typeof managedOrder)[number], string | null> = {
    min_free_disk: form.minFreeDisk.trim() ? `"${escapeTomlBasicString(form.minFreeDisk.trim())}"` : null,
    reclaim_free_disk: form.reclaimFreeDisk.trim() ? `"${escapeTomlBasicString(form.reclaimFreeDisk.trim())}"` : null,
    tmp_dir: form.tmpDir.trim() ? `"${escapeTomlBasicString(form.tmpDir.trim())}"` : null,
    protected_data_trees: trees.length > 0 ? `[${trees.map((p) => `"${escapeTomlBasicString(p)}"`).join(', ')}]` : null,
  };

  const sections = scanTomlTopLevelSections(toml);
  const engineSec = sections.find((s) => !s.isArrayTable && s.name === 'engine');
  if (!engineSec) {
    const lines: string[] = ['[engine]'];
    for (const key of managedOrder) {
      if (managedValues[key] !== null) {
        lines.push(`${key} = ${managedValues[key]}`);
      }
    }
    const engineBlock = lines.join('\n');
    return {
      nextToml: insertBeforeFirstRuleSection(toml, engineBlock),
      engineBlock,
    };
  }

  const body = toml.slice(engineSec.bodyStart, engineSec.end);
  const stmts = splitSectionStatements(body);
  const handled = new Set<string>();
  const outStmts: string[] = [];

  for (const stmt of stmts) {
    const m = stmt.match(/^[ \t]*(min_free_disk|reclaim_free_disk|tmp_dir|protected_data_trees)[ \t]*=/);
    if (m) {
      const key = m[1] as (typeof managedOrder)[number];
      if (!handled.has(key) && managedValues[key] !== null) {
        outStmts.push(`${key} = ${managedValues[key]}`);
      }
      handled.add(key);
    } else {
      outStmts.push(stmt);
    }
  }

  while (outStmts.length > 0 && outStmts[outStmts.length - 1].trim() === '') {
    outStmts.pop();
  }

  for (const key of managedOrder) {
    if (!handled.has(key) && managedValues[key] !== null) {
      outStmts.push(`${key} = ${managedValues[key]}`);
    }
  }

  const headerLine = toml.slice(engineSec.headerLineStart, engineSec.bodyStart).trimEnd();
  const engineBlock = [headerLine, ...outStmts].join('\n');
  const hasFollowingSection = engineSec.end < toml.length;
  const newBody = outStmts.length > 0
    ? `${outStmts.join('\n')}${hasFollowingSection ? '\n\n' : '\n'}`
    : hasFollowingSection ? '\n' : '';
  const nextToml = `${toml.slice(0, engineSec.bodyStart)}${newBody}${toml.slice(engineSec.end)}`;
  return { nextToml, engineBlock };
}

export default function PipelineEditor() {
  const session = usePipelineSession();
  const { lang, t } = useI18n();
  const locale = getLocale(lang);
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
  const [leftCollapsed, setLeftCollapsed] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);
  const toggleFullscreen = () => {
    const next = !fullscreen;
    setFullscreen(next);
    if (next && !document.fullscreenElement) {
      document.documentElement.requestFullscreen().catch(() => { /* ignore */ });
    } else if (!next && document.fullscreenElement) {
      document.exitFullscreen().catch(() => { /* ignore */ });
    }
  };
  const switchViewMode = (mode: 'guided' | 'canvas') => {
    localStorage.setItem('oxo_editor_mode', mode);
    setViewMode(mode);
  };

  // Guided mode has no canvas toolbar, so expose the same Run/Dry-Run entry
  // points and switch the user into Canvas + TOML before opening the dialog.
  const openRunDialogFromGuided = () => {
    switchViewMode('canvas');
    setShowRunDialog(true);
  };
  const [revisions, setRevisions] = useState<Array<{ id: string; version: string; actor: string; created_at: string }> | null>(null);
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const [showDataModal, setShowDataModal] = useState(() => searchParams.get('modal') === 'data');
  const [showEngineModal, setShowEngineModal] = useState(() => searchParams.get('modal') === 'engine');
  const [dataTab, setDataTab] = useState<'sheet' | 'upload' | 'verify'>('sheet');
  const [sheetText, setSheetText] = useState('sample,fastq_r1,fastq_r2\nS1,data/S1_R1.fq.gz,data/S1_R2.fq.gz\nS2,data/S2_R1.fq.gz,data/S2_R2.fq.gz');
  const [sheetParsed, setSheetParsed] = useState<SamplesheetParseResponse | null>(null);
  const [uploadedFiles, setUploadedFiles] = useState<UploadedFileEntry[]>([]);
  const [verifyTree, setVerifyTree] = useState('inputs');
  const [verifyMd5, setVerifyMd5] = useState('');
  const [verifyRes, setVerifyRes] = useState<VerifyDataResponse | null>(null);
  const [engineForm, setEngineForm] = useState<EngineFormState>(() => readEngineFormFromToml(toml));
  const fileUploadRef = useRef<HTMLInputElement>(null);

  // React to `?modal=data|engine` changes even when PipelineEditor is already mounted.
  const modalParam = searchParams.get('modal');
  const [lastModalParam, setLastModalParam] = useState(modalParam);
  if (lastModalParam !== modalParam) {
    setLastModalParam(modalParam);
    if (modalParam === 'data') {
      setShowDataModal(true);
      setShowEngineModal(false);
    } else if (modalParam === 'engine') {
      setEngineForm(readEngineFormFromToml(toml));
      setShowEngineModal(true);
      setShowDataModal(false);
    }
  }

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

  // Debounced buildDag+validate runs in both view modes: guided mode gates
  // its Run/Dry-Run buttons on `validation`, which nothing else populates
  // (in canvas mode it also keeps the DAG panel in sync).
  useEffect(() => {
    const timer = setTimeout(() => updateDag(toml), 300);
    return () => clearTimeout(timer);
  }, [toml, updateDag]);

  const handleRun = async (dryRun = false, options: RunOptions = { maxJobs: 4, keepGoing: false, samples: [], targets: [] }) => {
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
        cluster_id: options.clusterId,
        provenance: options.provenance,
        allow_protected_drift: options.allowProtectedDrift,
        config_overrides: options.configOverrides?.length ? options.configOverrides : undefined,
      });
      session.setRunResult({
        runId: res.run_id,
        message: t('editor.runStarted')
          .replace('{{kind}}', dryRun ? 'Dry-Run' : 'Run')
          .replace('{{id}}', res.run_id.slice(0, 8))
          .replace('{{rules}}', String(res.execution_plan.total_rules))
          .replace('{{secs}}', String(res.estimated_resources.estimated_duration_secs)),
        type: 'success',
      });
      if (!dryRun && res.run_id) {
        session.setActiveRunId(res.run_id);
        navigate(`/runs/${res.run_id}`);
      }
    } catch (err: unknown) {
      // describeApiError surfaces the backend's detail/suggestion hints
      // (the structured-error contract used to die in the UI, audit #653).
      const msg = describeApiError(err, t('editor.runStartFailed'));
      session.setRunResult({ message: t('editor.errorPrefix').replace('{{message}}', msg), type: 'error' });
    }
    setRunning(false);
  };

  // Every canvas/inspector/palette edit runs through the backend command API;
  // the returned canonical TOML replaces the local state (single source of
  // truth). Returns that TOML, or null when the edit was superseded/failed,
  // so callers can chain edits against the latest content.
  const runEdit = async (operation: string, payload: Record<string, unknown>, baseToml = toml): Promise<string | null> => {
    const seq = ++editSeq.current;
    try {
      const res = await api.dagCommand(pipelineId, baseToml, operation, payload);
      if (seq !== editSeq.current) return null; // superseded by a newer edit
      setToml(res.toml_content);
      const errors = res.validation_errors ?? [];
      if (!res.success && errors.length > 0) {
        session.setRunResult({ message: `Validation: ${errors.join('; ')}`, type: 'error' });
      }
      return res.toml_content;
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Edit failed';
      session.setRunResult({ message: `Edit failed: ${msg}`, type: 'error' });
      return null;
    }
  };

  const handleConnect = (from: string, to: string) => runEdit('connect', { from, to });
  // Same chaining rule as handleRemove: every edit posts the whole TOML, so
  // parallel disconnects would race and only the last one would survive.
  const handleDisconnect = async (edges: Array<{ from: string; to: string }>) => {
    let current = toml;
    for (const { from, to } of edges) {
      const next = await runEdit('disconnect', { from, to }, current);
      if (next === null) return; // superseded or failed — stop the chain
      current = next;
    }
  };
  // Each edit posts the whole TOML, so firing remove_rule per name in
  // parallel raced: every request carried the same original content and only
  // the last removal survived. Chain them, threading each response's TOML
  // into the next request.
  const handleRemove = async (names: string[]) => {
    let current = toml;
    for (const name of names) {
      const next = await runEdit('remove_rule', { name }, current);
      if (next === null) return; // superseded or failed — stop the chain
      current = next;
    }
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

  const handleInspectorSave = async (patch: Record<string, unknown>, newName?: string) => {
    if (!inspector) return;
    // The patch targets the old name, so it must be applied before the
    // rename. A failed edit keeps the dialog open (the result bar carries
    // the backend's validation message) instead of silently discarding work.
    // The rename must run against the PATCHED document, not the stale
    // closure `toml` — thread the update's result into the rename call.
    let current = toml;
    if (Object.keys(patch).length > 0) {
      const patched = await runEdit('update_rule', { name: inspector.ruleName, patch }, current);
      if (patched === null) return;
      current = patched;
    }
    if (newName && newName !== inspector.ruleName) {
      const renamed = await runEdit(
        'rename_rule',
        {
          name: inspector.ruleName,
          new_name: newName,
        },
        current,
      );
      if (renamed === null) return;
    }
    setInspector(null);
  };

  const handleUndo = async () => {
    // #547: undo/redo take the editSeq ticket like runEdit/updateDag — a
    // slow undo resolving after a newer edit used to revert the canvas
    // while the server recorded the newer edit (local/server divergence).
    const seq = ++editSeq.current;
    try {
      const res = await api.dagUndo(pipelineId, toml);
      if (seq !== editSeq.current) return; // superseded by a newer edit
      setToml(res.toml_content);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Nothing to undo';
      session.setRunResult({ message: msg, type: 'error' });
    }
  };
  const handleRedo = async () => {
    const seq = ++editSeq.current;
    try {
      const res = await api.dagRedo(pipelineId, toml);
      if (seq !== editSeq.current) return; // superseded by a newer edit
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
          {t('editor.history.empty')}
        </div>
      );
    }
    if (revisions === null) return <div className="empty-state">{t('editor.history.loading')}</div>;
    if (revisions.length === 0) {
      return <div className="empty-state">{t('editor.history.noSnapshots')}</div>;
    }
    return (
      <div>
        <div style={{ fontSize: '0.8rem', color: 'var(--color-text-secondary)', marginBottom: '8px' }}>
          {t('editor.history.snapshots').replace('{{count}}', String(revisions.length))}
        </div>
        {revisions.map((r) => (
          <div key={r.id} className="dash-card" style={{ marginBottom: '8px', padding: '8px' }}>
            <div style={{ fontSize: '0.82rem', fontWeight: 600 }}>
              v{r.version} <span className="mono" style={{ fontWeight: 400, fontSize: '0.72rem' }}>{r.id.slice(0, 8)}</span>
            </div>
            <div style={{ fontSize: '0.72rem', color: 'var(--color-text-tertiary)' }}>
              {new Date(r.created_at).toLocaleString(locale)} · {r.actor}
            </div>
            <div style={{ display: 'flex', gap: '6px', marginTop: '6px' }}>
              <button className="btn-sm" onClick={async () => {
                try {
                  const snap = await api.getRevision(savedPipelineId, r.id);
                  setToml(snap.toml_content);
                  session.setRunResult({ message: `Loaded snapshot ${r.id.slice(0, 8)} into the editor — Save to keep it`, type: 'success' });
                } catch { /* ignore */ }
              }}>{t('editor.history.load')}</button>
              <button className="btn-sm" onClick={async () => {
                if (!window.confirm(t('editor.history.rollbackConfirm').replace('{{time}}', new Date(r.created_at).toLocaleString(locale)))) return;
                try {
                  await api.rollbackPipeline(savedPipelineId, r.id);
                  setToml((await api.getPipeline(savedPipelineId)).toml_content);
                  api.listRevisions(savedPipelineId).then(setRevisions);
                  session.setRunResult({ message: 'Rolled back — the current version was preserved as a revision', type: 'success' });
                } catch { /* ignore */ }
              }}>{t('editor.history.rollback')}</button>
            </div>
          </div>
        ))}
      </div>
    );
  };

  const handleSave = async () => {
    // Issue #79 P1-09: Save reported success even for TOML with 23 errors
    // or cycles. Invalid content is refused with the error list instead.
    // Validate the exact content being saved: the debounced result can be
    // stale by up to 300ms (and was never populated at all in guided mode).
    let current = validation;
    try {
      current = await api.validate(toml);
      setValidation(current);
    } catch { /* keep the last known validation */ }
    if (current && !current.valid) {
      setShowErrors(true);
      session.setRunResult({
        message: `Not saved: ${current.errors.length} validation error(s) — see the panel below`,
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
        session.setRunResult({ message: t('editor.saved').replace('{{name}}', name), type: 'success' });
        return;
      }
      const res = await api.createPipeline({ name, toml_content: toml });
      session.setRunResult({
        message: t('editor.created')
          .replace('{{name}}', name)
          .replace('{{id}}', res.id.slice(0, 8)),
        type: 'success',
      });
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : t('editor.saveFailed');
      session.setRunResult({ message: t('editor.saveError').replace('{{message}}', msg), type: 'error' });
    }
  };

  const handleFormat = async () => {
    try {
      const res = await api.format(toml);
      setToml(res.formatted);
      session.setRunResult({ message: t('editor.formatted'), type: 'success' });
    } catch (err: unknown) {
      const msg = describeApiError(err, 'Format failed');
      session.setRunResult({ message: msg, type: 'error' });
    }
  };

  const handleLint = async () => {
    try {
      const res = await api.lint(toml);
      const issues = [...(res.errors ?? []), ...(res.warnings ?? [])];
      if (issues.length === 0) {
        session.setRunResult({ message: t('editor.lintClean'), type: 'success' });
      } else {
        setValidation({ valid: res.errors.length === 0, errors: issues });
        setShowErrors(true);
        session.setRunResult({
          message: `Lint found ${issues.length} item(s) — inspect the diagnostics panel`,
          type: 'info',
        });
      }
    } catch (err: unknown) {
      const msg = describeApiError(err, 'Lint failed');
      session.setRunResult({ message: msg, type: 'error' });
    }
  };

  const applyEngineSettings = () => {
    const rawFields = [engineForm.minFreeDisk, engineForm.reclaimFreeDisk, engineForm.tmpDir, engineForm.protectedTrees];
    if (rawFields.some(hasControlOrNewline)) {
      session.setRunResult({ message: t('editor.engineGuard.invalidChars'), type: 'error' });
      return;
    }
    const { nextToml } = mergeEngineSettingsIntoToml(toml, engineForm);
    setToml(nextToml);
    setShowEngineModal(false);
    session.setRunResult({ message: t('editor.engineGuard.updated'), type: 'success' });
  };

  useEffect(() => {
    if (showDataModal && dataTab === 'upload') {
      api.listUploadedFiles().then(setUploadedFiles).catch(() => setUploadedFiles([]));
    }
  }, [showDataModal, dataTab]);

  return (
    <div className="page">
      <div style={{ display: 'flex', alignItems: 'center', gap: '0.75rem', flexWrap: 'wrap', marginBottom: '1rem' }}>
        <h1 className="page-title" style={{ margin: 0 }}>{t('editor.title')}</h1>
        <div className="row" style={{ gap: '4px' }}>
          <button
            className={viewMode === 'guided' ? 'btn-run' : 'btn-sm'}
            onClick={() => switchViewMode('guided')}
            title={t('editor.guidedTooltip')}
          >
            {t('editor.guided')}
          </button>
          <button
            className={viewMode === 'canvas' ? 'btn-run' : 'btn-sm'}
            onClick={() => switchViewMode('canvas')}
            title={t('editor.canvasTooltip')}
          >
            {t('editor.canvas')}
          </button>
        </div>
        <div className="row" style={{ marginLeft: 'auto', gap: '6px' }}>
          <button className="btn-sm" onClick={() => setShowDataModal(true)} title={t('editor.data.title')}>
            <FileSpreadsheet size={14} /> {t('editor.dataWorkbench')}
          </button>
          <button
            className="btn-sm"
            onClick={() => {
              setEngineForm(readEngineFormFromToml(toml));
              setShowEngineModal(true);
            }}
            title={t('editor.engineGuard.title')}
          >
            <Shield size={14} /> {t('editor.engineGuard')}
          </button>
          <button className="btn-sm" onClick={() => void handleFormat()} title={t('editor.format')}>
            <AlignLeft size={14} /> {t('editor.format')}
          </button>
          <button className="btn-sm" onClick={() => void handleLint()} title={t('editor.lint')}>
            <Sparkles size={14} /> {t('editor.lint')}
          </button>
        </div>
      </div>

      {viewMode === 'guided' && (
        <div style={{ marginTop: '1rem' }}>
          <div className="action-row">
            <button
              className="btn-sm"
              onClick={() => void handleSave()}
              title={t('editor.save')}
            >
              <Save size={14} /> {t('editor.save')}
            </button>
            <button
              className="btn-sm"
              onClick={openRunDialogFromGuided}
              disabled={running || !validation?.valid}
              title="Preview the execution plan without running anything"
            >
              <CheckCircle size={14} /> Dry-Run
            </button>
            <button
              className="btn-run"
              onClick={openRunDialogFromGuided}
              disabled={running || !validation?.valid}
              title="Switch to Canvas mode and run this pipeline"
            >
              <Play size={16} /> Run
            </button>
          </div>
          <GuidedRuleBuilder toml={toml} onChange={(v) => setToml(v)} />
        </div>
      )}

      {/* Canvas/TOML/chat panels mount only in canvas mode — guided mode
          must not pay for xyflow/CodeMirror mounting. Editor state lives in
          this parent (toml/dagJson/validation), so switching back loses
          nothing. */}
      {viewMode === 'canvas' && (
      <Suspense fallback={<div className="empty-state">Loading editor…</div>}>
      <div className={`editor-layout ${leftCollapsed ? 'editor-layout--left-collapsed' : ''} ${fullscreen ? 'editor-layout--fullscreen' : ''}`}>
        <div className={`left-rail ${leftCollapsed ? 'left-rail--collapsed' : ''}`}>
          <div className="left-rail-tabs" role="tablist">
            <button
              role="tab"
              aria-selected={leftTab === 'palette'}
              className={`left-rail-tab ${leftTab === 'palette' ? 'active' : ''}`}
              onClick={() => setLeftTab('palette')}
            >
              <Blocks size={14} /> {t('editor.tools')}
            </button>
            <button
              role="tab"
              aria-selected={leftTab === 'assistant'}
              className={`left-rail-tab ${leftTab === 'assistant' ? 'active' : ''}`}
              onClick={() => setLeftTab('assistant')}
            >
              <Wand2 size={14} /> {t('editor.assistant')}
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
              <History size={14} /> {t('editor.history')}
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
            <span>{t('editor.panel.dag')}</span>
            <div className="panel-actions">
              <button
                className="btn-sm"
                title={leftCollapsed ? t('editor.expandLeft') : t('editor.collapseLeft')}
                aria-label={leftCollapsed ? t('editor.expandLeft') : t('editor.collapseLeft')}
                onClick={() => setLeftCollapsed((v) => !v)}
              >
                {leftCollapsed ? <PanelLeftOpen size={14} /> : <PanelLeftClose size={14} />}
              </button>
              <button className="btn-sm" onClick={handleUndo} title={t('editor.undo')} aria-label={t('editor.undo')}>
                <Undo2 size={14} />
              </button>
              <button className="btn-sm" onClick={handleRedo} title={t('editor.redo')} aria-label={t('editor.redo')}>
                <Redo2 size={14} />
              </button>
              {dagJson && (
                <span className="dag-counts">
                  {t('editor.nodesCount').replace('{{nodes}}', String(dagJson.nodes.length)).replace('{{edges}}', String(dagJson.edges.length))}
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
            onDisconnectRules={handleDisconnect}
            onRemoveRules={handleRemove}
          />
          <div className="canvas-legend">
            <span className="legend-item">
              <span className="legend-line legend-declared" /> {t('editor.legend.depends')}
            </span>
            <span className="legend-item">
              <span className="legend-line legend-file" /> {t('editor.legend.file')}
            </span>
            <span className="legend-hint">{t('editor.legend.hint')}</span>
          </div>
        </div>

        <div className="editor-panel">
          <div className="panel-header">
            <span>{t('editor.panel.toml')}</span>
            <div className="panel-actions">
              <button
                className="btn-sm"
                title={fullscreen ? t('editor.exitFullscreen') : t('editor.enterFullscreen')}
                aria-label={fullscreen ? t('editor.exitFullscreen') : t('editor.enterFullscreen')}
                onClick={toggleFullscreen}
              >
                {fullscreen ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
              </button>
              {validation && (
                <button
                  className={`val-badge ${validation.valid ? 'valid' : 'invalid'}`}
                  style={{ background: 'transparent', border: 'none', cursor: validation.valid ? 'default' : 'pointer' }}
                  onClick={() => setShowErrors((v) => !v)}
                  title={validation.valid ? t('editor.validation.tooltip.valid') : t('editor.validation.tooltip.invalid')}
                >
                  {validation.valid ? <CheckCircle size={14} /> : <AlertCircle size={14} />}
                  {validation.valid ? ` ${t('editor.validation.valid')}` : ` ${t('editor.validation.errors').replace('{{count}}', String(validation.errors.length))}`}
                </button>
              )}
              <button onClick={handleSave} className="btn-sm" style={{ background: 'transparent', border: '1px solid var(--color-border)' }}>
                <Save size={14} /> {t('editor.save')}
              </button>
              <button onClick={() => setShowRunDialog(true)} disabled={running || !validation?.valid} className="btn-run">
                <Play size={16} /> {running ? t('editor.starting') : t('editor.run')}
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
                      {t('editor.errors.line').replace('{{line}}', String(e.line))}
                    </button>
                  )}
                  <div className="validation-error-body">
                    <div>
                      {e.rule ? <strong>{e.rule}: </strong> : null}
                      {e.message}
                    </div>
                    {e.suggestion && <div className="validation-error-suggestion">{t('editor.errors.suggestion').replace('{{suggestion}}', e.suggestion)}</div>}
                  </div>
                </div>
              ))}
            </div>
          )}
          {/* #580: a CodeMirror plugin crash is caught by CodeMirror itself
              (logException), so React never sees it — the editor goes blank
              instead. This boundary keeps the rest of the panel usable and
              offers a remount. */}
          <ErrorBoundary
            fallback={
              <div className="empty-state">
                <p>The TOML editor crashed and was reset. Switch modes or reload to recover — your edits above are preserved in the pipeline state.</p>
              </div>
            }
          >
            <TomlEditor value={toml} onChange={(v) => setToml(v)} highlightLine={highlightLine} />
          </ErrorBoundary>
        </div>
      </div>
      </Suspense>
      )}

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

      {showDataModal && (
        <div className="modal-overlay" onClick={() => setShowDataModal(false)}>
          <div className="modal-dialog" style={{ maxWidth: 640 }} onClick={(e) => e.stopPropagation()} role="dialog" aria-modal="true" aria-label={t('editor.data.title')}>
            <div className="inspector-header" style={{ padding: '0 0 10px 0', marginBottom: 10 }}>
              <h3 style={{ margin: 0 }}>{t('editor.data.title')}</h3>
              <button className="btn-sm" onClick={() => setShowDataModal(false)} aria-label={t('common.close')}><X size={14} /></button>
            </div>
            <div className="pill-bar">
              <button type="button" className={`pill-chip ${dataTab === 'sheet' ? 'active' : ''}`} onClick={() => setDataTab('sheet')}>{t('editor.data.tabSheet')}</button>
              <button type="button" className={`pill-chip ${dataTab === 'upload' ? 'active' : ''}`} onClick={() => setDataTab('upload')}>{t('editor.data.tabUpload')}</button>
              <button type="button" className={`pill-chip ${dataTab === 'verify' ? 'active' : ''}`} onClick={() => setDataTab('verify')}>{t('editor.data.tabVerify')}</button>
            </div>
            {dataTab === 'sheet' && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
                <p className="muted">{t('editor.data.sheetHint')}</p>
                <textarea
                  className="search-input mono"
                  rows={5}
                  value={sheetText}
                  onChange={(e) => setSheetText(e.target.value)}
                />
                <div className="row">
                  <button
                    type="button"
                    className="btn-run"
                    onClick={async () => {
                      try {
                        const res = await api.parseSamplesheet(sheetText);
                        setSheetParsed(res);
                      } catch (err: unknown) {
                        session.setRunResult({ message: describeApiError(err, 'Sample sheet parse failed'), type: 'error' });
                      }
                    }}
                  >
                    <FileSpreadsheet size={14} /> {t('editor.data.parseBtn')}
                  </button>
                  {sheetParsed && sheetParsed.samples.length > 0 && (
                    <button
                      type="button"
                      className="btn-sm"
                      onClick={() => {
                        if (sheetParsed.samples.some((s) => hasControlOrNewline(s.sample ?? ''))) {
                          session.setRunResult({ message: t('editor.data.invalidSampleName'), type: 'error' });
                          return;
                        }
                        const sampleList = sheetParsed.samples.map((s) => `"${escapeTomlBasicString(s.sample ?? '')}"`).join(', ');
                        const groupSnippet = `[[sample_groups]]\nname = "cohort"\nsamples = [${sampleList}]`;
                        const next = insertBeforeFirstRuleSection(toml, groupSnippet);
                        setToml(next);
                        setShowDataModal(false);
                        session.setRunResult({
                          message: t('editor.data.injected').replace('{{count}}', String(sheetParsed.samples.length)),
                          type: 'success',
                        });
                      }}
                    >
                      {t('editor.data.insertConfig')} ({sheetParsed.samples.length})
                    </button>
                  )}
                </div>
                {sheetParsed && (
                  <div className="overflow-x" style={{ maxHeight: 180 }}>
                    <table className="data-table">
                      <thead>
                        <tr><th>Sample</th><th>R1</th><th>R2</th></tr>
                      </thead>
                      <tbody>
                        {sheetParsed.samples.map((s, idx) => (
                          <tr key={idx}>
                            <td className="mono">{s.sample}</td>
                            <td className="mono">{s.fastq_r1 ?? '-'}</td>
                            <td className="mono">{s.fastq_r2 ?? '-'}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
              </div>
            )}
            {dataTab === 'upload' && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
                <div className="row">
                  <button type="button" className="btn-run" onClick={() => fileUploadRef.current?.click()}>
                    <Upload size={14} /> {t('editor.data.uploadBtn')}
                  </button>
                  <input
                    ref={fileUploadRef}
                    type="file"
                    multiple
                    style={{ display: 'none' }}
                    onChange={async (e) => {
                      if (!e.target.files?.length) return;
                      try {
                        await api.uploadFiles(e.target.files);
                        const list = await api.listUploadedFiles();
                        setUploadedFiles(list);
                        session.setRunResult({ message: t('editor.data.uploaded'), type: 'success' });
                      } catch (err: unknown) {
                        session.setRunResult({ message: describeApiError(err, 'Upload failed'), type: 'error' });
                      }
                    }}
                  />
                </div>
                {uploadedFiles.length === 0 ? (
                  <div className="empty-state" style={{ padding: '1.5rem 0' }}>{t('editor.data.noUploads')}</div>
                ) : (
                  <div className="overflow-x" style={{ maxHeight: 200 }}>
                    <table className="data-table">
                      <thead><tr><th>Path</th><th>Size</th></tr></thead>
                      <tbody>
                        {uploadedFiles.map((f) => (
                          <tr key={f.path}>
                            <td className="mono">{f.path}</td>
                            <td>{(f.size_bytes / 1024).toFixed(1)} KB</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
              </div>
            )}
            {dataTab === 'verify' && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
                <label className="inspector-field">
                  <span>{t('editor.data.verifyTree')}</span>
                  <input className="mono" value={verifyTree} onChange={(e) => setVerifyTree(e.target.value)} placeholder="inputs" />
                </label>
                <label className="inspector-field">
                  <span>{t('editor.data.verifyMd5')}</span>
                  <textarea className="search-input mono" rows={4} value={verifyMd5} onChange={(e) => setVerifyMd5(e.target.value)} placeholder="d41d8cd98f00b204e9800998ecf8427e  sample_R1.fastq.gz" />
                </label>
                <div>
                  <button
                    type="button"
                    className="btn-run"
                    disabled={!verifyTree.trim() || !verifyMd5.trim()}
                    onClick={async () => {
                      try {
                        const r = await api.verifyData(verifyTree.trim(), verifyMd5);
                        setVerifyRes(r);
                      } catch (err: unknown) {
                        session.setRunResult({ message: describeApiError(err, 'Data verification failed'), type: 'error' });
                      }
                    }}
                  >
                    <CheckCircle size={14} /> {t('editor.data.verifyBtn')}
                  </button>
                </div>
                {verifyRes && (
                  <div className="dash-card" style={{ padding: '0.75rem' }}>
                    <div className="row" style={{ gap: '0.75rem' }}>
                      <span className="status-badge success">{t('editor.data.matched')}: {verifyRes.matched}</span>
                      <span className={`status-badge ${verifyRes.mismatched > 0 ? 'failed' : 'skipped'}`}>{t('editor.data.mismatched')}: {verifyRes.mismatched}</span>
                      <span className={`status-badge ${verifyRes.missing > 0 ? 'failed' : 'skipped'}`}>{t('editor.data.missing')}: {verifyRes.missing}</span>
                      <span className="status-badge">{t('editor.data.extra')}: {verifyRes.extra}</span>
                    </div>
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      )}

      {showEngineModal && (
        <div className="modal-overlay" onClick={() => setShowEngineModal(false)}>
          <div className="modal-dialog" onClick={(e) => e.stopPropagation()} role="dialog" aria-modal="true" aria-label={t('editor.engineGuard.title')}>
            <div className="inspector-header" style={{ padding: '0 0 10px 0', marginBottom: 10 }}>
              <h3 style={{ margin: 0 }}>{t('editor.engineGuard.title')}</h3>
              <button className="btn-sm" onClick={() => setShowEngineModal(false)} aria-label={t('common.close')}><X size={14} /></button>
            </div>
            <div className="inspector-body" style={{ padding: 0 }}>
              <label className="inspector-field">
                <span>{t('editor.engineGuard.minFreeDisk')}</span>
                <input className="mono" value={engineForm.minFreeDisk} onChange={(e) => setEngineForm({ ...engineForm, minFreeDisk: e.target.value })} placeholder="20G" />
              </label>
              <label className="inspector-field">
                <span>{t('editor.engineGuard.reclaimFreeDisk')}</span>
                <input className="mono" value={engineForm.reclaimFreeDisk} onChange={(e) => setEngineForm({ ...engineForm, reclaimFreeDisk: e.target.value })} placeholder="50G" />
              </label>
              <label className="inspector-field">
                <span>{t('editor.engineGuard.tmpDir')}</span>
                <input className="mono" value={engineForm.tmpDir} onChange={(e) => setEngineForm({ ...engineForm, tmpDir: e.target.value })} placeholder="/scratch/tmp" />
              </label>
              <label className="inspector-field">
                <span>{t('editor.engineGuard.protectedTrees')}</span>
                <input className="mono" value={engineForm.protectedTrees} onChange={(e) => setEngineForm({ ...engineForm, protectedTrees: e.target.value })} placeholder="raw_data, references" />
              </label>
              <div className="inspector-field" style={{ marginTop: 6 }}>
                <span>{t('editor.engineGuard.preview')}</span>
                <pre className="code-block mono" style={{ margin: 0, maxHeight: 140, overflow: 'auto' }}>
                  {mergeEngineSettingsIntoToml(toml, engineForm).engineBlock}
                </pre>
              </div>
            </div>
            <div className="modal-actions" style={{ marginTop: 14 }}>
              <button type="button" className="btn-sm" onClick={() => setShowEngineModal(false)}>{t('run.cancel')}</button>
              <button type="button" className="btn-run" onClick={applyEngineSettings}>
                <Shield size={14} /> {t('editor.engineGuard.apply')}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
