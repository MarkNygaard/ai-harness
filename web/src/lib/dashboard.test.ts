import { describe, expect, it } from "vitest";
import {
  buildProjectSummaries,
  NO_PROJECT,
  recentDayKeys,
  utcDayKey,
} from "./dashboard";
import type { RunDailyCount } from "@/types/run";

describe("utcDayKey", () => {
  it("slices the date from an ISO timestamp", () => {
    expect(utcDayKey("2026-06-08T12:34:56Z")).toBe("2026-06-08");
  });
});

describe("recentDayKeys", () => {
  it("returns the right count and order around a UTC month boundary", () => {
    // June 1 UTC → going back 3 days should include May 30, May 31, June 1
    const now = new Date(Date.UTC(2026, 5, 1)); // 2026-06-01
    const keys = recentDayKeys(now, 3);
    expect(keys).toEqual(["2026-06-01", "2026-05-31", "2026-05-30"]);
  });

  it("returns the requested number of days", () => {
    const now = new Date(Date.UTC(2026, 0, 15));
    expect(recentDayKeys(now, 5)).toHaveLength(5);
  });
});

describe("buildProjectSummaries", () => {
  it("folds cancelled into failed, sums completed into total, and sorts by total desc", () => {
    const rows: RunDailyCount[] = [
      {
        project: "proj-a",
        day: "2026-06-08T00:00:00Z",
        status: "completed",
        count: 3,
      },
      {
        project: "proj-a",
        day: "2026-06-08T00:00:00Z",
        status: "failed",
        count: 1,
      },
      {
        project: "proj-b",
        day: "2026-06-08T00:00:00Z",
        status: "completed",
        count: 1,
      },
      {
        project: "proj-b",
        day: "2026-06-08T00:00:00Z",
        status: "cancelled",
        count: 2,
      },
    ];
    const summaries = buildProjectSummaries(rows);
    expect(summaries).toHaveLength(2);
    expect(summaries[0].project).toBe("proj-a");
    expect(summaries[0].total).toBe(3);
    expect(summaries[0].byDay["2026-06-08"]).toEqual({
      completed: 3,
      failed: 1,
    });
    expect(summaries[1].project).toBe("proj-b");
    expect(summaries[1].total).toBe(1);
    expect(summaries[1].byDay["2026-06-08"]).toEqual({
      completed: 1,
      failed: 2,
    });
  });

  it("maps null/empty project to NO_PROJECT", () => {
    const rows: RunDailyCount[] = [
      {
        project: null,
        day: "2026-06-08T00:00:00Z",
        status: "completed",
        count: 1,
      },
      {
        project: "  ",
        day: "2026-06-08T00:00:00Z",
        status: "completed",
        count: 2,
      },
    ];
    const summaries = buildProjectSummaries(rows);
    expect(summaries).toHaveLength(1);
    expect(summaries[0].project).toBe(NO_PROJECT);
    expect(summaries[0].total).toBe(3);
  });

  it("keeps real projects named like the unassigned label separate", () => {
    const rows: RunDailyCount[] = [
      {
        project: null,
        day: "2026-06-08T00:00:00Z",
        status: "completed",
        count: 1,
      },
      {
        project: NO_PROJECT,
        day: "2026-06-08T00:00:00Z",
        status: "completed",
        count: 2,
      },
    ];
    const summaries = buildProjectSummaries(rows);

    expect(summaries).toHaveLength(2);
    expect(summaries.map((s) => s.total).sort()).toEqual([1, 2]);
    expect(summaries.find((s) => s.isUnassigned)?.total).toBe(1);
    expect(
      summaries.find((s) => !s.isUnassigned && s.project === NO_PROJECT)?.total,
    ).toBe(2);
  });

  it("falls back to project name sort when totals are tied", () => {
    const rows: RunDailyCount[] = [
      {
        project: "zebra",
        day: "2026-06-08T00:00:00Z",
        status: "completed",
        count: 1,
      },
      {
        project: "alpha",
        day: "2026-06-08T00:00:00Z",
        status: "completed",
        count: 1,
      },
    ];
    const summaries = buildProjectSummaries(rows);
    expect(summaries[0].project).toBe("alpha");
    expect(summaries[1].project).toBe("zebra");
  });
  it("buckets multiple days for the same project", () => {
    const rows: RunDailyCount[] = [
      {
        project: "proj-a",
        day: "2026-06-08T00:00:00Z",
        status: "completed",
        count: 2,
      },
      {
        project: "proj-a",
        day: "2026-06-07T00:00:00Z",
        status: "completed",
        count: 1,
      },
      {
        project: "proj-a",
        day: "2026-06-07T00:00:00Z",
        status: "failed",
        count: 1,
      },
    ];
    const summaries = buildProjectSummaries(rows);
    expect(summaries).toHaveLength(1);
    expect(summaries[0].total).toBe(3);
    expect(summaries[0].byDay["2026-06-08"]).toEqual({
      completed: 2,
      failed: 0,
    });
    expect(summaries[0].byDay["2026-06-07"]).toEqual({
      completed: 1,
      failed: 1,
    });
  });
});
