// Guided mode (issue #82 P1-5): a form-based rule builder for users who
// never want to see TOML. Each analysis step is a card with the fields
// that matter (name, command, inputs, outputs, threads, memory,
// environment). Cards round-trip through the backend parser — the TOML
// stays the single source of truth and the canvas stays in sync.

import { useCallback, useEffect, useRef, useState } from 'react';
import { Plus, Trash2 } from 'lucide-react';
import { api } from '../api/client';
import Glossary from './Glossary';

interface RuleCard {
  name: string;
  shell: string;
  inputs: string[];
  outputs: string[];
  threads: string;
  memory: string;
  environment: string;
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
};

function cardsToToml(cards: RuleCard[], workflowName: string, workflowVersion: string): string {
  const rules = cards
    .filter((c) => c.name.trim() !== '')
    .map((c) => {
      const lines: string[] = ['[[rules]]', `name = "${c.name.trim()}"`];
      if (c.inputs.length > 0) {
        const list = c.inputs.map((i) => `"${i.trim()}"`).join(', ');
        lines.push(`input = [${list}]`);
      }
      if (c.outputs.length > 0) {
        const list = c.outputs.map((o) => `"${o.trim()}"`).join(', ');
        lines.push(`output = [${list}]`);
      }
      if (c.threads.trim()) lines.push(`threads = ${c.threads.trim()}`);
      if (c.memory.trim()) lines.push(`memory = "${c.memory.trim()}"`);
      if (c.environment.trim() && c.environment !== 'system') {
        lines.push(`environment = "${c.environment.trim()}"`);
      }
      const shell = c.shell.replace(/"/g, '\\"');
      lines.push(`shell = """${shell}"""`);
      return lines.join('\n');
    })
    .join('\n\n');

  return `[workflow]\nname = "${workflowName}"\nversion = "${workflowVersion}"\n\n${rules}\n`;
}

export default function GuidedRuleBuilder({ toml, onChange }: GuidedRuleBuilderProps) {
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
    api
      .parse(toml)
      .then((parsed) => {
        if (cancelled) return;
        lastEmitted.current = toml;
        setWorkflowName(parsed.name || 'my-pipeline');
        setWorkflowVersion(parsed.version || '0.1.0');
        setCards(
          parsed.rules.map((r) => ({
            name: r.name,
            shell: r.shell ?? '',
            inputs: r.inputs,
            outputs: r.outputs,
            threads: (r.threads ?? 0) > 0 ? String(r.threads) : '',
            memory: '',
            environment: r.environment || 'system',
          })),
        );
      })
      .catch(() => setCards([]));
    return () => { cancelled = true; };
  }, [toml]);

  const updateCards = useCallback(
    (next: RuleCard[]) => {
      setCards(next);
      const generated = cardsToToml(next, workflowName, workflowVersion);
      lastEmitted.current = generated;
      onChange(generated);
    },
    [workflowName, workflowVersion, onChange],
  );

  const emitMeta = (name: string, version: string) => {
    const generated = cardsToToml(cards, name, version);
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
      <div style={{ display: 'flex', gap: '0.75rem', marginBottom: '0.75rem', flexWrap: 'wrap' }}>
        <label style={{ fontSize: '0.82rem', display: 'flex', flexDirection: 'column', gap: '4px' }}>
          <Glossary term="pipeline">Workflow name</Glossary>
          <input className="search-input" value={workflowName}
            onChange={(e) => { setWorkflowName(e.target.value); emitMeta(e.target.value, workflowVersion); }} />
        </label>
        <label style={{ fontSize: '0.82rem', display: 'flex', flexDirection: 'column', gap: '4px' }}>
          Version
          <input className="search-input" value={workflowVersion} style={{ width: 90 }}
            onChange={(e) => { setWorkflowVersion(e.target.value); emitMeta(workflowName, e.target.value); }} />
        </label>
      </div>

      {cards.map((card, index) => (
        <div key={index} className="dash-card" style={{ marginBottom: '0.75rem' }}>
          <div style={{ display: 'flex', gap: '0.5rem', alignItems: 'center', marginBottom: '0.5rem' }}>
            <span className="tag" style={{ fontWeight: 700 }}>Rule {index + 1}</span>
            <input
              className="search-input mono"
              style={{ flex: 1, minWidth: 160 }}
              placeholder="rule name (e.g. fastp_trim)"
              value={card.name}
              onChange={(e) => patchCard(index, { name: e.target.value })}
              aria-label={`Rule ${index + 1} name`}
            />
            <button className="btn-sm btn-error" title="Remove rule"
              onClick={() => updateCards(cards.filter((_, i) => i !== index))}>
              <Trash2 size={13} />
            </button>
          </div>

          <label style={{ fontSize: '0.82rem', display: 'block', marginBottom: '0.5rem' }}>
            <Glossary term="rule">Command</Glossary>
            <textarea
              className="search-input mono"
              rows={2}
              style={{ width: '100%', marginTop: '4px', resize: 'vertical' }}
              placeholder="fastp -i {sample}.fq -o clean_{sample}.fq"
              value={card.shell}
              onChange={(e) => patchCard(index, { shell: e.target.value })}
              aria-label={`Rule ${index + 1} command`}
            />
          </label>

          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(180px, 1fr))', gap: '0.5rem' }}>
            <label style={{ fontSize: '0.8rem' }}>
              Inputs <span style={{ color: 'var(--color-text-tertiary)' }}>(comma-separated, <Glossary term="wildcard">wildcards</Glossary> ok)</span>
              <input className="search-input mono" style={{ marginTop: '4px' }}
                value={card.inputs.join(', ')}
                onChange={(e) => patchCard(index, { inputs: splitList(e.target.value) })}
                aria-label={`Rule ${index + 1} inputs`} />
            </label>
            <label style={{ fontSize: '0.8rem' }}>
              Outputs
              <input className="search-input mono" style={{ marginTop: '4px' }}
                value={card.outputs.join(', ')}
                onChange={(e) => patchCard(index, { outputs: splitList(e.target.value) })}
                aria-label={`Rule ${index + 1} outputs`} />
            </label>
            <label style={{ fontSize: '0.8rem' }}>
              Threads
              <input className="search-input" style={{ marginTop: '4px', width: 80 }} type="number" min={1}
                value={card.threads}
                onChange={(e) => patchCard(index, { threads: e.target.value })}
                aria-label={`Rule ${index + 1} threads`} />
            </label>
            <label style={{ fontSize: '0.8rem' }}>
              Memory
              <input className="search-input" style={{ marginTop: '4px', width: 100 }}
                placeholder="e.g. 4GB"
                value={card.memory}
                onChange={(e) => patchCard(index, { memory: e.target.value })}
                aria-label={`Rule ${index + 1} memory`} />
            </label>
            <label style={{ fontSize: '0.8rem' }}>
              Environment
              <input className="search-input mono" style={{ marginTop: '4px' }}
                placeholder="system | bioconda::fastp | conda::env"
                value={card.environment}
                onChange={(e) => patchCard(index, { environment: e.target.value })}
                aria-label={`Rule ${index + 1} environment`} />
            </label>
          </div>
        </div>
      ))}

      <button className="btn-sm" onClick={() => updateCards([...cards, { ...EMPTY_CARD }])}>
        <Plus size={13} /> Add rule
      </button>

      <p style={{ fontSize: '0.78rem', color: 'var(--color-text-tertiary)', marginTop: '0.75rem' }}>
        Every change is converted to workflow TOML immediately — switch to the canvas view to
        see the <Glossary term="pipeline">pipeline</Glossary> graph or fine-tune advanced
        options like <Glossary term="depends_on">depends_on</Glossary>.
      </p>
    </div>
  );
}
