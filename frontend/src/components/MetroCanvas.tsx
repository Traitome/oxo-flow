import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  MiniMap,
  Position,
  ReactFlow,
  useEdgesState,
  useNodesState,
  type Connection,
  type Edge,
  type Node,
  type NodeProps,
  type NodeTypes,
} from '@xyflow/react';
import { graphStratify, sugiyama } from 'd3-dag';
import '@xyflow/react/dist/style.css';
import { GitBranch, Layers, TrainTrack } from 'lucide-react';
import type { DagJson } from '../api/types';

import {
  inferModality,
  MODALITY_COLORS,
  type BioModality,
  type MetroGranularity,
} from '../utils/metro';

interface MetroStationData extends Record<string, unknown> {
  label: string;
  modality: BioModality;
  environment: string;
  shell: string;
  status?: string;
  transform?: boolean;
  isInterchange?: boolean;
  stationCount?: number;
  zoomLevel: number;
  onEdit?: (name: string) => void;
}

function MetroStationNode({ data, selected }: NodeProps<Node<MetroStationData>>) {
  const d = data as MetroStationData;
  const isCompact = d.zoomLevel < 0.55;
  const colors = MODALITY_COLORS[d.modality];

  const haloStyle = useMemo(() => {
    if (d.status === 'failed') return { boxShadow: 'var(--halo-error)' };
    if (d.status === 'running') return { boxShadow: 'var(--halo-running)' };
    if (d.status === 'completed') return { boxShadow: 'var(--halo-success)' };
    return undefined;
  }, [d.status]);

  if (isCompact) {
    // High-level transit corridor station glyph
    return (
      <div
        className={`rf-metro-station-compact ${selected ? 'selected' : ''}`}
        style={{
          borderColor: colors.primary,
          backgroundColor: 'var(--color-bg-secondary, #0F172A)',
          ...haloStyle,
        }}
        title={d.label}
        onDoubleClick={() => d.onEdit?.(d.label)}
      >
        <Handle type="target" position={Position.Left} className="rf-handle" style={{ background: colors.primary }} />
        <div className="rf-metro-compact-glyph" style={{ background: colors.primary }} />
        <div className="rf-metro-compact-label" style={{ color: 'var(--color-text)' }}>
          {d.label}
        </div>
        <Handle type="source" position={Position.Right} className="rf-handle" style={{ background: colors.primary }} />
      </div>
    );
  }

  // Detailed station stop card
  return (
    <div
      className={`rf-metro-station-card ${selected ? 'selected' : ''}`}
      style={{
        borderLeft: `4px solid ${colors.primary}`,
        ...haloStyle,
      }}
      onDoubleClick={() => d.onEdit?.(d.label)}
    >
      <Handle type="target" position={Position.Left} className="rf-handle" style={{ background: colors.primary }} />
      <div className="rf-metro-header">
        <span className="rf-metro-dot" style={{ backgroundColor: colors.primary }} />
        <span className="rf-metro-title">{d.label}</span>
        {d.transform && <span className="rf-transform-badge" title="Split-Map-Combine">⇄</span>}
      </div>
      <div className="rf-metro-meta">
        <span className="rf-metro-modality-badge" style={{ backgroundColor: colors.glow, color: colors.primary }}>
          {d.modality}
        </span>
        <span className="rf-metro-env">{d.environment}</span>
        {d.status && <span className={`rf-rule-status status-${d.status}`}>{d.status}</span>}
      </div>
      {d.shell && <div className="rf-metro-shell">{d.shell}</div>}
      <Handle type="source" position={Position.Right} className="rf-handle" style={{ background: colors.primary }} />
    </div>
  );
}

const nodeTypes = {
  metroStation: MetroStationNode,
} satisfies NodeTypes;

export interface MetroCanvasProps {
  dag: DagJson | null;
  editable?: boolean;
  scopeKey?: string;
  onEditRule?: (name: string) => void;
  onConnectRules?: (from: string, to: string) => void;
  onRemoveRules?: (names: string[]) => void;
  statuses?: Record<string, string>;
  selectedRule?: string | null;
  onSelectRule?: (name: string | null) => void;
}

