import { useCallback, useEffect, useRef } from 'react';
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
} from '@xyflow/react';
import { graphStratify, sugiyama } from 'd3-dag';
import '@xyflow/react/dist/style.css';
import { LayoutGrid } from 'lucide-react';
import type { DagJson } from '../api/types';

// ── Node card: a terminal snippet, because the subject's vernacular is code ──

type RuleNodeData = Record<string, unknown> & {
  label: string;
  environment: string;
  shell: string;
  description: string;
  status?: string;
  /** called on double-click (editable canvas only) */
  onEdit?: (name: string) => void;
};

const ENV_LABELS: Record<string, string> = {
  system: 'system',
  conda: 'conda',
  mamba: 'mamba',
  docker: 'docker',
  singularity: 'singularity',
  venv: 'venv',
  modules: 'modules',
};

function RuleNodeCard({ data, selected }: NodeProps) {
  const d = data as RuleNodeData;
  return (
    <div
      className={`rf-rule-node ${d.status ? `node-status-${d.status}` : ''} ${selected ? 'selected' : ''}`}
      onDoubleClick={(e) => {
        e.stopPropagation();
        d.onEdit?.(d.label);
      }}
      title={d.description || d.label}
    >
      <Handle type="target" position={Position.Left} className="rf-handle" />
      <div className="rf-rule-name">{d.label}</div>
      <div className="rf-rule-meta">
        <span className={`rf-env-dot rf-env-${d.environment}`} aria-hidden />
        <span className="rf-env-label">{ENV_LABELS[d.environment] ?? d.environment}</span>
        {d.status && <span className="rf-rule-status">{d.status}</span>}
      </div>
      {d.shell && <div className="rf-rule-shell">{d.shell}</div>}
      <Handle type="source" position={Position.Right} className="rf-handle" />
    </div>
  );
}

const nodeTypes = { rule: RuleNodeCard };

// ── d3-dag auto-layout (layered Sugiyama; left-to-right) ──

function computeLayout(dagNodes: DagJson['nodes']): Record<string, { x: number; y: number }> {
  const positions: Record<string, { x: number; y: number }> = {};
  if (dagNodes.length === 0) return positions;
  try {
    const items = dagNodes.map((n) => ({ id: n.id, parentIds: [] as string[] }));
    const layout = sugiyama().nodeSize([110, 260]);
    const dag = graphStratify()(items);
    layout(dag);
    for (const n of dag.nodes()) {
      positions[n.data.id] = { x: n.y ?? 0, y: n.x ?? 0 };
    }
  } catch {
    // Fallback: column placement for graphs the layout cannot stratify.
    dagNodes.forEach((n, i) => {
      positions[n.id] = { x: 0, y: i * 130 };
    });
  }
  return positions;
}

// ── Position persistence (presentation-only; layout is not workflow data) ──

const positionsStore = {
  load(scopeKey: string): Record<string, { x: number; y: number }> {
    try {
      const raw = localStorage.getItem(`oxo-canvas-pos-${scopeKey}`);
      return raw ? (JSON.parse(raw) as Record<string, { x: number; y: number }>) : {};
    } catch {
      return {};
    }
  },
  save(scopeKey: string, positions: Record<string, { x: number; y: number }>) {
    try {
      localStorage.setItem(`oxo-canvas-pos-${scopeKey}`, JSON.stringify(positions));
    } catch {
      // Storage unavailable — layout persistence is cosmetic, not data.
    }
  },
};

// ── Canvas ──

export interface WorkflowCanvasProps {
  /** DAG document from /api/pipelines/dag (nodes carry the full rule). */
  dag: DagJson | null;
  /** When true, nodes can be dragged, connected, deleted, and edited. */
  editable: boolean;
  /** Scope for persisted node positions (e.g. pipeline id or "monitor-<runId>"). */
  scopeKey: string;
  /** Called when the user asks to edit a rule (double-click). */
  onEditRule?: (name: string) => void;
  /** Called when the user connects two nodes (editable canvas). */
  onConnectRules?: (from: string, to: string) => void;
  /** Called when the user deletes the selected node(s). */
  onRemoveRules?: (names: string[]) => void;
  /** Per-node status strings for the read-only monitor view. */
  statusById?: Record<string, string>;
  /** 'editor' shows the auto-layout button; 'monitor' hides it. */
  context?: 'editor' | 'monitor';
}

