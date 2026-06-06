import { describe, expect, it } from "vitest";
import { Position, type Edge, type Node } from "@xyflow/react";
import { fromGraph, makeNode, toGraph, type EditorNodeData } from "./graph";
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

  it("sets sourcePosition to Bottom and targetPosition to Top", () => {
    const { nodes } = toGraph({
      name: "d",
      nodes: [{ id: "a", bash: "echo a" }],
    });
    expect(nodes[0].sourcePosition).toBe(Position.Bottom);
    expect(nodes[0].targetPosition).toBe(Position.Top);
  });

  it("drops edges referencing missing nodes", () => {
    const { edges } = toGraph({ name: "d", nodes: [{ id: "b", depends_on: ["ghost"], prompt: "x" }] });
    expect(edges).toHaveLength(0);
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
  it("creates a node with Bottom source and Top target handles", () => {
    const n = makeNode({ id: "a", bash: "echo a" }, 10, 20);
    expect(n.position).toEqual({ x: 10, y: 20 });
    expect(n.sourcePosition).toBe(Position.Bottom);
    expect(n.targetPosition).toBe(Position.Top);
  });
});
