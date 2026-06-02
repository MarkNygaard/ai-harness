import Dagre from "@dagrejs/dagre";
import { type Edge, type Node, MarkerType, Position } from "@xyflow/react";
import type { NodeView } from "@/types/run";
import { statusColor } from "./format";

export const NODE_WIDTH = 196;
export const NODE_HEIGHT = 72;

export interface RunNodeData extends Record<string, unknown> {
  view: NodeView;
}

/**
 * Compute a left-to-right Dagre layout for a run's nodes. Edges come from each
 * node's `depends_on`; an edge is "active" when its downstream node is running
 * (drives the animated highlight). Pure — deterministic for a given input.
 */
export function layoutRun(views: NodeView[]): {
  nodes: Node<RunNodeData>[];
  edges: Edge[];
} {
  const g = new Dagre.graphlib.Graph().setDefaultEdgeLabel(() => ({}));
  g.setGraph({ rankdir: "LR", nodesep: 28, ranksep: 64, marginx: 24, marginy: 24 });

  const ids = new Set(views.map((v) => v.id));
  for (const v of views) g.setNode(v.id, { width: NODE_WIDTH, height: NODE_HEIGHT });

  const edges: Edge[] = [];
  for (const v of views) {
    for (const dep of v.depends_on) {
      if (!ids.has(dep)) continue; // tolerate dangling refs
      g.setEdge(dep, v.id);
      const active = v.status === "running";
      edges.push({
        id: `${dep}->${v.id}`,
        source: dep,
        target: v.id,
        animated: active,
        style: {
          stroke: active ? statusColor("running") : "var(--muted-foreground)",
          strokeWidth: active ? 2 : 1.5,
          opacity: active ? 0.9 : 0.35,
        },
        markerEnd: {
          type: MarkerType.ArrowClosed,
          width: 14,
          height: 14,
          color: active ? statusColor("running") : "var(--muted-foreground)",
        },
      });
    }
  }

  Dagre.layout(g);

  const nodes: Node<RunNodeData>[] = views.map((v) => {
    const pos = g.node(v.id);
    return {
      id: v.id,
      type: "run",
      position: { x: (pos?.x ?? 0) - NODE_WIDTH / 2, y: (pos?.y ?? 0) - NODE_HEIGHT / 2 },
      data: { view: v },
      sourcePosition: Position.Right,
      targetPosition: Position.Left,
      draggable: false,
    };
  });

  return { nodes, edges };
}
