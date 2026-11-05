// Guided mode (issue #82 P1-5): a form-based rule builder for users who
// never want to see TOML. Each analysis step is a card with the fields
// that matter (name, command, inputs, outputs, threads, memory,
// environment). Cards round-trip through the backend parser — the TOML
// stays the single source of truth and the canvas stays in sync.
//
// Write-back splices instead of regenerating: cardsToToml takes the current
// document as its base and replaces ONLY the rules sections, patching
// name/version into the existing [workflow] header. Everything the card
// model does not express ([engine], [ai], [config], depends_on, when,
// script, resources.*, …) is carried over verbatim — an edit used to
// rewrite the whole file and silently drop it.

import { useCallback, useEffect, useRef, useState } from 'react';
import { Plus, Trash2 } from 'lucide-react';
import { api } from '../api/client';
import Glossary from './Glossary';
import { useI18n } from '../context/I18n';
import { scanTomlTopLevelSections, splitSectionStatements } from '../utils/tomlSections';

interface RuleCard {
  name: string;
  shell: string;
  inputs: string[];
  outputs: string[];
  threads: string;
  memory: string;
  environment: string;
  // Raw TOML this UI does not model, captured at import: the original rule's
  // unmanaged statements (depends_on, when, script, tags, resources.* …)
  // plus any `[rules.*]` sub-table sections attached to it. Emitted verbatim
  // on write-back so a card edit keeps them.
  originalBody: string[];
  originalTails: string;
}

interface GuidedRuleBuilderProps {
  toml: string;
  onChange: (toml: string) => void;
}

const EMPTY_CARD: RuleCard = {
  name: '',
  shell: '',
  inputs: [],
  outputs: [],
  threads: '',
  memory: '',
  environment: 'system',
  originalBody: [],
  originalTails: '',
};

// Keys the card fields fully own: regenerated from the model on every
// write-back, so they must NOT also ride along from the original text (a
// kept duplicate would conflict with the edited value).
const CARD_MANAGED_KEYS = new Set(['name', 'input', 'output', 'threads', 'memory', 'environment', 'shell']);

// Top-level key of a TOML statement, bare or quoted (dotted keys like
// `resources.memory` come back whole). Returns '' when the text does not
// look like a key = value statement (comments, continuations).
function statementKey(stmt: string): string {
  const m = stmt.match(/^[ \t]*(?:"([^"]+)"|'([^']+)'|([A-Za-z0-9_.-]+))[ \t]*=/);
  return m ? (m[1] ?? m[2] ?? m[3] ?? '') : '';
}

