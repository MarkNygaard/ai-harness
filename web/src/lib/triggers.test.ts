import { describe, expect, it } from "vitest";
import type { Schedule } from "./schedules";
import {
  type RunTrigger,
  describeWhen,
  filterTriggers,
  toTriggers,
  triggerHref,
  triggerState,
} from "./triggers";

const base = {
  enabled: true,
  last_fired_at: null,
  last_run_id: null,
  last_error: null,
  created_by: null,
  created_at: "2026-11-01T00:00:00Z",
  updated_at: "2026-11-01T00:00:00Z",
};

function linear(over: Partial<RunTrigger> = {}): RunTrigger {
  return {
    ...base,
    id: "lin-1",
    name: "Todo → task",
    kind: "linear",
    project: "ai-harness",
    workflow: "task",
    config: {
      team_id: "t1",
      team_name: "AIH",
      state_id: "s1",
      state_name: "Todo",
      max_concurrent_runs: 1,
    },
    ...over,
  } as RunTrigger;
}

function webhook(over: Partial<RunTrigger> = {}): RunTrigger {
  return {
    ...base,
    id: "hook-1",
    name: "PR opened",
    kind: "webhook",
    project: "niles",
    workflow: "review-pr",
    url: "https://h.example/api/hooks/hook-1",
    config: {
      auth: "signature",
      filters: [{ path: "body.action", equals: ["opened", "reopened"] }],
      task_template: "Review {{body.pull_request.html_url}}",
    },
    ...over,
  } as RunTrigger;
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

describe("toTriggers", () => {
  it("merges every kind into one list, by workflow then project", () => {
    const rows = toTriggers([linear(), webhook()], [schedule()]);
    expect(rows.map((t) => t.kind)).toEqual(["schedule", "webhook", "linear"]);
  });

  it("keys rows uniquely across kinds", () => {
    const rows = toTriggers([linear({ id: "x" })], [schedule({ id: "x" })]);
    expect(new Set(rows.map((t) => t.key)).size).toBe(2);
  });
});

describe("triggerState", () => {
  const [lin] = toTriggers([linear()], []);
  const [hook] = toTriggers([webhook()], []);
  const [off] = toTriggers([linear({ enabled: false })], []);

  it("calls a Linear trigger idle while classic bindings are in charge", () => {
    expect(triggerState(lin, "classic")).toBe("idle");
    expect(triggerState(lin, "triggers")).toBe("on");
  });

  it("leaves webhooks on under either system", () => {
    expect(triggerState(hook, "classic")).toBe("on");
  });

  it("reads a disabled trigger as off, whatever the system", () => {
    expect(triggerState(off, "triggers")).toBe("off");
    expect(triggerState(off, "classic")).toBe("off");
  });
});

describe("describeWhen", () => {
  it("names a Linear trigger's team, status and how it starts", () => {
    const [plain] = toTriggers([linear()], []);
    expect(describeWhen(plain)).toBe("AIH · Todo · delegated");
    const [labelled] = toTriggers(
      [
        linear({
          config: {
            team_id: "t1",
            team_name: "AIH",
            state_id: "s1",
            state_name: "Todo",
            label: "go",
            linear_project_id: "p1",
            linear_project_name: "Checkout",
            max_concurrent_runs: 1,
          },
        } as Partial<RunTrigger>),
      ],
      [],
    );
    expect(describeWhen(labelled)).toBe(
      "AIH · Todo · only Checkout · delegated or labelled go",
    );
  });

  it("spells out a webhook's conditions, or says it takes anything signed", () => {
    const [hook] = toTriggers([webhook()], []);
    expect(describeWhen(hook)).toBe("body.action = opened | reopened");
    const [open] = toTriggers(
      [
        webhook({
          config: { auth: "token", filters: [], task_template: "" },
        } as Partial<RunTrigger>),
      ],
      [],
    );
    expect(describeWhen(open)).toBe("any signed delivery");
  });

  it("uses a schedule's preset words", () => {
    const [s] = toTriggers([], [schedule()]);
    expect(describeWhen(s)).toBe("Every Monday at 08:00 · Europe/Copenhagen");
  });
});

describe("filterTriggers", () => {
  const rows = toTriggers([linear(), webhook()], [schedule()]);

  it("narrows by kind, project and workflow", () => {
    expect(filterTriggers(rows, { kind: "webhook" })).toHaveLength(1);
    expect(filterTriggers(rows, { project: "ticket0" })).toHaveLength(1);
    expect(filterTriggers(rows, { workflow: "task" })).toHaveLength(1);
    expect(filterTriggers(rows, {})).toHaveLength(3);
  });

  it("searches names, conditions and templates", () => {
    expect(filterTriggers(rows, { q: "weekly inp" })).toHaveLength(1);
    expect(filterTriggers(rows, { q: "pull_request" })).toHaveLength(1);
    expect(filterTriggers(rows, { q: "aih todo" })).toHaveLength(1);
    expect(filterTriggers(rows, { q: "aih weekly" })).toHaveLength(0);
  });
});

describe("triggerHref", () => {
  it("sends each kind to its own page", () => {
    const [s, h, l] = toTriggers([linear(), webhook()], [schedule()]);
    expect(triggerHref(s)).toBe("/triggers/schedule/sch-1");
    expect(triggerHref(h)).toBe("/triggers/hook-1");
    expect(triggerHref(l)).toBe("/triggers/lin-1");
  });
});
