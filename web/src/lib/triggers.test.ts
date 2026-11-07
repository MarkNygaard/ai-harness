import { describe, expect, it } from "vitest";
import type { LinearSource, LinearTeam } from "@/types/linear";
import type { Schedule } from "./schedules";
import {
  describeLinearWhen,
  describeScheduleWhen,
  filterTriggers,
  fromBinding,
  toTriggers,
  triggerHref,
} from "./triggers";

function binding(over: Partial<LinearSource> = {}): LinearSource {
  return {
    project: "ai-harness",
    workflow: "idea-to-pr",
    team_id: "t1",
    team_name: "AIH",
    source_state_id: "s-todo",
    failed_label: null,
    in_progress_state_id: null,
    review_state_id: null,
    ready_state_id: null,
    piece_ready_state_id: null,
    epic_review_state_id: null,
    base_branch: null,
    poll_interval_secs: 60,
    max_concurrent_runs: 1,
    max_attempts: 1,
    enabled: true,
    live: true,
    linear_project_id: null,
    linear_project_name: null,
    trigger_label: null,
    created_at: "2026-11-01T00:00:00Z",
    updated_at: "2026-11-01T00:00:00Z",
    ...over,
  };
}

function schedule(over: Partial<Schedule> = {}): Schedule {
  return {
    id: "sch-1",
    name: "Weekly INP",
    project: "ticket0",
    workflow: "grafana-inp-report",
    cron: "0 8 * * 1",
    timezone: "Europe/Copenhagen",
    task: "Report on INP",
    enabled: true,
    next_run_at: null,
    last_run_at: null,
    last_run_id: null,
    created_by: null,
    created_at: "2026-11-01T00:00:00Z",
    updated_at: "2026-11-01T00:00:00Z",
    ...over,
  };
}

const teams: LinearTeam[] = [
  {
    id: "t1",
    name: "AIH",
    key: "AIH",
    states: [{ id: "s-todo", name: "Todo", kind: "unstarted", position: 1 }],
    labels: [],
  },
];

describe("fromBinding", () => {
  it("reads enabled + live as on, enabled alone as dry-run, else off", () => {
    expect(fromBinding(binding()).state).toBe("on");
    expect(fromBinding(binding({ live: false })).state).toBe("dry-run");
    expect(fromBinding(binding({ enabled: false, live: true })).state).toBe(
      "off",
    );
  });
});

describe("toTriggers", () => {
  it("merges both kinds into one list ordered by workflow, then project", () => {
    const rows = toTriggers(
      [
        binding({ workflow: "merge-pr" }),
        binding({ workflow: "idea-to-pr", project: "niles" }),
        binding({ workflow: "idea-to-pr" }),
      ],
      [schedule({ workflow: "geo-audit" })],
    );
    expect(rows.map((t) => `${t.workflow}/${t.project}/${t.kind}`)).toEqual([
      "geo-audit/ticket0/schedule",
      "idea-to-pr/ai-harness/linear",
      "idea-to-pr/niles/linear",
      "merge-pr/ai-harness/linear",
    ]);
  });

  it("gives every row a key unique across kinds", () => {
    const rows = toTriggers([binding()], [schedule()]);
    expect(new Set(rows.map((t) => t.key)).size).toBe(2);
  });
});

describe("filterTriggers", () => {
  const rows = toTriggers(
    [binding({ trigger_label: "harness:go" }), binding({ project: "niles" })],
    [schedule()],
  );

  it("narrows by kind, project and workflow together", () => {
    expect(filterTriggers(rows, { kind: "schedule" })).toHaveLength(1);
    expect(
      filterTriggers(rows, { kind: "linear", project: "niles" }),
    ).toHaveLength(1);
    expect(filterTriggers(rows, { workflow: "nope" })).toHaveLength(0);
  });

  it("matches every search word, across the kind's own fields", () => {
    expect(filterTriggers(rows, { q: "weekly inp" })).toHaveLength(1);
    expect(filterTriggers(rows, { q: "HARNESS:GO" })).toHaveLength(1);
    expect(filterTriggers(rows, { q: "every monday" })).toHaveLength(1);
    expect(filterTriggers(rows, { q: "weekly niles" })).toHaveLength(0);
  });

  it("returns everything when no filter is set", () => {
    expect(filterTriggers(rows, {})).toHaveLength(3);
  });
});

describe("describeLinearWhen", () => {
  it("names the status once discovery has loaded", () => {
    expect(describeLinearWhen(binding(), teams)).toBe("AIH · Todo · delegated");
  });

  it("still reads without discovery", () => {
    expect(describeLinearWhen(binding(), undefined)).toBe(
      "AIH · its start status · delegated",
    );
  });

  it("mentions the Linear project and the trigger label", () => {
    expect(
      describeLinearWhen(
        binding({
          linear_project_id: "p1",
          linear_project_name: "Checkout",
          trigger_label: "harness:go",
        }),
        teams,
      ),
    ).toBe("AIH · Todo · only Checkout · delegated or labelled harness:go");
  });
});

describe("describeScheduleWhen", () => {
  it("uses the preset's words when there are some, else the cron", () => {
    expect(describeScheduleWhen(schedule())).toBe(
      "Every Monday at 08:00 · Europe/Copenhagen",
    );
    expect(describeScheduleWhen(schedule({ cron: "15 6 * * 2" }))).toBe(
      "15 6 * * 2 · Europe/Copenhagen",
    );
  });
});

describe("triggerHref", () => {
  const [linear] = toTriggers([binding()], []);
  const [sched] = toTriggers([], [schedule()]);

  it("sends a Linear trigger to whichever home this browser chose", () => {
    expect(triggerHref(linear, "triggers")).toBe(
      "/triggers/linear/ai-harness/idea-to-pr",
    );
    expect(triggerHref(linear, "project")).toBe(
      "/settings/projects/ai-harness/linear",
    );
  });

  it("keeps schedules on the Triggers page either way", () => {
    expect(triggerHref(sched, "project")).toBe("/triggers/schedule/sch-1");
  });
});
