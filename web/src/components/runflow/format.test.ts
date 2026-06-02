import { describe, expect, it } from "vitest";
import {
  elapsedMs,
  formatDuration,
  formatTokens,
  statusColor,
  sumUsage,
  totalTokens,
} from "./format";

describe("formatDuration", () => {
  it("renders ms / s / m / h thresholds", () => {
    expect(formatDuration(null)).toBe("—");
    expect(formatDuration(420)).toBe("420ms");
    expect(formatDuration(3400)).toBe("3.4s");
    expect(formatDuration(125_000)).toBe("2m 05s");
    expect(formatDuration(3_780_000)).toBe("1h 03m");
  });
});

describe("formatTokens", () => {
  it("compacts thousands and millions", () => {
    expect(formatTokens(null)).toBe("—");
    expect(formatTokens(950)).toBe("950");
    expect(formatTokens(1234)).toBe("1.2k");
    expect(formatTokens(2_500_000)).toBe("2.50M");
  });
});

describe("elapsedMs", () => {
  it("uses now when end is missing and clamps to zero", () => {
    const start = "2026-01-01T00:00:00.000Z";
    const now = Date.parse("2026-01-01T00:00:05.000Z");
    expect(elapsedMs(start, null, now)).toBe(5000);
    expect(elapsedMs(start, "2026-01-01T00:00:02.000Z", now)).toBe(2000);
    expect(elapsedMs(null, null, now)).toBeNull();
  });
});

describe("usage helpers", () => {
  it("totals input+output and preserves nulls when nothing reported", () => {
    expect(totalTokens({ input: 10, output: 5, cache_read: null, cache_write: null })).toBe(15);
    const summed = sumUsage([
      { input: 10, output: 5, cache_read: 2, cache_write: null },
      { input: 3, output: null, cache_read: null, cache_write: null },
    ]);
    expect(summed).toEqual({ input: 13, output: 5, cache_read: 2, cache_write: null });
    expect(sumUsage([]).input).toBeNull();
  });
});

describe("statusColor", () => {
  it("maps states to themed tokens", () => {
    expect(statusColor("running")).toContain("running");
    expect(statusColor("success")).toContain("success");
    expect(statusColor("failed")).toContain("failed");
    expect(statusColor("cancelled")).toContain("failed");
    expect(statusColor("skipped")).toContain("skipped");
  });
});
