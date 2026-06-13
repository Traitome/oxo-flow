import { useEffect, useRef } from 'react';
import cytoscape, { type Core, type ElementDefinition } from 'cytoscape';
import dagre from 'cytoscape-dagre';
cytoscape.use(dagre);

interface DagViewProps {
  nodes: Array<{ id: string; label: string; color?: string }>;
  edges: Array<{ source: string; target: string }>;
  onNodeClick?: (id: string) => void;
}

const STATUS_COLORS: Record<string, string> = {
  green: '#10B981', blue: '#3B82F6', red: '#EF4444', gray: '#94A3B8', lightgray: '#CBD5E1',
  completed: '#10B981', running: '#3B82F6', failed: '#EF4444', pending: '#CBD5E1', skipped: '#94A3B8',
};

export default function DagView({ nodes, edges, onNodeClick }: DagViewProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const cyRef = useRef<Core | null>(null);

  useEffect(() => {
    if (!containerRef.current || nodes.length === 0) return;

    const elements: ElementDefinition[] = [
      ...nodes.map((n) => ({
        data: { id: n.id, label: n.label, color: n.color || '#CBD5E1', status: n.color },
      })),
      ...edges.map((e) => ({ data: { id: `${e.source}→${e.target}`, label: '', source: e.source, target: e.target } })),
    ];

    // Map node colors using cytoscape style selectors
    const nodeStyles = STATUS_COLORS;
    const styleRules: any[] = [
      { selector: 'node', style: { label: 'data(label)', color: '#e2e8f0', 'font-size': '12px', 'text-valign': 'center', 'text-halign': 'center', width: 120, height: 40, shape: 'round-rectangle', 'border-width': 2, 'border-color': '#1e293b', 'background-color': '#6B7280' } },
      { selector: 'edge', style: { width: 2, 'line-color': '#475569', 'target-arrow-color': '#475569', 'target-arrow-shape': 'triangle', 'curve-style': 'bezier' } },
      { selector: 'node:selected', style: { 'border-color': '#2563EB', 'border-width': 3 } },
    ];
    // Add color-coded node styles per status
    for (const [status, color] of Object.entries(nodeStyles)) {
      styleRules.push({
        selector: `node[color="${status}"]`,
        style: { 'background-color': color },
      });
    }

    cyRef.current = cytoscape({
      container: containerRef.current,
      elements,
      style: styleRules,
      layout: { name: 'dagre', nodeSep: 60, rankSep: 80 } as any,
    });

    cyRef.current.on('tap', 'node', (evt) => onNodeClick?.(evt.target.data('id')));
    return () => { cyRef.current?.destroy(); };
  }, [nodes, edges, onNodeClick]);

  if (nodes.length === 0) return <div className="empty-state">No nodes to display</div>;
  return <div ref={containerRef} className="dag-container" style={{ height: '100%', minHeight: '400px' }} />;
}
