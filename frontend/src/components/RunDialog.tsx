import { useEffect, useState } from 'react';
import { CheckCircle, Play, X } from 'lucide-react';
import { api } from '../api/client';
import type { ClusterInfo } from '../api/types';
import { useI18n } from '../context/I18n';
import Modal from './Modal';

export interface RunOptions {
  maxJobs: number;
  keepGoing: boolean;
  samples: string[];
  targets: string[];
  /** Execute on this SSH cluster connection instead of the local host. */
  clusterId?: string;
}

export interface RunDialogProps {
  onClose: () => void;
  /** dryRun=true produces the read-only execution plan. */
  onSubmit: (dryRun: boolean, options: RunOptions) => void;
}

export default function RunDialog({ onClose, onSubmit }: RunDialogProps) {
  const { t } = useI18n();
  const [maxJobs, setMaxJobs] = useState('4');
  const [keepGoing, setKeepGoing] = useState(false);
  const [samples, setSamples] = useState('');
  const [targets, setTargets] = useState('');
  const [clusterId, setClusterId] = useState('');
  const [clusters, setClusters] = useState<ClusterInfo[]>([]);

  useEffect(() => {
    api.listClusters().then((c) => setClusters(c.filter((x) => x.enabled))).catch(() => setClusters([]));
  }, []);

  const options = (): RunOptions => ({
    maxJobs: Number(maxJobs) || 4,
    keepGoing,
    samples: samples.split(',').map((s) => s.trim()).filter((s) => s !== ''),
    targets: targets.split(',').map((t) => t.trim()).filter((t) => t !== ''),
    clusterId: clusterId || undefined,
  });

  return (
    <Modal onClose={onClose} labelledBy="run-dialog-title">
      <div className="inspector-header">
        <h3 id="run-dialog-title">{t('run.title')}</h3>
        <button className="btn-sm" onClick={onClose} title={t('run.close')} aria-label={t('run.close')}>
          <X size={14} />
        </button>
      </div>
      <div className="inspector-body">
        <div className="inspector-grid">
          <label className="inspector-field">
            <span>{t('run.parallelJobs')}</span>
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
            <span>{t('run.samples')}</span>
            <input
              placeholder="S1, S2"
              value={samples}
              onChange={(e) => setSamples(e.target.value)}
            />
          </label>
          <label className="inspector-field">
            <span>{t('run.targets')}</span>
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
          <span>{t('run.keepGoing')}</span>
        </label>
        {clusters.length > 0 && (
          <label className="inspector-field">
            <span>{t('run.cluster')}</span>
            <select value={clusterId} onChange={(e) => setClusterId(e.target.value)}>
              <option value="">{t('run.local')}</option>
              {clusters.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name} ({c.ssh_host}){c.scheduler && c.scheduler !== 'auto' ? ` · ${c.scheduler}` : ''}
                </option>
              ))}
            </select>
          </label>
        )}
        <p className="run-dialog-hint">
          {t('run.hint')}
        </p>
      </div>
      <div className="modal-actions">
        <button className="btn-sm" onClick={onClose}>
          {t('run.cancel')}
        </button>
        <button
          className="btn-sm"
          style={{ background: 'transparent', border: '1px solid var(--color-border)' }}
          onClick={() => onSubmit(true, options())}
        >
          <CheckCircle size={14} /> {t('run.dryRun')}
        </button>
        <button className="btn-run" onClick={() => onSubmit(false, options())}>
          <Play size={14} /> {t('run.runBtn')}
        </button>
      </div>
    </Modal>
  );
}
