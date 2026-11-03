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
import type { DagJson, DagJsonNode, DagJsonEdge } from '../api/types';

import {
  inferModality,
  METRO_COMPACT_ZOOM,
  metroZoomBand,
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
  /** Raw aggregation key of a grouped station — the display label carries a
   *  ` (n)` count suffix that no rule id resolves to. */
  groupKey?: string;
  onEdit?: (name: string) => void;
}

function MetroStationNode({ data, selected }: NodeProps<Node<MetroStationData>>) {
  const d = data as MetroStationData;
  const isCompact = d.zoomLevel < METRO_COMPACT_ZOOM;
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
        onDoubleClick={() => d.onEdit?.(d.groupKey ?? d.label)}
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
      onDoubleClick={() => d.onEdit?.(d.groupKey ?? d.label)}
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
        {d.stationCount && (
          <span className="rf-metro-station-count" title={`${d.stationCount} rules in this group`}>
            ×{d.stationCount}
          </span>
        )}
        {d.isInterchange && (
          <span className="rf-metro-interchange" title="Interchange — multiple inbound/outbound routes">⊕</span>
        )}
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
  onEditRule?: (name: string) => void;
  onConnectRules?: (from: string, to: string) => void;
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
  // The compact/detailed station switch is the only zoom-sensitive visual:
  // quantize the continuous onMove zoom into that band, or every wheel tick
  // re-enters the layout memo below — an O(V·E) Sugiyama run plus a node
  // position reset — mid-gesture.
  const zoomBand = metroZoomBand(currentZoom);

  // Compute layout using Sugiyama with horizontal transit line corridors
  const { nodes: initialNodes, edges: initialEdges } = useMemo(() => {
    if (!dag || dag.nodes.length === 0) return { nodes: [], edges: [] };

    // Granularity grouping: when the user selects "process" or "module"
    // we aggregate nodes into super-stations by environment or modality.
    // This is the multi-scale zoom promised by Pillar III of the roadmap.
    const groupKey = (n: DagJsonNode): string => {
      if (granularity === 'process') return n.environment || 'system';
      if (granularity === 'module') return inferModality(n.label);
      return n.id; // 'rule' — no grouping
    };

    // Build grouped DAG when granularity > rule
    let effectiveNodes: DagJsonNode[];
    let effectiveEdges: DagJsonEdge[];
    let nodeGroups: Map<string, string[]> = new Map(); // groupKey -> member ids

    if (granularity === 'rule') {
      effectiveNodes = dag.nodes;
      effectiveEdges = dag.edges;
    } else {
      // Aggregate nodes by groupKey
      const groups = new Map<string, DagJsonNode>();
      nodeGroups = new Map();
      for (const n of dag.nodes) {
        const key = groupKey(n);
        if (!groups.has(key)) {
          groups.set(key, {
            ...n,
            id: key,
            label: `${key} (${dag.nodes.filter((x) => groupKey(x) === key).length})`,
            rule: {},
          });
          nodeGroups.set(key, []);
        }
        nodeGroups.get(key)!.push(n.id);
      }
      effectiveNodes = Array.from(groups.values());

      // Build edges between groups (deduplicated)
      const edgeSet = new Set<string>();
      effectiveEdges = [];
      for (const e of dag.edges) {
        const fromGroup = groupKey(dag.nodes.find((n) => n.id === e.from)!);
        const toGroup = groupKey(dag.nodes.find((n) => n.id === e.to)!);
        if (fromGroup !== toGroup) {
          const edgeKey = `${fromGroup}->${toGroup}`;
          if (!edgeSet.has(edgeKey)) {
            edgeSet.add(edgeKey);
            effectiveEdges.push({ from: fromGroup, to: toGroup, kind: e.kind });
          }
        }
      }
    }

    const inDegrees: Record<string, number> = {};
    const outDegrees: Record<string, number> = {};
    effectiveNodes.forEach((n) => {
      inDegrees[n.id] = 0;
      outDegrees[n.id] = 0;
    });
    effectiveEdges.forEach((e) => {
      inDegrees[e.to] = (inDegrees[e.to] || 0) + 1;
      outDegrees[e.from] = (outDegrees[e.from] || 0) + 1;
    });

    const parents = new Map<string, string[]>();
    for (const n of effectiveNodes) parents.set(n.id, []);
    for (const e of effectiveEdges) parents.get(e.to)?.push(e.from);

    const items = effectiveNodes.map((n) => ({
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
      effectiveNodes.forEach((n, i) => {
        positions[n.id] = { x: 0, y: i * 140 };
      });
    }

    const nodes: Node<MetroStationData>[] = effectiveNodes.map((n) => {
      const rule = (n.rule ?? {}) as Record<string, unknown>;
      const modality = inferModality(n.label);
      const isInterchange = (inDegrees[n.id] || 0) > 1 || (outDegrees[n.id] || 0) > 1;
      const members = nodeGroups.get(n.id);
      const stationCount = members ? members.length : undefined;

      return {
        id: n.id,
        type: 'metroStation',
        position: positions[n.id] || { x: 0, y: 0 },
        selected: selectedRule === n.label || (members?.includes(selectedRule ?? '') ?? false),
        data: {
          label: n.label,
          modality,
          environment: n.environment || granularity,
          shell: String(rule.shell ?? ''),
          status: members
            ? (() => {
                const memberStatuses = members.map((m) => statuses?.[m]).filter(Boolean);
                if (memberStatuses.some((s) => s === 'failed')) return 'failed';
                if (memberStatuses.some((s) => s === 'running')) return 'running';
                if (memberStatuses.length > 0 && memberStatuses.every((s) => s === 'completed')) return 'completed';
                return undefined;
              })()
            : statuses?.[n.label],
          transform: Boolean(rule.transform),
          isInterchange,
          stationCount,
          // The zoom band is applied by the shallow remap below, keeping this
          // memo independent of viewport motion.
          zoomLevel: 0,
          groupKey: members ? n.id : undefined,
          onEdit: onEditRule,
        },
      };
    });

    const edges: Edge[] = effectiveEdges.map((e, idx) => {
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
  }, [dag, selectedRule, statuses, onEditRule, granularity]);

  // Apply the zoom band as a data-only shallow remap: crossing a band
  // re-renders the station glyphs without recomputing (or resetting) layout.
  const zoomedNodes = useMemo(
    () => initialNodes.map((n) => ({ ...n, data: { ...n.data, zoomLevel: zoomBand } })),
    [initialNodes, zoomBand],
  );

  const [nodes, setNodes, onNodesChange] = useNodesState(zoomedNodes);
  const [edges, setEdges, onEdgesChange] = useEdgesState(initialEdges);

  useEffect(() => {
    setNodes(zoomedNodes);
    setEdges(initialEdges);
  }, [zoomedNodes, initialEdges, setNodes, setEdges]);

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
        onNodeClick={(_evt, node) => onSelectRule?.(node.data.groupKey ?? node.data.label)}
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