export default function WorkflowCanvas({
  dag,
  editable,
  scopeKey,
  onEditRule,
  onConnectRules,
  onRemoveRules,
  statusById,
  context = 'editor',
}: WorkflowCanvasProps) {
  const [nodes, setNodes, onNodesChange] = useNodesState<Node<RuleNodeData>>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
  const lastNodeSet = useRef('');

  // Rebuild the graph whenever the DAG document changes. Saved positions are
  // reused only while the node set is unchanged; a changed set gets a fresh
  // auto-layout so new rules land somewhere sensible.
  useEffect(() => {
    if (!dag) {
      setNodes([]);
      setEdges([]);
      lastNodeSet.current = '';
      return;
    }
    const nodeSet = dag.nodes.map((n) => n.id).sort().join('|');
    const sameSet = nodeSet === lastNodeSet.current;
    lastNodeSet.current = nodeSet;
    const saved = sameSet ? positionsStore.load(scopeKey) : {};
    const layout =
      sameSet && Object.keys(saved).length > 0 ? saved : computeLayout(dag.nodes);

    setNodes(
      dag.nodes.map((n) => {
        const pos = layout[n.id] ?? { x: 0, y: 0 };
        return {
          id: n.id,
          type: 'rule',
          position: pos,
          draggable: editable,
          selectable: true,
          data: {
            label: n.label,
            environment: n.environment,
            shell: typeof n.rule.shell === 'string' ? n.rule.shell : '',
            description: typeof n.rule.description === 'string' ? n.rule.description : '',
            status: statusById?.[n.id],
            onEdit: editable ? onEditRule : undefined,
          },
        };
      }),
    );
    setEdges(
      dag.edges.map((e) => ({
        id: `${e.from}->${e.to}`,
        source: e.from,
        target: e.to,
        style:
          e.kind === 'file'
            ? { strokeDasharray: '5 4', stroke: '#94a3b8' }
            : { stroke: '#475569' },
      })),
    );
  }, [dag, editable, scopeKey, statusById, onEditRule, setNodes, setEdges]);

  // Persist positions after drags settle.
  const handleNodesChange = useCallback(
    (changes: Parameters<typeof onNodesChange>[0]) => {
      onNodesChange(changes);
      if (changes.some((c) => c.type === 'position' && c.dragging === false)) {
        const saved = positionsStore.load(scopeKey);
        for (const n of nodes) {
          saved[n.id] = n.position;
        }
        positionsStore.save(scopeKey, saved);
      }
    },
    [onNodesChange, scopeKey, nodes],
  );

  const handleConnect = useCallback(
    (connection: Connection) => {
      if (!editable || !connection.source || !connection.target) return;
      onConnectRules?.(connection.source, connection.target);
    },
    [editable, onConnectRules],
  );

  const handleDeleteKey = useCallback(
    (event: KeyboardEvent) => {
      if (!editable || (event.key !== 'Delete' && event.key !== 'Backspace')) return;
      const target = event.target as HTMLElement | null;
      if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)) {
        return;
      }
      const selected = nodes.filter((n) => n.selected).map((n) => n.id);
      if (selected.length > 0) onRemoveRules?.(selected);
    },
    [editable, nodes, onRemoveRules],
  );

  useEffect(() => {
    window.addEventListener('keydown', handleDeleteKey);
    return () => window.removeEventListener('keydown', handleDeleteKey);
  }, [handleDeleteKey]);

  const applyLayout = useCallback(() => {
    if (!dag) return;
    const layout = computeLayout(dag.nodes);
    setNodes((current) => current.map((n) => ({ ...n, position: layout[n.id] ?? n.position })));
    positionsStore.save(scopeKey, layout);
  }, [dag, scopeKey, setNodes]);

  if (!dag || dag.nodes.length === 0) {
    return <div className="empty-state">Enter valid TOML to see the DAG</div>;
  }

  return (
    <div className="workflow-canvas" data-editable={editable}>
      <ReactFlow
        nodes={nodes}
        edges={edges}
        onNodesChange={handleNodesChange}
        onEdgesChange={onEdgesChange}
        onConnect={handleConnect}
        nodeTypes={nodeTypes}
        fitView
        fitViewOptions={{ padding: 0.25, maxZoom: 1.2 }}
        // Issue #79 (P2): minZoom 0.2 rendered node text at ~4px on large
        // DAGs — unreadable. 0.5 keeps the minimum glyph height legible
        // while still zooming out far enough for 30+ node graphs.
        minZoom={0.5}
        maxZoom={2}
        proOptions={{ hideAttribution: true }}
        nodesConnectable={editable}
        deleteKeyCode={null}
      >
        <Background variant={BackgroundVariant.Dots} gap={22} size={1.2} color="#cbd5e1" />
        <Controls showInteractive={false} />
        {/* The minimap overlays the canvas — useful for monitoring a large
            DAG, intrusive while editing. */}
        {/* Large-DAG navigation aid in every mode (issue #79 P2). */}
        <MiniMap pannable zoomable className="rf-minimap" />
      </ReactFlow>
      {context === 'editor' && (
        <button className="rf-layout-btn" onClick={applyLayout} title="Auto-layout the DAG">
          <LayoutGrid size={14} /> Auto layout
        </button>
      )}
    </div>
  );
}
