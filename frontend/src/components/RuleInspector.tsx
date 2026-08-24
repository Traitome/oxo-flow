import { useCallback, useState } from 'react';
import { Save, X } from 'lucide-react';
import Modal from './Modal';
import { useI18n } from '../context/I18n';

/** The fields the inspector edits; everything else stays in the TOML pane. */
interface RuleFormState {
  name: string;
  description: string;
  shell: string;
  script: string;
  input: string[];
  output: string[];
  environment: string;
  environmentSpec: string;
  threads: string;
  memory: string;
  gpu: string;
  disk: string;
  timeLimit: string;
  envvars: Array<{ key: string; value: string }>;
  when: string;
  retries: string;
  tags: string;
  optional: boolean;
  required: boolean;
  log: string;
  benchmark: string;
}

const ENV_BACKENDS = ['system', 'conda', 'mamba', 'docker', 'singularity', 'venv', 'modules'];

function emptyForm(name: string): RuleFormState {
  return {
    name,
    description: '',
    shell: '',
    script: '',
    input: [],
    output: [],
    environment: 'system',
    environmentSpec: '',
    threads: '',
    memory: '',
    gpu: '',
    disk: '',
    timeLimit: '',
    envvars: [],
    when: '',
    retries: '',
    tags: '',
    optional: false,
    required: false,
    log: '',
    benchmark: '',
  };
}

/** Prefill from the serialized rule the DAG node carries. */
function formFromRule(name: string, rule: Record<string, unknown>): RuleFormState {
  const f = emptyForm(name);
  f.name = typeof rule.name === 'string' ? rule.name : name;
  f.description = typeof rule.description === 'string' ? rule.description : '';
  f.shell = typeof rule.shell === 'string' ? rule.shell : '';
  f.script = typeof rule.script === 'string' ? rule.script : '';
  f.input = Array.isArray(rule.input) ? rule.input.filter((v): v is string => typeof v === 'string') : [];
  f.output = Array.isArray(rule.output) ? rule.output.filter((v): v is string => typeof v === 'string') : [];
  // Environment may be an object ({ backend, spec }) OR a string
  // ("bioconda::fastp") as emitted by the AI generator — the string form
  // was silently dropped before (issue #81).
  const rawEnv = rule.environment;
  // "bioconda::fastp" → { bioconda: "fastp" } so the backend-key lookup
  // below (ENV_BACKENDS) finds it; "conda" stays conda, unknown backends
  // map to conda with the full string as spec (never silently dropped).
  const env = typeof rawEnv === 'string'
    ? (() => {
        const [backend, spec] = rawEnv.split('::');
        const key = ENV_BACKENDS.includes(backend) ? backend : 'conda';
        const out: Record<string, unknown> = {};
        out[key] = spec || backend || rawEnv;
        return out;
      })()
    : ((rawEnv ?? {}) as Record<string, unknown>);
  const envKey = ENV_BACKENDS.find((k) => env[k] !== undefined && env[k] !== null);
  if (envKey) {
    f.environment = envKey;
    const spec = env[envKey];
    f.environmentSpec = Array.isArray(spec) ? spec.map(String).join(', ') : String(spec);
  }
  const resources = (rule.resources ?? {}) as Record<string, unknown>;
  f.threads = resources.threads !== undefined && resources.threads !== null ? String(resources.threads) : '';
  f.memory = typeof resources.memory === 'string' ? resources.memory : '';
  f.gpu = resources.gpu !== undefined && resources.gpu !== null ? String(resources.gpu) : '';
  f.disk = typeof resources.disk === 'string' ? resources.disk : '';
  f.timeLimit = typeof resources.time_limit === 'string' ? resources.time_limit : '';
  const envvars = (rule.envvars ?? {}) as Record<string, unknown>;
  f.envvars = Object.entries(envvars).map(([key, value]) => ({ key, value: String(value) }));
  f.when = typeof rule.when === 'string' ? rule.when : '';
  f.retries = rule.retries !== undefined && rule.retries !== null ? String(rule.retries) : '';
  f.tags = Array.isArray(rule.tags) ? rule.tags.filter((v): v is string => typeof v === 'string').join(', ') : '';
  f.optional = rule.optional === true;
  f.required = rule.required === true;
  f.log = typeof rule.log === 'string' ? rule.log : '';
  f.benchmark = typeof rule.benchmark === 'string' ? rule.benchmark : '';
  return f;
}

