import { useEffect, useRef, useState } from 'react';
import cytoscape, { type Core, type ElementDefinition } from 'cytoscape';
import dagre from 'cytoscape-dagre';
import { ZoomIn, ZoomOut, Maximize2, Download, GitGraph, GripHorizontal } from 'lucide-react';
cytoscape.use(dagre);

interface DagViewProps {
  nodes: Array<{ id: string; label: string; color?: string; tool?: string; threads?: number; memory?: string; duration_ms?: number }>;
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
  const [selectedNode, setSelectedNode] = useState<typeof nodes[0] | null>(null);
  const [layout, setLayout] = useState<'TB' | 'LR'>('TB');
  const [hoveredNode, setHoveredNode] = useState<{ label: string; tool?: string; threads?: number; memory?: string; duration_ms?: number; x: number; y: number } | null>(null);

  const zoomIn = () => cyRef.current?.zoom(cyRef.current.zoom() * 1.3);
  const zoomOut = () => cyRef.current?.zoom(cyRef.current.zoom() * 0.7);
  const fitView = () => cyRef.current?.fit(undefined, 30);
  const toggleLayout = () => setLayout(l => l === 'TB' ? 'LR' : 'TB');
  const exportPng = () => {
    const png = cyRef.current?.png({ full: true, scale: 2 });
    if (png) {
      const a = document.createElement('a'); a.href = png; a.download = 'dag.png'; a.click();
    }
  };

  useEffect(() => {
    if (!containerRef.current || nodes.length === 0) return;

    const elements: ElementDefinition[] = [
      ...nodes.map((n) => ({
        data: { id: n.id, label: n.label, color: n.color || '#CBD5E1', status: n.color, tool: n.tool || '', threads: n.threads || 0, memory: n.memory || '', duration_ms: n.duration_ms || 0 },
      })),
      ...edges.map((e) => ({ data: { id: `${e.source}→${e.target}`, label: '', source: e.source, target: e.target } })),
    ];

    const nodeStyles = STATUS_COLORS;
    const styleRules: any[] = [
      { selector: 'node', style: { label: 'data(label)', color: '#e2e8f0', 'font-size': '12px', 'text-valign': 'center', 'text-halign': 'center', width: 120, height: 40, shape: 'round-rectangle', 'border-width': 2, 'border-color': '#1e293b', 'background-color': '#6B7280', 'transition-property': 'background-color', 'transition-duration': '0.3s' } },
      { selector: 'edge', style: { width: 2, 'line-color': '#475569', 'target-arrow-color': '#475569', 'target-arrow-shape': 'triangle', 'curve-style': 'bezier' } },
      { selector: 'node:selected', style: { 'border-color': '#2563EB', 'border-width': 3 } },
      { selector: 'node[color="blue"]', style: { 'border-color': '#2563EB', 'border-width': 3, 'animation-name': 'pulse', 'animation-duration': '1.5s', 'animation-iteration-count': 'infinite' } },
    ];
    for (const [status, color] of Object.entries(nodeStyles)) {
      styleRules.push({ selector: `node[color="${status}"]`, style: { 'background-color': color } });
    }

    cyRef.current = cytoscape({
      container: containerRef.current,
      elements,
      style: styleRules,
      layout: { name: 'dagre', nodeSep: 60, rankSep: 80, rankDir: layout } as any,
    });

    const cy = cyRef.current;
    cy.on('tap', 'node', (evt) => {
      const id = evt.target.data('id');
      const node = nodes.find(n => n.id === id);
      setSelectedNode(node || null);
      onNodeClick?.(id);
    });
    cy.on('tap', () => setSelectedNode(null));
    cy.on('mouseover', 'node', (evt) => {
      const n = evt.target;
      const bb = containerRef.current?.getBoundingClientRect();
      const pos = n.renderedPosition();
      setHoveredNode({
        label: n.data('label'), tool: n.data('tool'), threads: n.data('threads'),
        memory: n.data('memory'), duration_ms: n.data('duration_ms'),
        x: pos.x + (bb?.left || 0), y: pos.y + (bb?.top || 0),
      });
    });
    cy.on('mouseout', 'node', () => setHoveredNode(null));
    return () => { cy.destroy(); };
  }, [nodes, edges, layout]);

