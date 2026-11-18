import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
} from 'react';
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
import { useI18n } from '../context/I18n';

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
}

/**
 * Interaction contract shared by every station glyph.
 *
 * UX-06 (issue #1148): station actions live in a context rather than in node
 * `data` so a connect-mode change re-renders the glyphs without invalidating
 * the layout memo (an O(V·E) Sugiyama run that also resets node positions).
 *
 * Grouped granularities aggregate rules by an environment/modality key — the
 * station id is NOT a rule name, so every action that would name a rule
 * (edit / connect / remove) is disabled there: a group operation cannot be
 * proven semantics-preserving, and a rule that happens to share the group
 * key would be silently edited instead of the group.
 */
interface MetroInteraction {
  /** Rule-level editing is available (editable editor, Rule granularity). */
  interactive: boolean;
  /** Stations aggregate rule sets rather than naming rules. */
  grouped: boolean;
  /** Rule name the keyboard connect flow started from, if any. */
  connectSource: string | null;
  onActivate: (rule: string) => void;
  onStartConnect: (rule: string) => void;
  onRemove: (rule: string) => void;
  onCancel: () => void;
  /** Announce why a station (grouped or read-only) has no edit action. */
  onReadOnlyNotice: () => void;
}

const MetroInteractionContext = createContext<MetroInteraction | null>(null);

