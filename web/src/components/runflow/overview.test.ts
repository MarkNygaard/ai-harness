import { describe, expect, it } from "vitest";
import { usageByModel, tokensByStep } from "./overview";
import type { NodeView } from "@/types/run";

function node(
  id: string,
  model: string | null,
  input: number,
  output: number,
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
});
