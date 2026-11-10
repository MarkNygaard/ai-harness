import { describe, expect, it } from "vitest";
import { groupByPage } from "./screenshots";

describe("groupByPage", () => {
  it("puts a page's viewports together, in the agents' order", () => {
    const groups = groupByPage([
      { file: "pdp-desktop.png", page: "/p/1", viewport: "desktop" },
      { file: "plp-desktop.png", page: "/c/shirts", viewport: "desktop" },
      { file: "pdp-mobile.png", page: "/p/1", viewport: "mobile" },
    ]);
    expect(groups.map(([page]) => page)).toEqual(["/p/1", "/c/shirts"]);
    expect(groups[0][1].map((s) => s.viewport)).toEqual(["desktop", "mobile"]);
  });

  it("files an undescribed screenshot under its own name", () => {
    expect(groupByPage([{ file: "x.png" }])).toEqual([
      ["x.png", [{ file: "x.png" }]],
    ]);
  });
});
