import Dagre from "@dagrejs/dagre";
import { type Edge, type Node, MarkerType, Position } from "@xyflow/react";
import type { EditorNode, EditorWorkflow } from "@/types/authoring";
import { nodeKind } from "@/lib/workflow-yaml";

export const NODE_WIDTH = 200;
export const NODE_HEIGHT = 56;

export interface EditorNodeData extends Record<string, unknown> {
  node: EditorNode;
}

/** Lay out editor nodes top-to-bottom with Dagre (used on load / "tidy"). */
export function layout(
  nodes: Node<EditorNodeData>[],
  edges: Edge[],
): Node<EditorNodeData>[] {
  const g = new Dagre.graphlib.Graph().setDefaultEdgeLabel(() => ({}));
  g.setGraph({
    rankdir: "TB",
    nodesep: 24,
    ranksep: 72,
    marginx: 24,
    marginy: 24,
  });
  for (const n of nodes)
    g.setNode(n.id, { width: NODE_WIDTH, height: NODE_HEIGHT });
  for (const e of edges) {
    if (g.hasNode(e.source) && g.hasNode(e.target))
      g.setEdge(e.source, e.target);
  }
  Dagre.layout(g);
  return nodes.map((n) => {
    const p = g.node(n.id);
    return {
      ...n,
      position: {
        x: (p?.x ?? 0) - NODE_WIDTH / 2,
        y: (p?.y ?? 0) - NODE_HEIGHT / 2,
      },
    };
  });
}

/**
 * The `when:` a step leaving a route node through `route` carries. Connecting
 * from a route's exit writes it; it is also how a saved workflow's edges find
 * their exit again on load.
 */
export function routeWhen(source: string, route: string): string {
  return `$${source}.output.route == '${route}'`;
}

/** The route of `source` that `when` selects, if it is exactly a route's `when`. */
export function routeOfWhen(
  source: EditorNode,
  when: string | undefined,
): string | undefined {
  if (!source.route || !when) return undefined;
  return source.route.routes.find((r) => routeWhen(source.id, r.name) === when)
    ?.name;
}

export function edgeOf(
  source: string,
  target: string,
  sourceHandle?: string,
): Edge {
  return {
    id: `${source}->${target}`,
    source,
    target,
    sourceHandle,
    style: {
      stroke: "var(--muted-foreground)",
      strokeWidth: 1.5,
      opacity: 0.5,
    },
    markerEnd: {
      type: MarkerType.ArrowClosed,
      width: 14,
      height: 14,
      color: "var(--muted-foreground)",
    },
  };
}

/** Build xyflow nodes + edges (edges = `depends_on`) from a workflow, laid out. */
export function toGraph(wf: EditorWorkflow): {
  nodes: Node<EditorNodeData>[];
  edges: Edge[];
} {
  const byId = new Map(wf.nodes.map((n) => [n.id, n]));
  const nodes: Node<EditorNodeData>[] = wf.nodes.map((n) => ({
    id: n.id,
    type: "editor",
    position: { x: 0, y: 0 },
    data: { node: n },
    sourcePosition: Position.Bottom,
    targetPosition: Position.Top,
  }));
  const edges: Edge[] = [];
  for (const n of wf.nodes) {
    for (const dep of n.depends_on ?? []) {
      const source = byId.get(dep);
      if (source) edges.push(edgeOf(dep, n.id, routeOfWhen(source, n.when)));
    }
  }
  return { nodes: layout(nodes, edges), edges };
}

/** Rebuild a workflow from the canvas: edges become each node's `depends_on`. */
export function fromGraph(
  nodes: Node<EditorNodeData>[],
  edges: Edge[],
  meta: Pick<
    EditorWorkflow,
    | "name"
    | "description"
    | "provider"
    | "model"
    | "ui"
    | "improvements"
    | "handles_epics"
  >,
): EditorWorkflow {
  const depsByTarget = new Map<string, string[]>();
  for (const e of edges) {
    const list = depsByTarget.get(e.target) ?? [];
    if (!list.includes(e.source)) list.push(e.source);
    depsByTarget.set(e.target, list);
  }
  return {
    ...meta,
    nodes: nodes.map((n) => ({
      ...n.data.node,
      depends_on: depsByTarget.get(n.id) ?? [],
    })),
  };
}

/**
 * Carry a route node's exits through an edit of it: a route renamed (same
 * position, new name) or a node renamed keeps its edges on the right exit and
 * rewrites the `when:` each step behind it carries; a route removed takes its
 * edges with it and clears their `when:`. Pure; a no-op for other nodes.
 */
export function syncRouteExits(
  prev: EditorNode,
  next: EditorNode,
  nodes: Node<EditorNodeData>[],
  edges: Edge[],
): { nodes: Node<EditorNodeData>[]; edges: Edge[] } {
  if (!prev.route) return { nodes, edges };
  const oldNames = prev.route.routes.map((r) => r.name);
  const newNames = next.route?.routes.map((r) => r.name) ?? [];
  // The drawer changes one thing at a time: the same count is a rename (by
  // position, so a half-typed or blank name keeps its edges); a different
  // count is an add or a remove, where names say which survived.
  const renamed = (old: string): string | undefined =>
    oldNames.length === newNames.length
      ? newNames[oldNames.indexOf(old)]
      : newNames.includes(old)
        ? old
        : undefined;
  // target -> [the `when:` its exit wrote, the one it should now carry]
  const whens = new Map<string, [string, string | undefined]>();
  const kept: Edge[] = [];
  for (const e of edges) {
    if (e.source !== prev.id || e.sourceHandle == null) {
      kept.push(e);
      continue;
    }
    const to = renamed(e.sourceHandle);
    whens.set(e.target, [
      routeWhen(prev.id, e.sourceHandle),
      to ? routeWhen(next.id, to) : undefined,
    ]);
    if (to !== undefined) kept.push({ ...e, sourceHandle: to });
  }
  return {
    edges: kept,
    nodes: nodes.map((n) => {
      const w = whens.get(n.id);
      // Only a `when:` the exit wrote is the editor's to change.
      if (!w || n.data.node.when !== w[0]) return n;
      return { ...n, data: { node: { ...n.data.node, when: w[1] } } };
    }),
  };
}

/** A new xyflow node for the canvas at a position. */
export function makeNode(
  node: EditorNode,
  x: number,
  y: number,
): Node<EditorNodeData> {
  return {
    id: node.id,
    type: "editor",
    position: { x, y },
    data: { node },
    sourcePosition: Position.Bottom,
    targetPosition: Position.Top,
  };
}

export { nodeKind };
