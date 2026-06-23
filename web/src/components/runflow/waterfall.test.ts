import { describe, expect, it } from "vitest";
import {
  nodeColor,
  runWindow,
  timeByCategory,
  timeByStep,
  usageByType,
  waterfall,
} from "./overview";
import type { Category } from "@/lib/categories";
import type { NodeView } from "@/types/run";

function node(
  id: string,
  startedAt: string | null,
  endedAt: string | null,
  status: NodeView["status"] = "success",
  category: string | null = null,
): NodeView {
  return {
    id,
    depends_on: [],
    status,
    provider: null,
    model: null,
    iterations: 1,
    usage: { input: null, output: null, cache_read: null, cache_write: null },
    note: null,
    output: "",
    started_at: startedAt,
    ended_at: endedAt,
    category,
    artifact: null,
    artifact_content: null,
    activity: null,
  };
}

function cat(id: string, color: string, ordinal: number): Category {
  return {
    id,
    label: id[0].toUpperCase() + id.slice(1),
    color,
    ordinal,
    created_at: "",
    updated_at: "",
  };
}

const T0 = "2026-06-05T12:00:00.000Z";
const T10 = "2026-06-05T12:00:10.000Z";
const T30 = "2026-06-05T12:00:30.000Z";
const T60 = "2026-06-05T12:01:00.000Z";
const NOW = Date.parse(T60);

describe("runWindow", () => {
  it("spans earliest start to latest end", () => {
    const win = runWindow([node("a", T0, T30), node("b", T10, T60)], NOW)!;
    expect(win.startMs).toBe(Date.parse(T0));
    expect(win.endMs).toBe(Date.parse(T60));
    expect(win.spanMs).toBe(60_000);
  });

  it("uses now for a still-running node's end", () => {
    const win = runWindow([node("a", T30, null, "running")], NOW)!;
    expect(win.endMs).toBe(NOW);
  });

  it("returns null when nothing has started", () => {
    expect(runWindow([node("a", null, null, "pending")], NOW)).toBeNull();
  });
});

describe("waterfall", () => {
  it("positions bars by offset and width within the window, sorted by start", () => {
    const rows = waterfall([node("b", T30, T60), node("a", T0, T30)], NOW);
    expect(rows.map((r) => r.id)).toEqual(["a", "b"]);
    expect(rows[0].offset).toBeCloseTo(0);
    expect(rows[0].width).toBeCloseTo(0.5);
    expect(rows[1].offset).toBeCloseTo(0.5);
    expect(rows[1].width).toBeCloseTo(0.5);
  });

  it("omits nodes that never started", () => {
    const rows = waterfall(
      [node("a", T0, T30), node("b", null, null, "skipped")],
      NOW,
    );
    expect(rows.map((r) => r.id)).toEqual(["a"]);
  });
});

describe("timeByStep", () => {
  it("sorts by duration descending and drops un-started nodes", () => {
    const rows = timeByStep(
      [node("short", T0, T10), node("long", T0, T60), node("none", null, null)],
      NOW,
    );
    expect(rows.map((r) => r.id)).toEqual(["long", "short"]);
    expect(rows[0].durationMs).toBe(60_000);
  });
});

describe("timeByCategory", () => {
  const cats = [cat("planning", "#aaa", 0), cat("impl", "#bbb", 1)];

  it("sums durations per category, ordinal order, skips uncategorized", () => {
    const segs = timeByCategory(
      [
        node("a", T0, T30, "success", "impl"),
        node("b", T0, T10, "success", "planning"),
        node("c", T0, T60, "success", null), // uncategorized → excluded
      ],
      NOW,
      cats,
    );
    expect(segs.map((s) => s.id)).toEqual(["planning", "impl"]); // ordinal order
    expect(segs.find((s) => s.id === "impl")!.ms).toBe(30_000);
    expect(segs.find((s) => s.id === "planning")!.ms).toBe(10_000);
  });

  it("is empty when no node is categorized", () => {
    expect(timeByCategory([node("a", T0, T30)], NOW, cats)).toEqual([]);
  });
});

describe("nodeColor", () => {
  const colors = new Map([["planning", "#aaa"]]);
  it("uses category color when known, else status color", () => {
    expect(nodeColor("success", "planning", colors)).toBe("#aaa");
    // Unknown category or none → status color (a CSS var, not the category color).
    expect(nodeColor("success", "nope", colors)).toContain("var(");
    expect(nodeColor("failed", null, colors)).toContain("var(");
  });
});

describe("usageByType", () => {
  it("returns ordered non-zero segments", () => {
    const segs = usageByType({
      input: 100,
      output: 20,
      cache_read: 0,
      cache_write: null,
    });
    expect(segs.map((s) => s.key)).toEqual(["input", "output"]);
    expect(segs[0].value).toBe(100);
  });
});
