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

function edgeOf(source: string, target: string): Edge {
  return {
    id: `${source}->${target}`,
    source,
    target,
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
  const ids = new Set(wf.nodes.map((n) => n.id));
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
      if (ids.has(dep)) edges.push(edgeOf(dep, n.id));
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
    "name" | "description" | "provider" | "model" | "ui"
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
