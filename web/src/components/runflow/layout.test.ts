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
    routes: [],
    when: null,
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
    liveProgress: null,
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

  it("leaves a route node from the exit each step's when: names", () => {
    const kind = {
      ...view("kind", []),
      routes: ["epic", "piece", "skip", "task"],
      output: JSON.stringify({ route: "task", reason: "", by: "rule" }),
    };
    const route = (r: string) => `$kind.output.route == '${r}'`;
    const { nodes, edges } = layoutRun([
      kind,
      { ...view("epic-start", ["kind"]), when: route("epic") },
      {
        ...view("ack", ["kind"]),
        when: `${route("task")} || ${route("piece")}`,
      },
      { ...view("after", ["kind"]) },
    ]);
    const handles = edges
      .map((e) => `${e.sourceHandle ?? "-"}->${e.target}`)
      .sort();
    expect(handles).toEqual([
      "-->after",
      "epic->epic-start",
      "piece->ack",
      "task->ack",
    ]);
    expect(nodes.find((n) => n.id === "kind")?.data.taken).toBe("task");
    // The exit the route didn't take fades behind the one it did.
    const opacity = (id: string) =>
      Number(edges.find((e) => e.id === id)?.style?.opacity);
    expect(opacity("kind.epic->epic-start")).toBeLessThan(
      opacity("kind.task->ack"),
    );
  });
});
