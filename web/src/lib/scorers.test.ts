import { describe, expect, it } from "vitest";
import { passRate, trendPoints } from "./scorers";

describe("trendPoints", () => {
  it("covers every day, leaving days without grades empty, not zero", () => {
    const points = trendPoints(
      [
        { day: "2026-11-09", runs: 4, passed: 3, cost_usd: 1.2 },
        { day: "2026-11-11", runs: 1, passed: 0, cost_usd: 0.3 },
      ],
      3,
      "2026-11-11",
    );
    expect(points).toEqual([
      { day: "2026-11-09", passRate: 75, cost: 1.2, runs: 4 },
      { day: "2026-11-10", passRate: null, cost: null, runs: 0 },
      { day: "2026-11-11", passRate: 0, cost: 0.3, runs: 1 },
    ]);
  });

  it("crosses a month end", () => {
    const days = trendPoints([], 3, "2026-12-01").map((p) => p.day);
    expect(days).toEqual(["2026-11-29", "2026-11-30", "2026-12-01"]);
  });
});

describe("passRate", () => {
  it("is a whole percent, or a dash with no runs", () => {
    expect(passRate(2, 3)).toBe("67%");
    expect(passRate(0, 0)).toBe("—");
  });
});
