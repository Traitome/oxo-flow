import { useCallback, useEffect, useState } from 'react';
import { Plus, Search } from 'lucide-react';
import { api } from '../api/client';
import type { KnowledgeTool } from '../api/types';
import { useI18n } from '../context/I18n';

export interface ToolPaletteProps {
  /** Called when the user picks a tool to add as a new rule. */
  onAddTool: (tool: KnowledgeTool) => void;
}

const DEBOUNCE_MS = 300;

export default function ToolPalette({ onAddTool }: ToolPaletteProps) {
  const { t } = useI18n();
  const [query, setQuery] = useState('');
  const [tools, setTools] = useState<KnowledgeTool[]>([]);
  const [total, setTotal] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Grounded search over the embedded Bioconda database (6103 tools).
  // An empty query shows the hint instead of an arbitrary DB slice — the
  // results are gated at render time, so no state resets are needed here.
  const effectiveQuery = query.trim();
  useEffect(() => {
    if (effectiveQuery === '') return;
    const timer = setTimeout(async () => {
      setLoading(true);
      setError(null);
      try {
        const res = await api.knowledgeTools(query, 20);
        setTools(res.tools);
        setTotal(res.total);
      } catch (e: unknown) {
        setError(e instanceof Error ? e.message : t('toolPalette.error'));
        setTools([]);
        setTotal(null);
      } finally {
        setLoading(false);
      }
    }, DEBOUNCE_MS);
    return () => clearTimeout(timer);
    // `query` drives the effect; effectiveQuery is its trimmed form.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query]);

  const handleAdd = useCallback(
    (tool: KnowledgeTool) => {
      onAddTool(tool);
    },
    [onAddTool],
  );

  return (
    <div className="tool-palette">
      <div className="tool-palette-head">
        <span className="tool-palette-title">{t('toolPalette.title')}</span>
        {total !== null && <span className="tool-palette-total">{total.toLocaleString()} tools</span>}
      </div>
      <div className="tool-palette-search">
        <Search size={14} />
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t('toolPalette.searchPlaceholder')}
          aria-label={t('toolPalette.searchAria')}
        />
      </div>
      <div className="tool-palette-results">
        {loading && <div className="tool-palette-hint">{t('toolPalette.loading')}</div>}
        {!loading && error && <div className="tool-palette-hint error">{error}</div>}
        {effectiveQuery === '' && (
          <div className="tool-palette-hint">
            {t('toolPalette.hint')}
          </div>
        )}
        {effectiveQuery !== '' &&
          !loading &&
          !error &&
          tools.length === 0 && (
            <div className="tool-palette-hint">
              {t('toolPalette.empty').replace('{{query}}', effectiveQuery)}
            </div>
          )}
        {effectiveQuery !== '' &&
          !loading &&
          !error &&
          tools.map((tool) => (
            <div className="tool-palette-item" key={tool.name}>
              <div className="tool-palette-item-main">
                <span className="tool-palette-name">{tool.name}</span>
                <span className="tool-palette-version">{tool.version}</span>
                <button
                  className="btn-sm tool-palette-add"
                  onClick={() => handleAdd(tool)}
                  title={t('toolPalette.addTitle').replace('{{tool}}', tool.name)}
                >
                  <Plus size={12} />
                </button>
              </div>
              {tool.summary && <div className="tool-palette-summary">{tool.summary}</div>}
            </div>
          ))}
      </div>
    </div>
  );
}
