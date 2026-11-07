import { describe, expect, it } from "vitest";
import { Position, type Edge, type Node } from "@xyflow/react";
import {
  exitsOfWhen,
  fromGraph,
  whenOfExits,
  toGraph,
  layout,
  makeNode,
  routeWhen,
  syncRouteExits,
  type EditorNodeData,
} from "./graph";
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
    const { edges } = toGraph({
      name: "d",
      nodes: [{ id: "b", depends_on: ["ghost"], prompt: "x" }],
    });
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
      {
        id: "a",
        type: "editor",
        position: { x: 0, y: 0 },
        data: { node: { id: "a", bash: "x" } },
      },
      {
        id: "b",
        type: "editor",
        position: { x: 0, y: 0 },
        data: { node: { id: "b", prompt: "y" } },
      },
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
describe("route exits", () => {
  const routed: EditorWorkflow = {
    name: "r",
    nodes: [
      {
        id: "kind",
        route: {
          prompt: "epic or task?",
          routes: [{ name: "epic" }, { name: "task" }],
          default: "task",
        },
      },
      {
        id: "supervise",
        depends_on: ["kind"],
        when: "$kind.output.route == 'epic'",
        prompt: "x",
      },
      {
        id: "build",
        depends_on: ["kind"],
        when: "$kind.output.route == 'task'",
        prompt: "y",
      },
      // Depends on the route without going through an exit: always runs.
      { id: "log", depends_on: ["kind"], bash: "echo" },
    ],
  };

  it("loads each edge onto the exit its step's when: names", () => {
    const { edges } = toGraph(routed);
    const handle = (t: string) =>
      edges.find((e) => e.target === t)?.sourceHandle;
    expect(handle("supervise")).toBe("epic");
    expect(handle("build")).toBe("task");
    expect(handle("log")).toBeUndefined();
  });

  // `task`'s explore runs for an ordinary issue and for a piece of an epic.
  const either: EditorWorkflow = {
    ...routed,
    nodes: [
      {
        ...routed.nodes[0],
        route: {
          ...routed.nodes[0].route!,
          routes: [{ name: "epic" }, { name: "piece" }, { name: "task" }],
        },
      },
      {
        id: "explore",
        depends_on: ["kind"],
        when: "$kind.output.route == 'task' || $kind.output.route == 'piece'",
        prompt: "x",
      },
    ],
  };

  it("draws a step reached through two exits from both", () => {
    const { edges } = toGraph(either);
    expect(
      edges
        .filter((e) => e.target === "explore")
        .map((e) => e.sourceHandle)
        .sort(),
    ).toEqual(["piece", "task"]);
    expect(new Set(edges.map((e) => e.id)).size).toBe(edges.length);
  });

  it("reads only a when: made of route clauses as exits", () => {
    expect(
      exitsOfWhen("$kind.output.route == 'task' || $b.output.route == 'pass'"),
    ).toEqual([
      ["kind", "task"],
      ["b", "pass"],
    ]);
    expect(exitsOfWhen("$v.output.passed == 'true'")).toBeUndefined();
    expect(
      exitsOfWhen("$kind.output.route == 'task' && $x.output != ''"),
    ).toBeUndefined();
    expect(whenOfExits([])).toBeUndefined();
  });

  it("renames one exit of a step reached through two", () => {
    const { nodes, edges } = toGraph(either);
    const prev = either.nodes[0];
    const next = {
      ...prev,
      route: {
        ...prev.route!,
        routes: [{ name: "epic" }, { name: "part" }, { name: "task" }],
      },
    };
    const out = syncRouteExits(prev, next, nodes, edges);
    const explore = out.nodes.find((n) => n.id === "explore")!.data.node;
    expect(explore.when).toBe(
      "$kind.output.route == 'task' || $kind.output.route == 'part'",
    );
  });

  it("writes the when: a downstream step compares against", () => {
    expect(routeWhen("kind", "epic")).toBe("$kind.output.route == 'epic'");
  });

  it("keeps edges on a renamed route and rewrites the steps behind it", () => {
    const { nodes, edges } = toGraph(routed);
    const prev = routed.nodes[0];
    const next = {
      ...prev,
      route: { ...prev.route!, routes: [{ name: "epic" }, { name: "single" }] },
    };
    const out = syncRouteExits(prev, next, nodes, edges);
    expect(out.edges.find((e) => e.target === "build")?.sourceHandle).toBe(
      "single",
    );
    const build = out.nodes.find((n) => n.id === "build")!.data.node;
    expect(build.when).toBe("$kind.output.route == 'single'");
    const sup = out.nodes.find((n) => n.id === "supervise")!.data.node;
    expect(sup.when).toBe("$kind.output.route == 'epic'");
  });

  it("drops the edges of a removed route and clears their when:", () => {
    const { nodes, edges } = toGraph(routed);
    const prev = routed.nodes[0];
    // Fewer routes than before: a removal, not a rename by position.
    const next = {
      ...prev,
      route: { ...prev.route!, routes: [{ name: "task" }] },
    };
    const out = syncRouteExits(prev, next, nodes, edges);
    expect(out.edges.some((e) => e.target === "supervise")).toBe(false);
    expect(out.edges.some((e) => e.target === "log")).toBe(true);
    const sup = out.nodes.find((n) => n.id === "supervise")!.data.node;
    expect(sup.when).toBeUndefined();
  });

  it("follows a renamed route node", () => {
    const { nodes, edges } = toGraph(routed);
    const prev = routed.nodes[0];
    const out = syncRouteExits(prev, { ...prev, id: "classify" }, nodes, edges);
    const sup = out.nodes.find((n) => n.id === "supervise")!.data.node;
    expect(sup.when).toBe("$classify.output.route == 'epic'");
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
      {
        id: "a",
        type: "editor",
        position: { x: 0, y: 0 },
        data: { node: { id: "a", bash: "x" } },
        sourcePosition: Position.Bottom,
        targetPosition: Position.Top,
      },
      {
        id: "b",
        type: "editor",
        position: { x: 0, y: 0 },
        data: { node: { id: "b", bash: "y" } },
        sourcePosition: Position.Bottom,
        targetPosition: Position.Top,
      },
    ];
    const edges: Edge[] = [{ id: "a->b", source: "a", target: "b" }];
    const laidOut = layout(raw, edges);
    const a = laidOut.find((n) => n.id === "a")!;
    const b = laidOut.find((n) => n.id === "b")!;
    expect(a.position.y).toBeLessThan(b.position.y);
  });

  it("preserves sourcePosition and targetPosition on laid-out nodes", () => {
    const raw: Node<EditorNodeData>[] = [
      {
        id: "a",
        type: "editor",
        position: { x: 0, y: 0 },
        data: { node: { id: "a", bash: "x" } },
        sourcePosition: Position.Bottom,
        targetPosition: Position.Top,
      },
    ];
    const laidOut = layout(raw, []);
    expect(laidOut[0].sourcePosition).toBe(Position.Bottom);
    expect(laidOut[0].targetPosition).toBe(Position.Top);
  });
});
