import { describe, expect, it } from "vitest";
import { browserTimezone, describeCron, PRESETS } from "./schedules";

describe("describeCron", () => {
  it("names a preset in words, whatever the spacing", () => {
    expect(describeCron("0 8 * * 1")).toBe("Every Monday at 08:00");
    expect(describeCron("  0   8 * *   1 ")).toBe("Every Monday at 08:00");
  });

  it("says nothing for an expression it has no words for", () => {
    expect(describeCron("15 6 * * 2")).toBeNull();
  });

  it("has five-field presets only", () => {
    for (const p of PRESETS) expect(p.cron.split(" ")).toHaveLength(5);
  });
});

describe("browserTimezone", () => {
  it("returns a zone name", () => {
    expect(browserTimezone()).toMatch(/\w/);
  });
});
