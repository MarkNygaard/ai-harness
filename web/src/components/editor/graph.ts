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

/** A route exit a step is reached through: the route node, and the route. */
export type Exit = [source: string, route: string];

const ROUTE_CLAUSE = /^\$([\w-]+)\.output\.route == '([^']*)'$/;

/**
 * The exits a `when:` names, if it is nothing but route clauses joined by
 * `||` — what the editor writes when a step is reached through one exit or
 * several (`task` or `piece`). Anything else is a hand-written condition, and
 * `undefined` says the editor doesn't own it.
 */
export function exitsOfWhen(when: string | undefined): Exit[] | undefined {
  if (!when?.trim()) return undefined;
  const exits: Exit[] = [];
  for (const clause of when.split("||")) {
    const m = ROUTE_CLAUSE.exec(clause.trim());
    if (!m) return undefined;
    exits.push([m[1], m[2]]);
  }
  return exits;
}

/** The `when:` for a step reached through `exits`; none for no exits. */
export function whenOfExits(exits: Exit[]): string | undefined {
  return exits.length
    ? exits.map(([s, r]) => routeWhen(s, r)).join(" || ")
    : undefined;
}

/** The routes of `source` that `when` reaches a step through. */
export function routesOfWhen(
  source: EditorNode,
  when: string | undefined,
): string[] {
  if (!source.route) return [];
  const names = new Set(source.route.routes.map((r) => r.name));
  return (exitsOfWhen(when) ?? [])
    .filter(([s, r]) => s === source.id && names.has(r))
    .map(([, r]) => r);
}

/** An edge's id: one per source, target and exit. */
export function edgeId(
  source: string,
  target: string,
  sourceHandle?: string | null,
): string {
  return sourceHandle == null
    ? `${source}->${target}`
    : `${source}.${sourceHandle}->${target}`;
}

export function edgeOf(
  source: string,
  target: string,
  sourceHandle?: string,
): Edge {
  return {
    id: edgeId(source, target, sourceHandle),
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
      if (!source) continue;
      // One edge per exit: a step reached through `task` or `piece` leaves
      // the route twice, not once from whichever exit comes first.
      const routes = routesOfWhen(source, n.when);
      if (routes.length)
        for (const r of routes) edges.push(edgeOf(dep, n.id, r));
      else edges.push(edgeOf(dep, n.id));
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
  const touched = new Set<string>();
  const kept: Edge[] = [];
  for (const e of edges) {
    if (e.source !== prev.id || e.sourceHandle == null) {
      kept.push(e);
      continue;
    }
    touched.add(e.target);
    const to = renamed(e.sourceHandle);
    if (to !== undefined)
      kept.push({
        ...e,
        id: edgeId(e.source, e.target, to),
        sourceHandle: to,
      });
  }
  return {
    edges: kept,
    nodes: nodes.map((n) => {
      // Only a `when:` made of exits is the editor's to change.
      const exits = touched.has(n.id)
        ? exitsOfWhen(n.data.node.when)
        : undefined;
      if (!exits) return n;
      const now = exits.flatMap(([s, r]): Exit[] => {
        if (s !== prev.id) return [[s, r]];
        const to = renamed(r);
        return to === undefined ? [] : [[next.id, to]];
      });
      return {
        ...n,
        data: { node: { ...n.data.node, when: whenOfExits(now) } },
      };
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