  if (nodes.length === 0) return <div className="empty-state"><div className="empty-state-icon">🔷</div><p>No nodes to display</p></div>;

  return (
    <div style={{ position: 'relative', height: '100%', display: 'flex', flexDirection: 'column' }}>
      {/* Toolbar */}
      <div style={{ display: 'flex', gap: '4px', padding: '4px 0', borderBottom: '1px solid var(--color-border)', marginBottom: '4px' }}>
        <button className="btn-sm" onClick={fitView} title="Fit to view"><Maximize2 size={14} /></button>
        <button className="btn-sm" onClick={zoomIn} title="Zoom in"><ZoomIn size={14} /></button>
        <button className="btn-sm" onClick={zoomOut} title="Zoom out"><ZoomOut size={14} /></button>
        <button className="btn-sm" onClick={toggleLayout} title="Toggle layout (TB/LR)">{layout === 'TB' ? <GitGraph size={14} /> : <GripHorizontal size={14} />}</button>
        <button className="btn-sm" onClick={exportPng} title="Export PNG"><Download size={14} /></button>
        {selectedNode && <span style={{ marginLeft: 'auto', fontSize: '0.8rem', color: 'var(--color-text-secondary)', alignSelf: 'center' }}>Selected: <strong>{selectedNode.label}</strong></span>}
      </div>

      {/* DAG Canvas */}
      <div ref={containerRef} className="dag-container" style={{ flex: 1, minHeight: '300px', background: 'var(--color-bg-tertiary)', borderRadius: 'var(--radius-sm)' }} />

      {/* Hover tooltip */}
      {hoveredNode && (
        <div style={{ position: 'fixed', left: hoveredNode.x + 15, top: hoveredNode.y - 10, background: 'var(--color-bg)', border: '1px solid var(--color-border)', borderRadius: 'var(--radius-sm)', padding: '8px 12px', fontSize: '0.78rem', zIndex: 500, boxShadow: '0 4px 12px rgba(0,0,0,0.1)', pointerEvents: 'none' }}>
          <div style={{ fontWeight: 600, marginBottom: '4px' }}>{hoveredNode.label}</div>
          {hoveredNode.tool && <div style={{ color: 'var(--color-text-secondary)' }}>Tool: {hoveredNode.tool}</div>}
          {(hoveredNode.threads ?? 0) > 0 && <div style={{ color: 'var(--color-text-secondary)' }}>Threads: {hoveredNode.threads}</div>}
          {hoveredNode.memory && <div style={{ color: 'var(--color-text-secondary)' }}>Memory: {hoveredNode.memory}</div>}
          {(hoveredNode.duration_ms ?? 0) > 0 && <div style={{ color: 'var(--color-text-secondary)' }}>Duration: {((hoveredNode.duration_ms ?? 0) / 1000).toFixed(1)}s</div>}
        </div>
      )}

      {/* Selected node slide-out detail panel */}
      {selectedNode && (
        <div style={{ borderTop: '1px solid var(--color-border)', padding: '0.75rem', background: 'var(--color-bg-secondary)', fontSize: '0.85rem' }}>
          <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: '0.5rem' }}>
            <strong>{selectedNode.label}</strong>
            <button className="btn-sm" onClick={() => setSelectedNode(null)}>✕</button>
          </div>
          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '4px' }}>
            <div>Status: <span className={`status-badge ${selectedNode.color || 'pending'}`}>{selectedNode.color || 'pending'}</span></div>
            {selectedNode.tool && <div>Tool: {selectedNode.tool}</div>}
            {selectedNode.threads && <div>Threads: {selectedNode.threads}</div>}
            {selectedNode.memory && <div>Memory: {selectedNode.memory}</div>}
            {selectedNode.duration_ms != null && selectedNode.duration_ms > 0 && <div>Duration: {(selectedNode.duration_ms / 1000).toFixed(1)}s</div>}
          </div>
        </div>
      )}
    </div>
  );
}
