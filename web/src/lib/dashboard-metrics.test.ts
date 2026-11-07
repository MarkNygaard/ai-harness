import { describe, expect, it } from "vitest";
import {
  completionRate,
  dayLabel,
  delta,
  formatDuration,
  formatUsd,
} from "./dashboard-metrics";

describe("completionRate", () => {
  it("is completed over every finished run, cancelled included", () => {
    expect(completionRate({ completed: 3, failed: 1, cancelled: 0 })).toBe(
      0.75,
    );
    expect(completionRate({ completed: 1, failed: 0, cancelled: 1 })).toBe(0.5);
    expect(completionRate({ completed: 0, failed: 0, cancelled: 0 })).toBe(
      null,
    );
  });
});

describe("formatDuration", () => {
  it("reads at a glance", () => {
    expect(formatDuration(null)).toBe("—");
    expect(formatDuration(42)).toBe("42s");
    expect(formatDuration(14 * 60)).toBe("14m");
    expect(formatDuration(5400)).toBe("1.5h");
    expect(formatDuration(12 * 3600)).toBe("12h");
  });
});

describe("formatUsd", () => {
  it("keeps cents only while they matter", () => {
    expect(formatUsd(0.4)).toBe("$0.40");
    expect(formatUsd(12.4)).toBe("$12");
    expect(formatUsd(1234)).toBe("$1.2K");
  });
});

describe("delta", () => {
  it("judges a relative change by which way is better", () => {
    expect(delta(12, 10, { better: "up" })).toEqual({
      text: "+20%",
      good: true,
    });
    expect(delta(8, 10, { better: "up" })).toEqual({
      text: "−20%",
      good: false,
    });
    expect(delta(8, 10, { better: "down" })).toEqual({
      text: "−20%",
      good: true,
    });
    expect(delta(12, 10, { better: "none" })).toEqual({
      text: "+20%",
      good: null,
    });
  });

  it("gives rates in percentage points", () => {
    expect(delta(0.9, 0.8, { better: "up", kind: "points" })).toEqual({
      text: "+10 pts",
      good: true,
    });
  });

  it("says nothing it can't compute honestly", () => {
    expect(delta(5, 0, { better: "up" })).toBeNull();
    expect(delta(0, 0, { better: "up" })).toEqual({
      text: "no change",
      good: null,
    });
    expect(delta(null, 3, { better: "up" })).toBeNull();
  });
});

describe("dayLabel", () => {
  it("labels the UTC day, whatever the reader's zone", () => {
    // Locale-independent: the reader's locale formats it ("Nov 7", "7. nov.").
    expect(dayLabel("2026-11-07")).toMatch(/7/);
    expect(dayLabel("2026-11-07")).not.toBe(dayLabel("2026-11-08"));
    // Midnight UTC is still the 7th, not the 6th in a zone west of UTC.
    expect(dayLabel("2026-11-07")).not.toMatch(/\b6\b/);
  });
});
