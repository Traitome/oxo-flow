import { useEffect, useRef } from 'react';
import cytoscape, { type Core, type ElementDefinition } from 'cytoscape';

interface DagViewProps {
  nodes: { name: string; inputs: string[]; outputs: string[]; environment: string }[];
  edges: { from: string; to: string }[];
  onNodeClick?: (name: string) => void;
}

export default function DagView({ nodes, edges, onNodeClick }: DagViewProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const cyRef = useRef<Core | null>(null);

  useEffect(() => {
    if (!containerRef.current) return;

    const elements: ElementDefinition[] = [
      ...nodes.map((n) => ({
        data: {
          id: n.name,
          label: n.name,
          env: n.environment,
          inputCount: n.inputs.length,
          outputCount: n.outputs.length,
        },
      })),
      ...edges.map((e) => ({
        data: { id: `${e.from}-${e.to}`, source: e.from, target: e.to },
      })),
    ];

    cyRef.current = cytoscape({
      container: containerRef.current,
      elements,
      style: [
        {
          selector: 'node',
          style: {
            'background-color': '#1a1a2e',
            'border-color': '#4a6fa5',
            'border-width': 2,
            label: 'data(label)',
            color: '#e0e0e0',
            'font-size': '12px',
            'text-valign': 'center',
            'text-halign': 'center',
            width: 120,
            height: 40,
            shape: 'round-rectangle',
          },
        },
        {
          selector: 'edge',
          style: {
            width: 2,
            'line-color': '#4a5568',
            'target-arrow-color': '#4a5568',
            'target-arrow-shape': 'triangle',
            'curve-style': 'bezier',
          },
        },
        {
          selector: 'node:selected',
          style: { 'border-color': '#63b3ed', 'border-width': 3 },
        },
      ],
      layout: { name: 'dagre', nodeSep: 60, rankSep: 80 } as any,
      wheelSensitivity: 0.3,
    });

    cyRef.current.on('tap', 'node', (evt) => {
      const name = evt.target.data('id');
      onNodeClick?.(name);
    });

    return () => {
      cyRef.current?.destroy();
    };
  }, [nodes, edges, onNodeClick]);

  return <div ref={containerRef} className="dag-container" />;
}
