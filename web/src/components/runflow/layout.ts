import Dagre from "@dagrejs/dagre";
import { type Edge, type Node, MarkerType, Position } from "@xyflow/react";
import type { NodeView } from "@/types/run";
import { exitsOfWhen } from "@/components/editor/graph";
import { statusColor } from "./format";

export const NODE_WIDTH = 224;
export const NODE_HEIGHT = 72;
/** Height of one route exit row under a route node's card. */
export const EXIT_ROW_HEIGHT = 20;

export interface RunNodeData extends Record<string, unknown> {
  view: NodeView;
  /** The exit a finished route node took, if its output says. */
  taken: string | null;
}

/** A node's rendered height: route nodes grow one row per exit. */
export function nodeHeight(v: NodeView): number {
  return v.routes.length
    ? NODE_HEIGHT + v.routes.length * EXIT_ROW_HEIGHT
    : NODE_HEIGHT;
}

/** The route a route node's output (`{"route": "<name>", …}`) names. */
export function takenRoute(v: NodeView): string | null {
  if (!v.routes.length || !v.output) return null;
  try {
    const route = (JSON.parse(v.output) as { route?: unknown }).route;
    return typeof route === "string" && v.routes.includes(route) ? route : null;
  } catch {
    return null;
  }
}

/**
 * Compute a left-to-right Dagre layout for a run's nodes. Edges come from each
 * node's `depends_on`; a step behind a route node gets one edge per exit its
 * `when:` names, leaving from that exit's handle, as in the editor. An edge is
 * "active" when its downstream node is running (drives the animated
 * highlight). Pure — deterministic for a given input.
 */
export function layoutRun(views: NodeView[]): {
  nodes: Node<RunNodeData>[];
  edges: Edge[];
} {
  const g = new Dagre.graphlib.Graph().setDefaultEdgeLabel(() => ({}));
  g.setGraph({
    rankdir: "LR",
    nodesep: 28,
    ranksep: 64,
    marginx: 24,
    marginy: 24,
  });

  const byId = new Map(views.map((v) => [v.id, v]));
  const taken = new Map(views.map((v) => [v.id, takenRoute(v)]));
  for (const v of views)
    g.setNode(v.id, { width: NODE_WIDTH, height: nodeHeight(v) });

  const edges: Edge[] = [];
  for (const v of views) {
    const exits = exitsOfWhen(v.when ?? undefined) ?? [];
    for (const dep of v.depends_on) {
      const source = byId.get(dep);
      if (!source) continue; // tolerate dangling refs
      g.setEdge(dep, v.id);
      const routes = exits
        .filter(([s, r]) => s === dep && source.routes.includes(r))
        .map(([, r]) => r);
      const handles: Array<string | undefined> = routes.length
        ? routes
        : [undefined];
      for (const handle of handles) {
        const active = v.status === "running";
        // Once the route has decided, the exits it didn't take fade further.
        const decided = taken.get(dep);
        const passed =
          handle === undefined || decided == null || decided === handle;
        edges.push({
          id:
            handle === undefined
              ? `${dep}->${v.id}`
              : `${dep}.${handle}->${v.id}`,
          source: dep,
          target: v.id,
          sourceHandle: handle,
          animated: active,
          style: {
            stroke: active ? statusColor("running") : "var(--muted-foreground)",
            strokeWidth: active ? 2 : 1.5,
            opacity: active ? 0.9 : passed ? 0.35 : 0.15,
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
  }

  Dagre.layout(g);

  const nodes: Node<RunNodeData>[] = views.map((v) => {
    const pos = g.node(v.id);
    return {
      id: v.id,
      type: "run",
      position: {
        x: (pos?.x ?? 0) - NODE_WIDTH / 2,
        y: (pos?.y ?? 0) - nodeHeight(v) / 2,
      },
      data: { view: v, taken: taken.get(v.id) ?? null },
      sourcePosition: Position.Right,
      targetPosition: Position.Left,
      draggable: false,
    };
  });

  return { nodes, edges };
}