function MetroStationNode({ data, selected }: NodeProps<Node<MetroStationData>>) {
  const d = data as MetroStationData;
  const { t } = useI18n();
  const interaction = useContext(MetroInteractionContext);
  const isCompact = d.zoomLevel < METRO_COMPACT_ZOOM;
  const colors = MODALITY_COLORS[d.modality];

  const grouped = interaction?.grouped ?? false;
  const interactive = interaction?.interactive ?? false;
  const ruleName = d.groupKey ?? d.label;
  const isConnectSource = interaction?.connectSource === ruleName && !grouped;

  const haloStyle = useMemo(() => {
    if (d.status === 'failed') return { boxShadow: 'var(--halo-error)' };
    if (d.status === 'running') return { boxShadow: 'var(--halo-running)' };
    if (d.status === 'completed') return { boxShadow: 'var(--halo-success)' };
    return undefined;
  }, [d.status]);

  // aria-label states the station's role AND why it is or is not actionable;
  // `aria-keyshortcuts` documents the keyboard editing path for screen
  // readers instead of leaving it to the mouse-only double-click.
  const ariaLabel = grouped
    ? t('canvas.groupedStationAria')
        .replace('{{name}}', ruleName)
        .replace('{{count}}', String(d.stationCount ?? 0))
    : interaction?.connectSource
      ? isConnectSource
        ? t('canvas.connectSourceAria').replace('{{name}}', ruleName)
        : t('canvas.connectTargetAria')
            .replace('{{name}}', ruleName)
            .replace('{{source}}', interaction.connectSource)
      : interactive
        ? t('canvas.editRuleAria').replace('{{name}}', ruleName)
        : t('canvas.readOnlyStationAria').replace('{{name}}', ruleName);

  const handleKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (!interaction) return;
    // Grouped / read-only stations stay focusable (so keyboard users can
    // inspect the aggregation) but only announce why they cannot be edited.
    if (!interactive) {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        interaction.onReadOnlyNotice();
      } else if (e.key === 'Delete' || e.key === 'Backspace' || e.key === 'c' || e.key === 'C') {
        // Inert, but swallowed so Backspace cannot navigate the browser back.
        e.preventDefault();
      }
      return;
    }
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      e.stopPropagation();
      interaction.onActivate(ruleName);
    } else if (e.key === 'c' || e.key === 'C') {
      e.preventDefault();
      e.stopPropagation();
      interaction.onStartConnect(ruleName);
    } else if (e.key === 'Delete' || e.key === 'Backspace') {
      e.preventDefault();
      e.stopPropagation();
      interaction.onRemove(ruleName);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      interaction.onCancel();
    }
  };

  const a11yProps = {
    tabIndex: 0,
    role: interactive ? 'button' : 'group',
    'aria-label': ariaLabel,
    'aria-keyshortcuts': interactive ? 'Enter Space C Delete Backspace Escape' : undefined,
    'aria-describedby': interactive ? 'rf-metro-station-help' : undefined,
    'data-station': ruleName,
    'data-grouped': grouped ? 'true' : undefined,
    'data-interactive': interactive ? 'true' : undefined,
    'data-connect-source': isConnectSource ? 'true' : undefined,
    onKeyDown: handleKeyDown,
    onDoubleClick: interactive
      ? (e: ReactMouseEvent<HTMLDivElement>) => {
          e.stopPropagation();
          interaction?.onActivate(ruleName);
        }
      : undefined,
  } as const;

  if (isCompact) {
    // High-level transit corridor station glyph
    return (
      <div
        className={`rf-metro-station-compact ${selected ? 'selected' : ''} ${isConnectSource ? 'connect-source' : ''}`}
        style={{
          borderColor: colors.primary,
          backgroundColor: 'var(--color-bg-secondary, #0F172A)',
          ...haloStyle,
        }}
        title={d.label}
        {...a11yProps}
      >
        {!grouped && (
          <Handle type="target" position={Position.Left} className="rf-handle" style={{ background: colors.primary }} />
        )}
        <div className="rf-metro-compact-glyph" style={{ background: colors.primary }} />
        <div className="rf-metro-compact-label" style={{ color: 'var(--color-text)' }}>
          {d.label}
        </div>
        {!grouped && (
          <Handle type="source" position={Position.Right} className="rf-handle" style={{ background: colors.primary }} />
        )}
      </div>
    );
  }

  // Detailed station stop card
  return (
    <div
      className={`rf-metro-station-card ${selected ? 'selected' : ''} ${isConnectSource ? 'connect-source' : ''}`}
      style={{
        borderLeft: `4px solid ${colors.primary}`,
        ...haloStyle,
      }}
      {...a11yProps}
    >
      {!grouped && (
        <Handle type="target" position={Position.Left} className="rf-handle" style={{ background: colors.primary }} />
      )}
      <div className="rf-metro-header">
        <span className="rf-metro-dot" style={{ backgroundColor: colors.primary }} />
        <span className="rf-metro-title">{d.label}</span>
        {d.transform && <span className="rf-transform-badge" title="Split-Map-Combine">⇄</span>}
        {grouped && <span className="rf-metro-readonly-badge">{t('canvas.groupedReadOnlyBadge')}</span>}
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
      {!grouped && (
        <Handle type="source" position={Position.Right} className="rf-handle" style={{ background: colors.primary }} />
      )}
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
  /** Keyboard Delete/Backspace on a focused station (Rule granularity only). */
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
  onRemoveRules,
  statuses,
  selectedRule,
  onSelectRule,
}: MetroCanvasProps) {
  const { t } = useI18n();
  const [granularity, setGranularity] = useState<MetroGranularity>('rule');
  const [currentZoom, setCurrentZoom] = useState(1);
  // The compact/detailed station switch is the only zoom-sensitive visual:
  // quantize the continuous onMove zoom into that band, or every wheel tick
  // re-enters the layout memo below — an O(V·E) Sugiyama run plus a node
  // position reset — mid-gesture.
  const zoomBand = metroZoomBand(currentZoom);

  // ── UX-06 station interaction (issue #1148) ────────────────────────────
  // Rule granularity maps every station to exactly one rule, so edit /
  // connect / remove name a real rule and are semantics-preserving. Grouped
  // granularities map a station to an aggregation key (environment or
  // modality) whose members are an arbitrary rule set — no action there can
  // be proven to preserve execution semantics (and a rule literally named
  // after the key would be silently targeted instead of the group), so the
  // grouped views are read-only and say why.
  const grouped = granularity !== 'rule';
  const canInteract = editable && !grouped;
  const canConnect = canInteract && Boolean(onConnectRules);
  const canRemove = canInteract && Boolean(onRemoveRules);
  // The granularity is captured with the connect source: a mode switch
  // invalidates the pending connection without a setState-in-effect. The
  // source must still name a rule in the CURRENT DAG (undo/redo can remove
  // it), and a read-only canvas never exposes one.
  const [connectState, setConnectState] = useState<{ source: string; granularity: MetroGranularity } | null>(null);
  const connectSource =
    canInteract &&
    connectState &&
    connectState.granularity === granularity &&
    (dag?.nodes.some((n) => n.id === connectState.source) ?? false)
      ? connectState.source
      : null;
  const setConnectSource = useCallback(
    (source: string | null) => setConnectState(source ? { source, granularity } : null),
    [granularity],
  );
  const [announcement, setAnnouncement] = useState('');

  const announce = useCallback((message: string) => setAnnouncement(message), []);

  const activateStation = useCallback(
    (rule: string) => {
      if (!canInteract) return;
      if (connectSource) {
        if (connectSource === rule) {
          announce(t('canvas.connectSelf'));
          return;
        }
        if (!canConnect) return;
        onConnectRules?.(connectSource, rule);
        announce(t('canvas.connectDone').replace('{{from}}', connectSource).replace('{{to}}', rule));
        setConnectSource(null);
        return;
      }
      onEditRule?.(rule);
    },
    [canInteract, canConnect, connectSource, onConnectRules, onEditRule, setConnectSource, announce, t],
  );

  const startConnect = useCallback(
    (rule: string) => {
      if (!canConnect) return;
      if (connectSource === rule) {
        setConnectSource(null);
        announce(t('canvas.connectCancel'));
        return;
      }
      setConnectSource(rule);
      announce(t('canvas.connectStart').replace('{{name}}', rule));
    },
    [canConnect, connectSource, setConnectSource, announce, t],
  );

  const removeStation = useCallback(
    (rule: string) => {
      if (!canRemove) return;
      if (connectSource === rule) setConnectSource(null);
      onRemoveRules?.([rule]);
      announce(t('canvas.ruleRemoved').replace('{{name}}', rule));
    },
    [canRemove, connectSource, onRemoveRules, setConnectSource, announce, t],
  );

  const cancelInteraction = useCallback(() => {
    if (connectSource) {
      setConnectSource(null);
      announce(t('canvas.connectCancel'));
      return;
    }
    onSelectRule?.(null);
  }, [connectSource, onSelectRule, setConnectSource, announce, t]);

  const readOnlyNotice = useCallback(() => {
    announce(t(grouped ? 'canvas.groupedReadOnly' : 'canvas.readOnlyCanvas'));
  }, [grouped, announce, t]);

  // Escape cancels connect mode from anywhere in the canvas (focus may have
  // moved to the toolbar or pane); station-level Escape stops propagation.
  useEffect(() => {
    if (!connectSource) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      e.preventDefault();
      setConnectSource(null);
      announce(t('canvas.connectCancel'));
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [connectSource, setConnectSource, announce, t]);

  const interaction = useMemo<MetroInteraction>(
    () => ({
      interactive: canInteract,
      grouped,
      connectSource,
      onActivate: activateStation,
      onStartConnect: startConnect,
      onRemove: removeStation,
      onCancel: cancelInteraction,
      onReadOnlyNotice: readOnlyNotice,
    }),
    [canInteract, grouped, connectSource, activateStation, startConnect, removeStation, cancelInteraction, readOnlyNotice],
  );

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
  }, [dag, selectedRule, statuses, granularity]);

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
      // A grouped station's id is an aggregation key, not a rule name — the
      // mouse drag path is disabled there for the same reason as the
      // keyboard one (see the interaction block above).
      if (!editable || grouped || !connection.source || !connection.target) return;
      onConnectRules?.(connection.source, connection.target);
    },
    [editable, grouped, onConnectRules]
  );

  return (
    <div className="rf-metro-canvas-container" style={{ width: '100%', height: '100%', position: 'relative' }}>
      <div className="rf-metro-toolbar">
        <div className="rf-metro-granularity-group">
          <button
            type="button"
            className={`rf-metro-btn ${granularity === 'rule' ? 'active' : ''}`}
            onClick={() => {
              setConnectState(null);
              setGranularity('rule');
            }}
            title={t('canvas.granularityRuleTitle')}
          >
            <TrainTrack size={14} /> {t('canvas.granularityRule')}
          </button>
          <button
            type="button"
            className={`rf-metro-btn ${granularity === 'process' ? 'active' : ''}`}
            onClick={() => {
              setConnectState(null);
              setGranularity('process');
            }}
            title={t('canvas.granularityProcessTitle')}
          >
            <Layers size={14} /> {t('canvas.granularityProcess')}
          </button>
          <button
            type="button"
            className={`rf-metro-btn ${granularity === 'module' ? 'active' : ''}`}
            onClick={() => {
              setConnectState(null);
              setGranularity('module');
            }}
            title={t('canvas.granularityModuleTitle')}
          >
            <GitBranch size={14} /> {t('canvas.granularityModule')}
          </button>
        </div>
        {grouped && (
          <span className="rf-metro-readonly-note" data-testid="metro-grouped-note">
            {t('canvas.groupedReadOnly')}
          </span>
        )}
      </div>

      {/* One visually hidden line of keyboard help, referenced by every
          actionable station's aria-describedby. */}
      <p id="rf-metro-station-help" className="rf-metro-sr-only">
        {t('canvas.stationHelp')}
      </p>
      <div className="rf-metro-sr-only rf-metro-announcer" role="status" aria-live="polite">
        {announcement}
      </div>

      <MetroInteractionContext.Provider value={interaction}>
        <ReactFlow
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={onConnect}
          onNodeClick={(_evt, node) => onSelectRule?.(node.data.groupKey ?? node.data.label)}
          onPaneClick={() => {
            if (connectSource) {
              setConnectSource(null);
              announce(t('canvas.connectCancel'));
            }
            onSelectRule?.(null);
          }}
          onMove={(_evt, viewport) => setCurrentZoom(viewport.zoom)}
          // Stations are the keyboard stops (the glyph owns tabIndex and the
          // edit/connect/remove handlers); React Flow's per-node wrapper would
          // otherwise add a second, inert tab stop per station.
          nodesFocusable={false}
          nodesConnectable={canConnect}
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
      </MetroInteractionContext.Provider>
    </div>
  );
}
