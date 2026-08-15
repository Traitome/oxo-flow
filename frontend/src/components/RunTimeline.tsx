// RunTimeline — the terminal-native signature element.
//
// A run's rules render as a vertical timeline in the idiom the audience
// already reads every day: a shell transcript. Each node is a prompt line
// (`$ oxo-flow run <rule>`), its terminal symbol carries the state
// (✓ done / ✗ failed / ⏳ running / ○ pending), and duration+exit land in
// the mono gutter. The running dot "breathes" like a phosphor cursor
// (disabled under prefers-reduced-motion).

import type { DagStatus } from '../api/types';

interface RunTimelineProps {
  dag: DagStatus;
}

function stateSymbol(status: string): string {
  switch (status) {
    case 'completed':
    case 'success':
      return '✓';
    case 'failed':
      return '✗';
    case 'running':
      return '⏳';
    case 'skipped':
      return '·';
    default:
      return '○';
  }
}

function formatDuration(ms: number | null): string {
  if (ms === null || ms === undefined) return '';
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)}s`;
  return `${Math.floor(s / 60)}m ${(s % 60).toFixed(0)}s`;
}

export default function RunTimeline({ dag }: RunTimelineProps) {
  return (
    <div className="run-timeline">
      {dag.nodes.map((n) => (
        <div key={n.id} className={`run-tl-node ${n.status}`}>
          <span className={`run-tl-symbol ${n.status}`}>{stateSymbol(n.status)}</span>
          <span className="run-tl-cmd">
            <span className="run-tl-prompt">$</span> oxo-flow run <span className="run-tl-rule">{n.label}</span>
          </span>
          <span className="run-tl-meta">
            {formatDuration(n.duration_ms)}
            {n.exit_code !== null && n.exit_code !== undefined && n.exit_code !== 0
              ? ` · exit ${n.exit_code}`
              : ''}
          </span>
        </div>
      ))}
    </div>
  );
}
