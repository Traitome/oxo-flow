import { useEffect, useRef } from 'react';

import embed from 'vega-embed';

interface VegaChartProps {
  spec: Record<string, unknown>;
  data?: Array<Record<string, unknown>>;
  title?: string;
}

export default function VegaChart({ spec, data, title }: VegaChartProps) {
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!containerRef.current || !spec) return;

    const fullSpec = {
      ...spec,
      data: data ? { values: data } : spec.data || { values: [] },
      width: 'container' as unknown as number,
      height: 300,
      autosize: {
        type: 'fit' as const,
        contains: 'padding' as const,
      },
      config: {
        axis: {
          labelFontSize: 11,
          titleFontSize: 12,
          labelColor: '#475569',
          titleColor: '#0F172A',
          gridColor: '#E2E8F0',
        },
        legend: {
          labelFontSize: 11,
          titleFontSize: 12,
        },
        view: {
          stroke: 'transparent',
        },
        background: 'transparent',
      },
    };

    embed(containerRef.current, fullSpec as unknown as Parameters<typeof embed>[1], {
      actions: { export: true, source: false, compiled: false, editor: false },
      renderer: 'canvas',
    });

    return () => {
      if (containerRef.current) {
        containerRef.current.innerHTML = '';
      }
    };
  }, [spec, data]);

  return (
    <div className="dash-card" style={{ marginBottom: '1rem' }}>
      {title && (
        <h4 style={{ fontSize: '0.85rem', marginBottom: '8px', color: 'var(--color-text)' }}>
          {title}
        </h4>
      )}
      <div ref={containerRef} style={{ minHeight: '300px', width: '100%' }} />
    </div>
  );
}