export interface RuleInspectorProps {
  /** Rule name being edited. */
  ruleName: string;
  /** The full serialized rule (from the DAG node) or null for a new rule. */
  rule: Record<string, unknown> | null;
  /** Called with the update_rule patch (only changed/complete sub-objects). */
  onSave: (patch: Record<string, unknown>) => void;
  onClose: () => void;
}

type StringFieldKey = {
  [K in keyof RuleFormState]: RuleFormState[K] extends string ? K : never;
}[keyof RuleFormState];

export default function RuleInspector({ ruleName, rule, onSave, onClose }: RuleInspectorProps) {
  const { t } = useI18n();
  // The inspector remounts per rule via the `key` prop on the parent, so the
  // form initializes from props once — no effect-sync needed.
  const [form, setForm] = useState<RuleFormState>(() =>
    rule ? formFromRule(ruleName, rule) : emptyForm(ruleName),
  );

  const set = <K extends keyof RuleFormState>(key: K, value: RuleFormState[K]) =>
    setForm((prev) => ({ ...prev, [key]: value }));

  const setListItem = useCallback(
    (key: 'input' | 'output', index: number, value: string) =>
      setForm((prev) => {
        const next = [...prev[key]];
        next[index] = value;
        return { ...prev, [key]: next };
      }),
    [],
  );

  const addListItem = useCallback(
    (key: 'input' | 'output') => setForm((prev) => ({ ...prev, [key]: [...prev[key], ''] })),
    [],
  );

  const removeListItem = useCallback(
    (key: 'input' | 'output', index: number) =>
      setForm((prev) => {
        const next = prev[key].filter((_, i) => i !== index);
        return { ...prev, [key]: next };
      }),
    [],
  );

  const handleSave = () => {
    const patch: Record<string, unknown> = {
      description: form.description,
      shell: form.shell,
      script: form.script,
      input: form.input.filter((v) => v.trim() !== ''),
      output: form.output.filter((v) => v.trim() !== ''),
      when: form.when,
      retries: form.retries === '' ? null : Number(form.retries),
      tags: form.tags.split(',').map((t) => t.trim()).filter((t) => t !== ''),
      optional: form.optional,
      required: form.required,
      log: form.log,
      benchmark: form.benchmark,
    };
    if (form.environment !== 'system') {
      patch.environment = { [form.environment]: form.environmentSpec };
    } else if (form.environmentSpec === '') {
      // Removing the environment: the patch replaces the table wholesale.
      patch.environment = null;
    }
    const resources: Record<string, unknown> = {};
    if (form.threads !== '') resources.threads = Number(form.threads);
    if (form.memory !== '') resources.memory = form.memory;
    if (form.gpu !== '') resources.gpu = Number(form.gpu);
    if (form.disk !== '') resources.disk = form.disk;
    if (form.timeLimit !== '') resources.time_limit = form.timeLimit;
    if (Object.keys(resources).length > 0) patch.resources = resources;
    if (form.envvars.length > 0) {
      const envvars: Record<string, string> = {};
      for (const { key, value } of form.envvars) {
        if (key.trim() !== '') envvars[key.trim()] = value;
      }
      patch.envvars = envvars;
    }
    onSave(patch);
  };

  const stringList = (key: 'input' | 'output') => (
    <div className="inspector-list">
      {form[key].map((item, i) => (
        <div className="inspector-list-row" key={i}>
          <input
            value={item}
            onChange={(e) => setListItem(key, i, e.target.value)}
            placeholder={key === 'input' ? t('inspector.inputPlaceholder') : t('inspector.outputPlaceholder')}
          />
          <button type="button" className="btn-sm" onClick={() => removeListItem(key, i)} title={t('inspector.remove')}>
            <X size={12} />
          </button>
        </div>
      ))}
      <button type="button" className="btn-sm" onClick={() => addListItem(key)}>
        {key === 'input' ? t('inspector.addInput') : t('inspector.addOutput')}
      </button>
    </div>
  );

  const textInput = (
    label: string,
    key: StringFieldKey,
    placeholder?: string,
    type: 'text' | 'number' = 'text',
  ) => (
    <label className="inspector-field">
      <span>{label}</span>
      <input
        type={type}
        value={form[key]}
        onChange={(e) => set(key, e.target.value)}
        placeholder={placeholder}
      />
    </label>
  );

  return (
    <Modal onClose={onClose} labelledBy="rule-inspector-title" className="inspector-dialog">
      <div className="inspector-header">
        <h3 id="rule-inspector-title">{t('inspector.title').replace('{{name}}', form.name)}</h3>
        <button className="btn-sm" onClick={onClose} title={t('inspector.close')} aria-label={t('inspector.close')}>
          <X size={14} />
        </button>
      </div>

      <div className="inspector-body">
        {textInput(t('inspector.description'), 'description', t('inspector.descriptionPlaceholder'))}
        <label className="inspector-field">
          <span>{t('inspector.shell')}</span>
          <textarea
            className="inspector-shell"
            value={form.shell}
            onChange={(e) => set('shell', e.target.value)}
            placeholder={t('inspector.shellPlaceholder')}
            rows={3}
          />
        </label>
        {textInput(t('inspector.script'), 'script', t('inspector.scriptPlaceholder'))}

        <div className="inspector-section">{t('inspector.inputs')}</div>
        {stringList('input')}
        <div className="inspector-section">{t('inspector.outputs')}</div>
        {stringList('output')}

        <div className="inspector-grid">
          <label className="inspector-field">
            <span>{t('inspector.environment')}</span>
            <select value={form.environment} onChange={(e) => set('environment', e.target.value)}>
              {ENV_BACKENDS.map((b) => (
                <option key={b} value={b}>
                  {b}
                </option>
              ))}
            </select>
          </label>
          {textInput(t('inspector.environmentSpec'), 'environmentSpec', t('inspector.environmentSpecPlaceholder'))}
        </div>

        <div className="inspector-section">{t('inspector.resources')}</div>
        <div className="inspector-grid">
          {textInput(t('inspector.threads'), 'threads', t('inspector.threadsPlaceholder'), 'number')}
          {textInput(t('inspector.memory'), 'memory', t('inspector.memoryPlaceholder'))}
          {textInput(t('inspector.gpu'), 'gpu', t('inspector.gpuPlaceholder'), 'number')}
          {textInput(t('inspector.disk'), 'disk', t('inspector.diskPlaceholder'))}
          {textInput(t('inspector.timeLimit'), 'timeLimit', t('inspector.timeLimitPlaceholder'))}
        </div>

        <div className="inspector-section">{t('inspector.envvars')}</div>
        {form.envvars.map((ev, i) => (
          <div className="inspector-list-row" key={i}>
            <input
              placeholder={t('inspector.envvarNamePlaceholder')}
              value={ev.key}
              onChange={(e) =>
                setForm((prev) => {
                  const next = [...prev.envvars];
                  next[i] = { ...next[i], key: e.target.value };
                  return { ...prev, envvars: next };
                })
              }
            />
            <input
              placeholder={t('inspector.envvarValuePlaceholder')}
              value={ev.value}
              onChange={(e) =>
                setForm((prev) => {
                  const next = [...prev.envvars];
                  next[i] = { ...next[i], value: e.target.value };
                  return { ...prev, envvars: next };
                })
              }
            />
            <button
              type="button"
              className="btn-sm"
              onClick={() => setForm((prev) => ({ ...prev, envvars: prev.envvars.filter((_, j) => j !== i) }))}
              title={t('inspector.remove')}
            >
              <X size={12} />
            </button>
          </div>
        ))}
        <button
          type="button"
          className="btn-sm"
          onClick={() => setForm((prev) => ({ ...prev, envvars: [...prev.envvars, { key: '', value: '' }] }))}
        >
          {t('inspector.addVariable')}
        </button>

        <div className="inspector-grid">
          {textInput(t('inspector.condition'), 'when', t('inspector.conditionPlaceholder'))}
          {textInput(t('inspector.retries'), 'retries', t('inspector.retriesPlaceholder'), 'number')}
          {textInput(t('inspector.tags'), 'tags', t('inspector.tagsPlaceholder'))}
          {textInput(t('inspector.log'), 'log', t('inspector.logPlaceholder'))}
          {textInput(t('inspector.benchmark'), 'benchmark', t('inspector.benchmarkPlaceholder'))}
        </div>

        <div className="inspector-grid">
          <label className="inspector-check">
            <input type="checkbox" checked={form.optional} onChange={(e) => set('optional', e.target.checked)} />
            <span>{t('inspector.optional')}</span>
          </label>
          <label className="inspector-check">
            <input type="checkbox" checked={form.required} onChange={(e) => set('required', e.target.checked)} />
            <span>{t('inspector.required')}</span>
          </label>
        </div>
      </div>

      <div className="modal-actions">
        <button className="btn-sm" onClick={onClose}>
          {t('inspector.cancel')}
        </button>
        <button className="btn-run" onClick={handleSave}>
          <Save size={14} /> {t('inspector.apply')}
        </button>
      </div>
    </Modal>
  );
}
