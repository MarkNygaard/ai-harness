import { describe, expect, it } from "vitest";
import { layoutRun } from "./layout";
import type { NodeView } from "@/types/run";

function view(
  id: string,
  depends_on: string[],
  status: NodeView["status"] = "success",
): NodeView {
  return {
    id,
    depends_on,
    status,
    provider: null,
    model: null,
    iterations: 1,
    usage: { input: null, output: null, cache_read: null, cache_write: null },
    note: null,
    output: "",
    started_at: null,
    ended_at: null,
    category: null,
    artifact: null,
    artifact_content: null,
    activity: null,
    activityLog: [],
  };
}

describe("layoutRun", () => {
  it("builds one edge per dependency and positions every node", () => {
    const { nodes, edges } = layoutRun([
      view("a", []),
      view("b", ["a"]),
      view("c", ["a", "b"]),
    ]);
    expect(nodes).toHaveLength(3);
    expect(edges.map((e) => e.id).sort()).toEqual(["a->b", "a->c", "b->c"]);
    // Dagre assigns finite coordinates.
    for (const n of nodes) {
      expect(Number.isFinite(n.position.x)).toBe(true);
      expect(Number.isFinite(n.position.y)).toBe(true);
    }
  });

  it("drops edges to nodes that aren't present", () => {
    const { edges } = layoutRun([view("b", ["ghost"])]);
    expect(edges).toHaveLength(0);
  });

  it("animates edges into a running node", () => {
    const { edges } = layoutRun([view("a", []), view("b", ["a"], "running")]);
    expect(edges[0].animated).toBe(true);
  });
});