export function MetroCanvas({
  dag,
  editable = false,
  onEditRule,
  onConnectRules,
  statuses,
  selectedRule,
  onSelectRule,
}: MetroCanvasProps) {
  const [granularity, setGranularity] = useState<MetroGranularity>('rule');
  const [currentZoom, setCurrentZoom] = useState(1);

  // Compute layout using Sugiyama with horizontal transit line corridors
  const { nodes: initialNodes, edges: initialEdges } = useMemo(() => {
    if (!dag || dag.nodes.length === 0) return { nodes: [], edges: [] };

    const inDegrees: Record<string, number> = {};
    const outDegrees: Record<string, number> = {};
    dag.nodes.forEach((n) => {
      inDegrees[n.id] = 0;
      outDegrees[n.id] = 0;
    });
    dag.edges.forEach((e) => {
      inDegrees[e.to] = (inDegrees[e.to] || 0) + 1;
      outDegrees[e.from] = (outDegrees[e.from] || 0) + 1;
    });

    const parents = new Map<string, string[]>();
    for (const n of dag.nodes) parents.set(n.id, []);
    for (const e of dag.edges) parents.get(e.to)?.push(e.from);

    const items = dag.nodes.map((n) => ({
      id: n.id,
      parentIds: parents.get(n.id) ?? [],
    }));

    const positions: Record<string, { x: number; y: number }> = {};
    try {
      const layout = sugiyama().nodeSize([120, 300]);
      const stratified = graphStratify()(items);
      layout(stratified);
      for (const n of stratified.nodes()) {
        positions[n.data.id] = { x: n.y ?? 0, y: n.x ?? 0 };
      }
    } catch {
      dag.nodes.forEach((n, i) => {
        positions[n.id] = { x: 0, y: i * 140 };
      });
    }

    const nodes: Node<MetroStationData>[] = dag.nodes.map((n) => {
      const rule = (n.rule ?? {}) as Record<string, unknown>;
      const modality = inferModality(n.label);
      const isInterchange = (inDegrees[n.id] || 0) > 1 || (outDegrees[n.id] || 0) > 1;

      return {
        id: n.id,
        type: 'metroStation',
        position: positions[n.id] || { x: 0, y: 0 },
        selected: selectedRule === n.label,
        data: {
          label: n.label,
          modality,
          environment: n.environment || 'system',
          shell: String(rule.shell ?? ''),
          status: statuses?.[n.label],
          transform: Boolean(rule.transform),
          isInterchange,
          zoomLevel: currentZoom,
          onEdit: onEditRule,
        },
      };
    });

    const edges: Edge[] = dag.edges.map((e, idx) => {
      const fromModality = inferModality(e.from);
      const colors = MODALITY_COLORS[fromModality];
      return {
        id: `e-${e.from}-${e.to}-${idx}`,
        source: e.from,
        target: e.to,
        animated: statuses?.[e.from] === 'running',
        style: {
          stroke: colors.primary,
          strokeWidth: 3,
        },
      };
    });

    return { nodes, edges };
  }, [dag, selectedRule, statuses, currentZoom, onEditRule]);

  const [nodes, setNodes, onNodesChange] = useNodesState(initialNodes);
  const [edges, setEdges, onEdgesChange] = useEdgesState(initialEdges);

  useEffect(() => {
    setNodes(initialNodes);
    setEdges(initialEdges);
  }, [initialNodes, initialEdges, setNodes, setEdges]);

  const onConnect = useCallback(
    (connection: Connection) => {
      if (!editable || !connection.source || !connection.target) return;
      onConnectRules?.(connection.source, connection.target);
    },
    [editable, onConnectRules]
  );

  return (
    <div className="rf-metro-canvas-container" style={{ width: '100%', height: '100%', position: 'relative' }}>
      <div className="rf-metro-toolbar">
        <div className="rf-metro-granularity-group">
          <button
            type="button"
            className={`rf-metro-btn ${granularity === 'rule' ? 'active' : ''}`}
            onClick={() => setGranularity('rule')}
            title="Rule granularity"
          >
            <TrainTrack size={14} /> Rule
          </button>
          <button
            type="button"
            className={`rf-metro-btn ${granularity === 'process' ? 'active' : ''}`}
            onClick={() => setGranularity('process')}
            title="Process granularity"
          >
            <Layers size={14} /> Process
          </button>
          <button
            type="button"
            className={`rf-metro-btn ${granularity === 'module' ? 'active' : ''}`}
            onClick={() => setGranularity('module')}
            title="Module granularity"
          >
            <GitBranch size={14} /> Module
          </button>
        </div>
      </div>

      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onConnect={onConnect}
        onNodeClick={(_evt, node) => onSelectRule?.(node.data.label)}
        onPaneClick={() => onSelectRule?.(null)}
        onMove={(_evt, viewport) => setCurrentZoom(viewport.zoom)}
        fitView
        minZoom={0.2}
        maxZoom={2.0}
      >
        <Background variant={BackgroundVariant.Dots} gap={24} size={1} color="var(--color-border, #263449)" />
        <Controls position="bottom-right" showInteractive={false} />
        <MiniMap
          position="bottom-left"
          nodeColor={(n) => {
            const d = n.data as MetroStationData;
            return MODALITY_COLORS[d.modality]?.primary || '#64748B';
          }}
          maskColor="rgba(0, 0, 0, 0.4)"
        />
      </ReactFlow>
    </div>
  );
}
