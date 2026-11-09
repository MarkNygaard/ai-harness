import { describe, expect, it } from "vitest";
import { clampEvery } from "./WorkflowImprovementsDrawer";

describe("clampEvery", () => {
  it("keeps a value inside the server's 3–100 range", () => {
    expect(clampEvery(10)).toBe(10);
    expect(clampEvery(3)).toBe(3);
    expect(clampEvery(100)).toBe(100);
  });

  it("pulls an out-of-range value to the nearest bound", () => {
    expect(clampEvery(1)).toBe(3);
    expect(clampEvery(0)).toBe(3);
    expect(clampEvery(500)).toBe(100);
  });

  it("rounds a fraction and falls back to the default for nonsense", () => {
    expect(clampEvery(7.6)).toBe(8);
    expect(clampEvery(Number.NaN)).toBe(10);
  });
});
