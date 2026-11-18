import { describe, expect, it } from "vitest";
import {
  actionsFor,
  editorLink,
  filterOptions,
  type ImprovementSuggestion,
} from "./improvements";

function suggestion(
  over: Partial<ImprovementSuggestion> = {},
): ImprovementSuggestion {
  return {
    id: "s-1",
    review_id: "r-1",
    workflow: "ecom-task",
    project: "dilling-ecom",
    target: "repo",
    category: "skill",
    node: null,
    title: "t",
    problem: "p",
    change: "c",
    evidence: [],
    status: "new",
    applied_run_id: null,
    seen_count: 1,
    last_seen_at: "2026-11-18T00:00:00Z",
    created_at: "2026-11-18T00:00:00Z",
    updated_at: "2026-11-18T00:00:00Z",
    ...over,
  };
}

describe("actionsFor", () => {
  it("applies a repo change as a run, and a workflow change in the editor", () => {
    expect(actionsFor(suggestion({ target: "repo" }))).toEqual([
      "apply",
      "dismiss",
    ]);
    // The server refuses to apply a workflow suggestion: a person edits the
    // workflow and says so.
    expect(actionsFor(suggestion({ target: "workflow" }))).toEqual([
      "open-editor",
      "mark-applied",
      "dismiss",
    ]);
  });

  it("only reopens a decided suggestion", () => {
    expect(actionsFor(suggestion({ status: "applied" }))).toEqual(["reopen"]);
    expect(
      actionsFor(suggestion({ status: "dismissed", target: "workflow" })),
    ).toEqual(["reopen"]);
  });
});

describe("editorLink", () => {
  it("opens the editor on the step a suggestion names", () => {
    expect(editorLink({ workflow: "ecom-task", node: "verify-pr-title" })).toBe(
      "/settings/workflows/ecom-task?node=verify-pr-title",
    );
    expect(editorLink({ workflow: "ecom-task", node: null })).toBe(
      "/settings/workflows/ecom-task",
    );
  });

  it("opens the workflow when the suggestion names several steps", () => {
    expect(
      editorLink({
        workflow: "ecom-task",
        node: "review-correctness / review-architecture",
      }),
    ).toBe("/settings/workflows/ecom-task");
  });
});

describe("filterOptions", () => {
  it("collects workflows and projects from reviews and suggestions", () => {
    const opts = filterOptions({
      workflows: [
        {
          workflow: "triage-todo",
          project: "dilling-ecom",
          every: 10,
          runs_since_review: 0,
          review: null,
        },
        {
          workflow: "bc-task",
          project: "business-central",
          every: 10,
          runs_since_review: 1,
          review: null,
        },
      ],
      suggestions: [suggestion(), suggestion({ id: "s-2" })],
    });
    expect(opts.workflows).toEqual(["bc-task", "ecom-task", "triage-todo"]);
    expect(opts.projects).toEqual(["business-central", "dilling-ecom"]);
    expect(filterOptions(undefined)).toEqual({ workflows: [], projects: [] });
  });
});
