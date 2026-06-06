import { describe, expect, it } from "vitest";
import { Position, type Edge, type Node } from "@xyflow/react";
import { fromGraph, toGraph, layout, makeNode, type EditorNodeData } from "./graph";
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
    }
  });

  it("drops edges referencing missing nodes", () => {
    const { edges } = toGraph({ name: "d", nodes: [{ id: "b", depends_on: ["ghost"], prompt: "x" }] });
    expect(edges).toHaveLength(0);
  });

  it("sets source handle to Bottom and target handle to Top on every node", () => {
    const wf: EditorWorkflow = {
      name: "d",
      nodes: [{ id: "a", bash: "echo a" }],
    };
    const { nodes } = toGraph(wf);
    expect(nodes[0].sourcePosition).toBe(Position.Bottom);
    expect(nodes[0].targetPosition).toBe(Position.Top);
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
describe("makeNode", () => {
  it("sets source handle to Bottom and target handle to Top", () => {
    const node = { id: "n", bash: "echo" };
    const n = makeNode(node, 10, 20);
    expect(n.sourcePosition).toBe(Position.Bottom);
    expect(n.targetPosition).toBe(Position.Top);
  });
});

describe("layout", () => {
  it("places upstream nodes above downstream nodes in TB layout", () => {
    const raw: Node<EditorNodeData>[] = [
      { id: "a", type: "editor", position: { x: 0, y: 0 }, data: { node: { id: "a", bash: "x" } }, sourcePosition: Position.Bottom, targetPosition: Position.Top },
      { id: "b", type: "editor", position: { x: 0, y: 0 }, data: { node: { id: "b", bash: "y" } }, sourcePosition: Position.Bottom, targetPosition: Position.Top },
    ];
    const edges: Edge[] = [{ id: "a->b", source: "a", target: "b" }];
    const laidOut = layout(raw, edges);
    const a = laidOut.find((n) => n.id === "a")!;
    const b = laidOut.find((n) => n.id === "b")!;
    expect(a.position.y).toBeLessThan(b.position.y);
  });

  it("preserves sourcePosition and targetPosition on laid-out nodes", () => {
    const raw: Node<EditorNodeData>[] = [
      { id: "a", type: "editor", position: { x: 0, y: 0 }, data: { node: { id: "a", bash: "x" } }, sourcePosition: Position.Bottom, targetPosition: Position.Top },
    ];
    const laidOut = layout(raw, []);
    expect(laidOut[0].sourcePosition).toBe(Position.Bottom);
    expect(laidOut[0].targetPosition).toBe(Position.Top);
  });
});
