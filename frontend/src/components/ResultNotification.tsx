import { usePipelineSession } from '../context/PipelineSession';
import { X, ExternalLink } from 'lucide-react';
import { useNavigate } from 'react-router-dom';

export default function ResultNotification() {
  const session = usePipelineSession();
  const navigate = useNavigate();
  const result = session.state.lastRunResult;
  if (!result) return null;

  const bg = result.type === 'success' ? 'var(--color-success-bg)'
    : result.type === 'error' ? 'var(--color-error-bg)'
    : 'var(--color-info-bg)';
  const border = result.type === 'success' ? 'var(--color-success)'
    : result.type === 'error' ? 'var(--color-error)'
    : 'var(--color-info)';
  const color = result.type === 'success' ? 'var(--color-success)'
    : result.type === 'error' ? 'var(--color-error)'
    : 'var(--color-info)';

  return (
    <div style={{
      background: bg, border: `1px solid ${border}`, color,
      borderRadius: 'var(--radius-sm)', padding: '0.5rem 0.75rem',
      margin: '0 0 0.75rem', fontSize: '0.85rem',
      display: 'flex', alignItems: 'center', gap: '0.5rem',
    }}>
      <span style={{ flex: 1 }}>{result.message}</span>
      {result.runId && (
        <button className="btn-sm" style={{ background: 'transparent', border: `1px solid ${border}`, color }}
          onClick={() => navigate(`/runs/${result.runId}`)}>
          <ExternalLink size={12} /> View
        </button>
      )}
      <button className="btn-sm" style={{ background: 'transparent', border: 'none', color, padding: '2px 4px', cursor: 'pointer' }}
        onClick={() => session.setRunResult(null)} title="Dismiss">
        <X size={14} />
      </button>
    </div>
  );
}
