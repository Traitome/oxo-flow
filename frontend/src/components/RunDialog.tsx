import { useState } from 'react';
import { CheckCircle, Play, X } from 'lucide-react';

export interface RunOptions {
  maxJobs: number;
  keepGoing: boolean;
  samples: string[];
  targets: string[];
}

export interface RunDialogProps {
  onClose: () => void;
  /** dryRun=true produces the read-only execution plan. */
  onSubmit: (dryRun: boolean, options: RunOptions) => void;
}

export default function RunDialog({ onClose, onSubmit }: RunDialogProps) {
  const [maxJobs, setMaxJobs] = useState('4');
  const [keepGoing, setKeepGoing] = useState(false);
  const [samples, setSamples] = useState('');
  const [targets, setTargets] = useState('');

  const options = (): RunOptions => ({
    maxJobs: Number(maxJobs) || 4,
    keepGoing,
    samples: samples.split(',').map((s) => s.trim()).filter((s) => s !== ''),
    targets: targets.split(',').map((t) => t.trim()).filter((t) => t !== ''),
  });

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div
        className="modal-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="run-dialog-title"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="inspector-header">
          <h3 id="run-dialog-title">Run options</h3>
          <button className="btn-sm" onClick={onClose} title="Close">
            <X size={14} />
          </button>
        </div>
        <div className="inspector-body">
          <div className="inspector-grid">
            <label className="inspector-field">
              <span>Parallel jobs (max)</span>
              <input
                type="number"
                min={1}
                max={128}
                value={maxJobs}
                onChange={(e) => setMaxJobs(e.target.value)}
              />
            </label>
          </div>
          <div className="inspector-grid">
            <label className="inspector-field">
              <span>Samples — run only this subset (names, first:N, ready; empty = all)</span>
              <input
                placeholder="S1, S2"
                value={samples}
                onChange={(e) => setSamples(e.target.value)}
              />
            </label>
            <label className="inspector-field">
              <span>Target rules (comma-separated; empty = engine default)</span>
              <input
                placeholder="align, report"
                value={targets}
                onChange={(e) => setTargets(e.target.value)}
              />
            </label>
          </div>
          <label className="inspector-check">
            <input
              type="checkbox"
              checked={keepGoing}
              onChange={(e) => setKeepGoing(e.target.checked)}
            />
            <span>Keep going when a rule fails</span>
          </label>
          <p className="run-dialog-hint">
            Dry-Run produces the execution plan without running anything — the
            engine's checkpoint rules decide what would re-run. Start with a
            dry-run, then execute.
          </p>
        </div>
        <div className="modal-actions">
          <button className="btn-sm" onClick={onClose}>
            Cancel
          </button>
          <button
            className="btn-sm"
            style={{ background: 'transparent', border: '1px solid var(--color-border)' }}
            onClick={() => onSubmit(true, options())}
          >
            <CheckCircle size={14} /> Dry-Run (preview)
          </button>
          <button className="btn-run" onClick={() => onSubmit(false, options())}>
            <Play size={14} /> Run
          </button>
        </div>
      </div>
    </div>
  );
}