// Escape a value for interpolation into a TOML basic string: a raw quote
// or backslash in a name/path used to emit UNPARSEABLE TOML, silently
// disabling Run/Dry-Run with no visible error (audit #652 — only `shell`
// was escaped before).
function tomlEscape(value: string): string {
  return value.trim().replace(/\\/g, '\\\\').replace(/"/g, '\\"');
}

// Extract, per [[rules]] entry in document order, the TOML the card model
// does not own: statements whose key is not card-managed, plus any
// `[rules.*]` sub-table section (TOML array-of-tables semantics: a
// `[rules.resources]` header belongs to the most recent [[rules]] entry).
function rulePreserved(toml: string): Array<{ statements: string[]; tails: string }> {
  const sections = scanTomlTopLevelSections(toml);
  const bodies: string[] = [];
  const tails: string[] = [];
  let current = -1;
  for (const sec of sections) {
    if (sec.isArrayTable && sec.name === 'rules') {
      current = bodies.length;
      bodies.push(toml.slice(sec.bodyStart, sec.end));
      tails.push('');
      continue;
    }
    if (current >= 0 && sec.name.startsWith('rules.')) {
      tails[current] += toml.slice(sec.headerLineStart, sec.end);
    }
  }
  return bodies.map((body, i) => ({
    statements: splitSectionStatements(body)
      .map((s) => s.trim())
      .filter((s) => s !== '' && !CARD_MANAGED_KEYS.has(statementKey(s))),
    tails: tails[i],
  }));
}

// Render a card's preserved TOML. threads/memory exist in two spellings
// (rule-level shorthand and resources.*) — when the card now carries a
// value, the original spelling is dropped so exactly one definition is
// emitted (rule.rs: the shorthand IS resources.threads/resources.memory);
// when the card leaves the field blank, the original rides along untouched.
function preservedToml(card: RuleCard): string {
  const ownsThreads = card.threads.trim() !== '';
  const ownsMemory = card.memory.trim() !== '';
  const statements = card.originalBody.filter((s) => {
    const key = statementKey(s);
    if (ownsMemory && key === 'resources.memory') return false;
    if (ownsThreads && key === 'resources.threads') return false;
    return true;
  });
  let tails = card.originalTails;
  if (tails && (ownsThreads || ownsMemory)) {
    // Strip the same leaf keys from `[rules.*]` sub-table bodies. Lines
    // inside a multi-line string (`script = """…"""`) are shell text and
    // must not match — track `"""` parity (a line with an odd number of
    // `"""` toggles string state; TOML multi-line strings are the only
    // construct in a rule body that spans lines).
    let inMultiline = false;
    tails = tails
      .split('\n')
      .filter((line) => {
        if ((line.match(/"""/g)?.length ?? 0) % 2 === 1) {
          inMultiline = !inMultiline;
          return true;
        }
        if (inMultiline) return true;
        const m = line.match(/^(threads|memory)[ \t]*=/);
        if (!m) return true;
        return m[1] === 'threads' ? !ownsThreads : !ownsMemory;
      })
      .join('\n');
  }
  return [...statements, tails].map((s) => s.trim()).filter(Boolean).join('\n');
}

// Patch name/version into the ORIGINAL [workflow] section span instead of
// rebuilding it, so description and any other workflow keys survive.
function patchWorkflowMeta(span: string, name: string, version: string): string {
  const nl = span.indexOf('\n');
  const header = nl === -1 ? span : span.slice(0, nl + 1);
  const bodySrc = nl === -1 ? '' : span.slice(nl + 1);
  const stmts = splitSectionStatements(bodySrc);
  const nameLine = `name = "${tomlEscape(name)}"`;
  const versionLine = `version = "${tomlEscape(version)}"`;
  const nameIdx = stmts.findIndex((s) => statementKey(s) === 'name');
  if (nameIdx !== -1) {
    stmts[nameIdx] = nameLine;
    const versionIdx = stmts.findIndex((s) => statementKey(s) === 'version');
    if (versionIdx !== -1) stmts[versionIdx] = versionLine;
    else stmts.splice(nameIdx + 1, 0, versionLine);
  } else {
    const versionIdx = stmts.findIndex((s) => statementKey(s) === 'version');
    if (versionIdx !== -1) stmts[versionIdx] = versionLine;
    else stmts.unshift(versionLine);
    stmts.unshift(nameLine);
  }
  if (stmts.length === 0) return header;
  let body = stmts.join('\n');
  // Every original statement was newline-terminated; the trailing ''
  // statement stands for the blank separator line before the next header.
  if (bodySrc.endsWith('\n') && !body.endsWith('\n')) body += '\n';
  if (stmts[stmts.length - 1] === '') body += '\n';
  return header + body;
}

function cardsToToml(cards: RuleCard[], workflowName: string, workflowVersion: string, originalToml: string): string {
  const rulesBlock = cards
    .filter((c) => c.name.trim() !== '')
    .map((c) => {
      const lines: string[] = ['[[rules]]', `name = "${tomlEscape(c.name)}"`];
      if (c.inputs.length > 0) {
        const list = c.inputs.map((i) => `"${tomlEscape(i)}"`).join(', ');
        lines.push(`input = [${list}]`);
      }
      if (c.outputs.length > 0) {
        const list = c.outputs.map((o) => `"${tomlEscape(o)}"`).join(', ');
        lines.push(`output = [${list}]`);
      }
      if (c.threads.trim()) lines.push(`threads = ${c.threads.trim()}`);
      if (c.memory.trim()) lines.push(`memory = "${tomlEscape(c.memory)}"`);
      if (c.environment.trim() && c.environment !== 'system') {
        lines.push(`environment = "${tomlEscape(c.environment)}"`);
      }
      // A basic `"""` string escape-processes its content, so `sed 's/\t/,/g'`
      // became a literal tab and a trailing backslash broke parsing; escape
      // backslashes before quotes so the shell round-trips verbatim.
      const shell = c.shell.replace(/\\/g, '\\\\').replace(/"/g, '\\"');
      lines.push(`shell = """${shell}"""`);
      const preserved = preservedToml(c);
      if (preserved) lines.push(preserved);
      return lines.join('\n');
    })
    .join('\n\n');

  const sections = scanTomlTopLevelSections(originalToml);
  if (sections.length === 0) {
    return `[workflow]\nname = "${tomlEscape(workflowName)}"\nversion = "${tomlEscape(workflowVersion)}"\n\n${rulesBlock}\n`;
  }

  const wf = sections.find((s) => !s.isArrayTable && s.name === 'workflow');
  const wfSpan = wf ? patchWorkflowMeta(originalToml.slice(wf.headerLineStart, wf.end), workflowName, workflowVersion) : '';

  const out: string[] = [];
  // Root-level keys before the first section header stay verbatim.
  out.push(originalToml.slice(0, sections[0].headerLineStart));
  if (!wf) {
    out.push(`[workflow]\nname = "${tomlEscape(workflowName)}"\nversion = "${tomlEscape(workflowVersion)}"\n\n`);
  }
  let rulesPlaced = false;
  for (const sec of sections) {
    if (sec === wf) {
      out.push(wfSpan);
      continue;
    }
    // [[rules]] entries and their `[rules.*]` sub-tables are rebuilt from
    // the card model (sub-tables ride along inside each card's preserved
    // TOML); every other section is emitted verbatim.
    if (sec.isArrayTable && sec.name === 'rules') {
      if (!rulesPlaced) {
        out.push(rulesBlock ? `${rulesBlock}\n\n` : '');
        rulesPlaced = true;
      }
      continue;
    }
    if (sec.name.startsWith('rules.')) continue;
    out.push(originalToml.slice(sec.headerLineStart, sec.end));
  }
  if (!rulesPlaced && rulesBlock) {
    // The original had no [[rules]] — append at the end of the document.
    out.push(`\n${rulesBlock}\n`);
  }
  return out.join('').replace(/\n+$/, '\n');
}

export default function GuidedRuleBuilder({ toml, onChange }: GuidedRuleBuilderProps) {
  const { t } = useI18n();
  const [cards, setCards] = useState<RuleCard[]>([]);
  const [workflowName, setWorkflowName] = useState('my-pipeline');
  const [workflowVersion, setWorkflowVersion] = useState('0.1.0');
  // The TOML we generated ourselves — changes matching it are our own
  // echoes and must not re-import (that would clobber mid-typing state).
  const lastEmitted = useRef('');

  // Re-import into cards ONLY on external TOML changes (canvas, AI chat,
  // template load). Guided edits emit TOML directly and skip the echo.
  useEffect(() => {
    if (toml === lastEmitted.current) return;
    let cancelled = false;
    // Captured from the same document the parse below validates: the raw
    // statements/sub-tables each card must carry through its next edit.
    const preserved = rulePreserved(toml);
    api
      .parse(toml)
      .then((parsed) => {
        if (cancelled) return;
        lastEmitted.current = toml;
        setWorkflowName(parsed.name || 'my-pipeline');
        setWorkflowVersion(parsed.version || '0.1.0');
        // `memory` is absent from the parse API's rule summary, but the DAG
        // nodes carry the full serialized rule — without it a card edit
        // rebuilt the TOML and silently dropped every memory request. Read
        // both forms: cards emit the rule-level shorthand, while hand-written
        // TOML usually uses resources.memory.
        const memoryByRule = new Map(
          (parsed.dag?.nodes ?? []).map((n) => {
            const res = (n.rule?.resources ?? {}) as Record<string, unknown>;
            const shorthand = typeof n.rule?.memory === 'string' ? n.rule.memory : '';
            return [n.id, typeof res.memory === 'string' ? res.memory : shorthand];
          }),
        );
        setCards(
          parsed.rules.map((r, i) => ({
            name: r.name,
            shell: r.shell ?? '',
            inputs: r.inputs,
            outputs: r.outputs,
            threads: (r.threads ?? 0) > 0 ? String(r.threads) : '',
            memory: memoryByRule.get(r.name) ?? '',
            environment: r.environment || 'system',
            originalBody: preserved[i]?.statements ?? [],
            originalTails: preserved[i]?.tails ?? '',
          })),
        );
      })
      .catch(() => setCards([]));
    return () => { cancelled = true; };
  }, [toml]);

  const updateCards = useCallback(
    (next: RuleCard[]) => {
      setCards(next);
      const generated = cardsToToml(next, workflowName, workflowVersion, toml);
      lastEmitted.current = generated;
      onChange(generated);
    },
    [workflowName, workflowVersion, toml, onChange],
  );

  const emitMeta = (name: string, version: string) => {
    const generated = cardsToToml(cards, name, version, toml);
    lastEmitted.current = generated;
    onChange(generated);
  };

  const patchCard = (index: number, patch: Partial<RuleCard>) => {
    const next = cards.map((c, i) => (i === index ? { ...c, ...patch } : c));
    updateCards(next);
  };

  const splitList = (value: string): string[] =>
    value
      .split(',')
      .map((s) => s.trim())
      .filter(Boolean);

  return (
    <div>
      <div className="guided-meta">
        <label className="field" style={{ minWidth: 220 }}>
          <Glossary term="pipeline">{t('guided.workflowName')}</Glossary>
          <input className="mono" value={workflowName}
            onChange={(e) => { setWorkflowName(e.target.value); emitMeta(e.target.value, workflowVersion); }} />
        </label>
        <label className="field" style={{ width: 110 }}>
          {t('guided.version')}
          <input value={workflowVersion}
            onChange={(e) => { setWorkflowVersion(e.target.value); emitMeta(workflowName, e.target.value); }} />
        </label>
      </div>

      {cards.map((card, index) => (
        <div key={index} className="dash-card" style={{ marginBottom: '0.75rem' }}>
          <div className="row" style={{ marginBottom: '0.6rem', flexWrap: 'nowrap' }}>
            <span className="tag" style={{ fontWeight: 700 }}>{t('guided.ruleNumber').replace('{{n}}', String(index + 1))}</span>
            <input
              className="search-input mono"
              style={{ flex: 1, minWidth: 120 }}
              placeholder={t('guided.ruleNamePlaceholder')}
              value={card.name}
              onChange={(e) => patchCard(index, { name: e.target.value })}
              aria-label={t('guided.ruleNameAria').replace('{{n}}', String(index + 1))}
            />
            <button className="icon-btn danger" title={t('guided.removeRule')} aria-label={t('guided.removeRuleAria').replace('{{n}}', String(index + 1))}
              onClick={() => updateCards(cards.filter((_, i) => i !== index))}>
              <Trash2 size={13} />
            </button>
          </div>

          <label className="field" style={{ marginBottom: '0.6rem' }}>
            <Glossary term="rule">{t('guided.command')}</Glossary>
            <textarea
              className="mono"
              rows={2}
              style={{ resize: 'vertical' }}
              placeholder={t('guided.commandPlaceholder')}
              value={card.shell}
              onChange={(e) => patchCard(index, { shell: e.target.value })}
              aria-label={t('guided.commandAria').replace('{{n}}', String(index + 1))}
            />
          </label>

          <div className="rule-fields">
            <label className="field">
              <span>{t('guided.inputs')} <span className="muted">{t('guided.inputsHint')}</span></span>
              <input className="mono"
                value={card.inputs.join(', ')}
                onChange={(e) => patchCard(index, { inputs: splitList(e.target.value) })}
                aria-label={t('guided.inputsAria').replace('{{n}}', String(index + 1))} />
            </label>
            <label className="field">
              {t('guided.outputs')}
              <input className="mono"
                value={card.outputs.join(', ')}
                onChange={(e) => patchCard(index, { outputs: splitList(e.target.value) })}
                aria-label={t('guided.outputsAria').replace('{{n}}', String(index + 1))} />
            </label>
            <label className="field">
              {t('guided.threads')}
              <input type="number" min={1}
                value={card.threads}
                onChange={(e) => patchCard(index, { threads: e.target.value })}
                aria-label={t('guided.threadsAria').replace('{{n}}', String(index + 1))} />
            </label>
            <label className="field">
              {t('guided.memory')}
              <input
                placeholder={t('guided.memoryPlaceholder')}
                value={card.memory}
                onChange={(e) => patchCard(index, { memory: e.target.value })}
                aria-label={t('guided.memoryAria').replace('{{n}}', String(index + 1))} />
            </label>
            <label className="field">
              {t('guided.environment')}
              <input className="mono"
                placeholder={t('guided.environmentPlaceholder')}
                value={card.environment}
                onChange={(e) => patchCard(index, { environment: e.target.value })}
                aria-label={t('guided.environmentAria').replace('{{n}}', String(index + 1))} />
            </label>
          </div>
        </div>
      ))}

      <button className="btn-sm" onClick={() => updateCards([...cards, { ...EMPTY_CARD }])}>
        <Plus size={13} /> {t('guided.addRule')}
      </button>

      <p className="muted" style={{ marginTop: '0.75rem' }}>
        {t('guided.hint')}
      </p>
    </div>
  );
}
