import { describe, expect, it } from "vitest";
import { Position, type Edge, type Node } from "@xyflow/react";
import { fromGraph, layout, makeNode, toGraph, type EditorNodeData } from "./graph";
import type { EditorWorkflow } from "@/types/authoring";

describe("toGraph", () => {
  it("creates a node per step and an edge per dependency, all positioned", () => {
    const wf: EditorWorkflow = {
      name: "d",
      nodes: [
        { id: "a", bash: "echo a" },
        { id: "b", depends_on: ["a"], prompt: "x" },
        { id: "c", depends_on: ["a", "b"], prompt: "y" },
      ],
    };
    const { nodes, edges } = toGraph(wf);
    expect(nodes).toHaveLength(3);
    expect(edges.map((e) => e.id).sort()).toEqual(["a->b", "a->c", "b->c"]);
    for (const n of nodes) {
      expect(Number.isFinite(n.position.x)).toBe(true);
      expect(Number.isFinite(n.position.y)).toBe(true);
      expect(n.sourcePosition).toBe(Position.Bottom);
      expect(n.targetPosition).toBe(Position.Top);
    }
  });
  it("drops edges referencing missing nodes", () => {
    const { edges } = toGraph({ name: "d", nodes: [{ id: "b", depends_on: ["ghost"], prompt: "x" }] });
    expect(edges).toHaveLength(0);
  });
});
describe("layout", () => {
  it("assigns finite positions to every node", () => {
    const nodes: Node<EditorNodeData>[] = [
      { id: "a", type: "editor", position: { x: 0, y: 0 }, data: { node: { id: "a", bash: "x" } } },
      { id: "b", type: "editor", position: { x: 0, y: 0 }, data: { node: { id: "b", prompt: "y" } } },
    ];
    const edges: Edge[] = [{ id: "a->b", source: "a", target: "b" }];
    const laidOut = layout(nodes, edges);
    expect(laidOut).toHaveLength(2);
    for (const n of laidOut) {
      expect(Number.isFinite(n.position.x)).toBe(true);
      expect(Number.isFinite(n.position.y)).toBe(true);
    }
  });
  it("ignores edges whose source or target is missing from the node list", () => {
    const nodes: Node<EditorNodeData>[] = [
      { id: "a", type: "editor", position: { x: 0, y: 0 }, data: { node: { id: "a", bash: "x" } } },
    ];
    const edges: Edge[] = [{ id: "a->ghost", source: "a", target: "ghost" }];
    const laidOut = layout(nodes, edges);
    expect(laidOut).toHaveLength(1);
    expect(Number.isFinite(laidOut[0].position.x)).toBe(true);
  });
});
describe("makeNode", () => {
  it("creates a node with the given position and top-bottom handle alignment", () => {
    const n = makeNode({ id: "n", bash: "echo hi" }, 120, 240);
    expect(n.id).toBe("n");
    expect(n.type).toBe("editor");
    expect(n.position).toEqual({ x: 120, y: 240 });
    expect(n.sourcePosition).toBe(Position.Bottom);
    expect(n.targetPosition).toBe(Position.Top);
    expect(n.data.node).toEqual({ id: "n", bash: "echo hi" });
  });
});

describe("fromGraph", () => {
  it("rebuilds depends_on from the canvas edges", () => {
    const nodes: Node<EditorNodeData>[] = [
      { id: "a", type: "editor", position: { x: 0, y: 0 }, data: { node: { id: "a", bash: "x" } } },
      { id: "b", type: "editor", position: { x: 0, y: 0 }, data: { node: { id: "b", prompt: "y" } } },
    ];
    const edges: Edge[] = [{ id: "a->b", source: "a", target: "b" }];
    const wf = fromGraph(nodes, edges, { name: "d" });
    expect(wf.name).toBe("d");
    expect(wf.nodes.find((n) => n.id === "b")?.depends_on).toEqual(["a"]);
    expect(wf.nodes.find((n) => n.id === "a")?.depends_on).toEqual([]);
  });

  it("round-trips toGraph -> fromGraph preserving edges", () => {
    const wf: EditorWorkflow = {
      name: "d",
      nodes: [
        { id: "a", bash: "echo a" },
        { id: "b", depends_on: ["a"], prompt: "x" },
      ],
    };
    const { nodes, edges } = toGraph(wf);
    const back = fromGraph(nodes, edges, { name: "d" });
    expect(back.nodes.find((n) => n.id === "b")?.depends_on).toEqual(["a"]);
  });
});
