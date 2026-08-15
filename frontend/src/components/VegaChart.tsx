import { useEffect, useRef } from 'react';

import embed from 'vega-embed';
import { expressionInterpreter } from 'vega-interpreter';

interface VegaChartProps {
  spec: Record<string, unknown>;
  data?: Array<Record<string, unknown>>;
  title?: string;
}

/// Theme-aware Vega colors: charts must follow the app's light/dark theme,
/// never hardcoded light-only hex (readability in dark mode).
function chartTheme(): { label: string; title: string; grid: string } {
  if (typeof document !== 'undefined' && document.documentElement.dataset.theme === 'dark') {
    return { label: '#94A3B8', title: '#E2E8F0', grid: '#263449' };
  }
  if (
    typeof window !== 'undefined' &&
    window.matchMedia('(prefers-color-scheme: dark)').matches
  ) {
    return { label: '#94A3B8', title: '#E2E8F0', grid: '#263449' };
  }
  return { label: '#475569', title: '#0F172A', grid: '#E2E8F0' };
}

export default function VegaChart({ spec, data, title }: VegaChartProps) {
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!containerRef.current || !spec) return;

    const theme = chartTheme();
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
          labelColor: theme.label,
          titleColor: theme.title,
          gridColor: theme.grid,
        },
        legend: {
          labelFontSize: 11,
          titleFontSize: 12,
          labelColor: theme.label,
          titleColor: theme.title,
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
      // CSP-safe expressions (issue #79 P2 "unsafe-eval on every page"):
      // vega's default codegen compiles expressions with `new Function`,
      // which our script-src policy blocks. AST mode + the official
      // interpreter evaluates the same expressions without eval.
      ast: true,
      expr: expressionInterpreter,
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
