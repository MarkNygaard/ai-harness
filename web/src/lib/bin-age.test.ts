import { describe, expect, it } from "vitest";
import { clearedIn, daysAgo } from "./bin-age";

const DAY = 86_400;

describe("bin entry ages", () => {
  it("says how long ago a workflow was deleted", () => {
    expect(daysAgo(0, 3600)).toBe("today");
    expect(daysAgo(0, DAY + 1)).toBe("yesterday");
    expect(daysAgo(0, 5 * DAY)).toBe("5 days ago");
  });

  it("says when it will be cleared out, rounding up", () => {
    const deleted = 0;
    const expires = deleted + 14 * DAY;
    expect(clearedIn(expires, deleted)).toBe("in 14 days");
    expect(clearedIn(expires, 13 * DAY + 1)).toBe("in 1 day");
    expect(clearedIn(expires, expires)).toBe("today");
  });
});
