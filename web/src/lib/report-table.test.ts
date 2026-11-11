import { describe, expect, it } from "vitest";
import { findingKey, type WorkflowFinding } from "./report";
import {
  categoryTotals,
  columnsOf,
  decisionState,
  defaultSort,
  formatMs,
  visibleRows,
} from "./report-table";

const f = (x: Partial<WorkflowFinding>): WorkflowFinding => ({ ...x });

const audit = [
  f({
    id: "a",
    title: "keeps a",
    category: "merge",
    severity: "low",
    score: 0.4,
    duration_ms: 30,
    location: "web/a.test.ts",
  }),
  f({
    id: "b",
    title: "drops b",
    category: "delete",
    severity: "high",
    score: 0.1,
    duration_ms: 5,
    location: "web/b.test.ts",
    detail: "asserts a constant",
  }),
  f({
    id: "c",
    title: "drops c",
    category: "delete",
    severity: "high",
    score: 0.2,
    location: "api/C",
  }),
  f({
    id: "d",
    title: "no score",
    category: "delete",
    severity: "high",
    duration_ms: 1,
  }),
];

describe("categoryTotals", () => {
  it("puts the most severe category first and adds up its time", () => {
    expect(categoryTotals(audit)).toEqual([
      { category: "delete", count: 3, durationMs: 6 },
      { category: "merge", count: 1, durationMs: 30 },
    ]);
  });

  it("leaves time out of a category whose findings have none", () => {
    expect(categoryTotals([f({ title: "x" })])).toEqual([
      { category: "other", count: 1, durationMs: null },
    ]);
  });
});

describe("visibleRows", () => {
  it("filters by category and sorts lowest score first, blanks last", () => {
    const rows = visibleRows(audit, "delete", "", { key: "score", dir: "asc" });
    expect(rows.map((r) => r.id)).toEqual(["b", "c", "d"]);
    const desc = visibleRows(audit, "delete", "", {
      key: "score",
      dir: "desc",
    });
    expect(desc.map((r) => r.id)).toEqual(["c", "b", "d"]);
  });

  it("matches every word of the search in title, location or detail", () => {
    expect(
      visibleRows(audit, null, "web constant", {
        key: "title",
        dir: "asc",
      }).map((r) => r.id),
    ).toEqual(["b"]);
    expect(
      visibleRows(audit, null, "nothing", { key: "title", dir: "asc" }),
    ).toEqual([]);
  });
});

describe("table columns", () => {
  it("opens on the lowest score when there are scores", () => {
    expect(defaultSort(audit)).toEqual({ key: "score", dir: "asc" });
    expect(defaultSort([f({ title: "x" })])).toEqual({
      key: "title",
      dir: "asc",
    });
    expect(columnsOf([f({ title: "x" })])).toEqual({
      location: false,
      score: false,
      duration: false,
    });
  });

  it("keys a finding by its id, so a repeated title stays two findings", () => {
    const one = f({
      id: "web/a.test.ts :: works",
      title: "works",
      category: "keep",
    });
    const two = f({
      id: "web/b.test.ts :: works",
      title: "works",
      category: "keep",
    });
    expect(findingKey(one)).not.toEqual(findingKey(two));
    expect(findingKey(f({ title: "works", category: "keep" }))).toBe(
      "keep::works",
    );
  });

  it("counts anything but a decision as undecided", () => {
    expect(decisionState("approved")).toBe("approved");
    expect(decisionState("applied")).toBe("applied");
    expect(decisionState("built")).toBe("undecided");
    expect(decisionState(undefined)).toBe("undecided");
  });

  it("formats durations at a readable scale", () => {
    expect(formatMs(12.4)).toBe("12 ms");
    expect(formatMs(3400)).toBe("3.4 s");
    expect(formatMs(125_000)).toBe("2 min 5 s");
  });
});
