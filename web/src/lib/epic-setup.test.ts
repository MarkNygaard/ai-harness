import { describe, expect, it } from "vitest";
import type { LinearSource } from "@/types/linear";
import {
  epicBindingWrites,
  epicColumnsProblem,
  isBuilder,
  MERGE_WORKFLOW,
  SUPERVISOR_WORKFLOW,
} from "./epic-setup";

function binding(over: Partial<LinearSource>): LinearSource {
  return {
    project: "p",
    workflow: "bc-idea-to-pr",
    team_id: "erp",
    team_name: "ERP",
    source_state_id: "todo",
    failed_label: null,
    in_progress_state_id: "in-progress",
    review_state_id: null,
    ready_state_id: "functional-testing",
    piece_ready_state_id: null,
    epic_review_state_id: null,
    base_branch: "main",
    poll_interval_secs: 60,
    max_concurrent_runs: 2,
    max_attempts: 1,
    enabled: true,
    live: true,
    created_at: "",
    updated_at: "",
    ...over,
  };
}

const columns = {
  merge: "ready-for-merge",
  merged: "done",
  finished: "functional-testing",
};

describe("epicBindingWrites", () => {
  it("wires every hand-off of the relay from a build binding alone", () => {
    const build = binding({});
    const writes = epicBindingWrites([build], build, columns);
    const by = (w: string) => writes.find((x) => x.workflow === w)!;

    // The build binding keeps its own Ready for standalone issues.
    expect(by("bc-idea-to-pr")).toMatchObject({
      source_state_id: "todo",
      ready_state_id: "functional-testing",
      piece_ready_state_id: "ready-for-merge",
      max_concurrent_runs: 2,
    });
    // Each hand-off: one binding's ready column is the next one's source.
    expect(by(MERGE_WORKFLOW)).toMatchObject({
      team_id: "erp",
      source_state_id: "ready-for-merge",
      ready_state_id: "done",
      enabled: true,
      live: true,
    });
    expect(by(SUPERVISOR_WORKFLOW)).toMatchObject({
      source_state_id: "done",
      epic_review_state_id: "functional-testing",
      base_branch: "main",
      live: true,
    });
  });

  it("keeps an existing binding's own settings and re-points revise-pr", () => {
    const build = binding({});
    const merge = binding({
      workflow: MERGE_WORKFLOW,
      source_state_id: "old",
      failed_label: "harness-failed",
      max_attempts: 3,
    });
    const revise = binding({
      workflow: "revise-pr",
      source_state_id: "changes-requested",
    });
    const writes = epicBindingWrites([build, merge, revise], build, {
      ...columns,
      finished: "",
    });

    expect(writes.find((w) => w.workflow === MERGE_WORKFLOW)).toMatchObject({
      source_state_id: "ready-for-merge",
      failed_label: "harness-failed",
      max_attempts: 3,
    });
    expect(writes.find((w) => w.workflow === "revise-pr")).toMatchObject({
      source_state_id: "changes-requested",
      piece_ready_state_id: "ready-for-merge",
    });
    expect(
      writes.find((w) => w.workflow === SUPERVISOR_WORKFLOW)
        ?.epic_review_state_id,
    ).toBeUndefined();
  });
});

describe("epicColumnsProblem", () => {
  it("refuses a column shared by two bindings of the relay", () => {
    const build = binding({});
    expect(epicColumnsProblem(build, columns)).toBeNull();
    expect(
      epicColumnsProblem(build, { ...columns, merged: "ready-for-merge" }),
    ).not.toBeNull();
    expect(
      epicColumnsProblem(build, { ...columns, merge: "todo" }),
    ).not.toBeNull();
  });
});

describe("isBuilder", () => {
  it("excludes the workflows that only relay pieces", () => {
    expect(isBuilder(binding({}))).toBe(true);
    for (const workflow of [MERGE_WORKFLOW, SUPERVISOR_WORKFLOW, "revise-pr"]) {
      expect(isBuilder(binding({ workflow }))).toBe(false);
    }
  });
});
