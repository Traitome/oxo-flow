// Guided mode (issue #82 P1-5): a form-based rule builder for users who
// never want to see TOML. Each analysis step is a card with the fields
// that matter (name, command, inputs, outputs, threads, memory,
// environment). Cards round-trip through the backend parser — the TOML
// stays the single source of truth and the canvas stays in sync.

import { useCallback, useEffect, useRef, useState } from 'react';
import { Plus, Trash2 } from 'lucide-react';
import { api } from '../api/client';
import Glossary from './Glossary';
import { useI18n } from '../context/I18n';

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
