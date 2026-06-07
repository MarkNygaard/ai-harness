import { describe, expect, it } from "vitest";
import { usageByModel, tokensByStep } from "./overview";
import type { NodeView } from "@/types/run";

function node(
  id: string,
  model: string | null,
  input: number | null,
  output: number | null,
): NodeView {
  return {
    id,
    depends_on: [],
    status: "success",
    provider: model ? "claude" : null,
    model,
    iterations: 1,
    usage: { input, output, cache_read: null, cache_write: null },
    note: null,
    output: "",
    started_at: null,
    ended_at: null,
    category: null,
    artifact: null,
    artifact_content: null,
  };
}


describe("usageByModel", () => {
  it("groups by model, sums usage, and sorts by total desc", () => {
    const rows = usageByModel([
      node("a", "sonnet", 100, 20),
      node("b", "sonnet", 50, 10),
      node("c", "opus", 300, 40),
    ]);
    expect(rows.map((r) => r.key)).toEqual(["opus", "sonnet"]);
    const sonnet = rows.find((r) => r.key === "sonnet")!;
    expect(sonnet.steps).toBe(2);
    expect(sonnet.usage.input).toBe(150);
    expect(sonnet.total).toBe(180);
  });

  it("excludes steps with no reported tokens", () => {
    const rows = usageByModel([node("a", "sonnet", 0, 0)]);
    expect(rows).toHaveLength(0);
  });
});
describe("tokensByStep", () => {
  it("sorts by total desc and drops zero-usage steps", () => {
    const rows = tokensByStep([
      node("light", "sonnet", 100, 20),
      node("heavy", "opus", 300, 90),
      node("idle", "sonnet", 0, 0),
    ]);
    expect(rows.map((r) => r.id)).toEqual(["heavy", "light"]);
    expect(rows[0]).toMatchObject({ input: 300, output: 90, total: 390 });
  });

  it("treats null counters as zero", () => {
    const rows = tokensByStep([
      node("a", "sonnet", null, 50),
      node("b", "sonnet", 30, null),
    ]);
    expect(rows).toHaveLength(2);
    expect(rows.find((r) => r.id === "a")).toMatchObject({
      input: 0,
      output: 50,
      total: 50,
    });
    expect(rows.find((r) => r.id === "b")).toMatchObject({
      input: 30,
      output: 0,
      total: 30,
    });
  });

  it("returns empty array when every step has zero tokens", () => {
    expect(tokensByStep([node("a", "sonnet", 0, 0)])).toEqual([]);
    expect(tokensByStep([node("a", "sonnet", null, null)])).toEqual([]);
  });

  it("passes through status and category", () => {
    const rows = tokensByStep([
      {
        ...node("step-1", null, 10, 5),
        status: "failed",
        category: "planning",
      },
    ]);
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({
      id: "step-1",
      status: "failed",
      category: "planning",
      input: 10,
      output: 5,
      total: 15,
    });
  });
});
