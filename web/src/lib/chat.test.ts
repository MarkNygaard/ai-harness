import { describe, expect, it } from "vitest";
import {
  type ChatAction,
  contextFromPath,
  describeAction,
  describeContext,
  toolSteps,
} from "./chat";

describe("contextFromPath", () => {
  it("names the run or workflow that is open", () => {
    expect(contextFromPath("/runs/abc-123")).toEqual({ run_id: "abc-123" });
    expect(contextFromPath("/editor/idea-to-pr")).toEqual({
      workflow: "idea-to-pr",
    });
    expect(contextFromPath("/settings/workflows/idea-to-pr")).toEqual({
      workflow: "idea-to-pr",
    });
  });

  it("ignores pages that are not one run or one workflow", () => {
    expect(contextFromPath("/runs/pair/p1")).toEqual({});
    expect(contextFromPath("/editor/new")).toEqual({});
    expect(contextFromPath("/settings/workflows/new")).toEqual({});
    expect(contextFromPath("/settings/workflows")).toEqual({});
    expect(contextFromPath("/runs")).toEqual({});
    expect(contextFromPath("/")).toEqual({});
  });

  it("decodes names", () => {
    expect(contextFromPath("/editor/my%20flow")).toEqual({
      workflow: "my flow",
    });
  });
});

describe("describeContext", () => {
  it("shortens a run id and says nothing when nothing is open", () => {
    expect(describeContext({ run_id: "0123456789abcdef" })).toBe(
      "run 01234567",
    );
    expect(describeContext({ workflow: "review-pr" })).toBe(
      "the review-pr workflow",
    );
    expect(describeContext({})).toBeNull();
  });
});

describe("describeAction", () => {
  const action = (tool: string, args: Record<string, unknown>): ChatAction => ({
    tool,
    args,
    status: "pending",
  });

  it("says what a held call will do", () => {
    expect(
      describeAction(
        action("run_trigger", { workflow: "idea-to-pr", project: "shop" }),
      ),
    ).toBe("Start a run of idea-to-pr in shop.");
    expect(describeAction(action("run_trigger", {}))).toBe(
      "Start a run of the project's default workflow.",
    );
    expect(describeAction(action("workflow_delete", { name: "old" }))).toBe(
      "Delete the workflow old. It stays in the bin for 14 days.",
    );
    expect(describeAction(action("something_new", {}))).toBe(
      "Run something_new.",
    );
  });
});

describe("toolSteps", () => {
  it("pairs each call with its result by call id", () => {
    expect(
      toolSteps([
        { kind: "tool", text: "issue_prs", detail: "ECOM-1", tool_id: "a" },
        { kind: "tool", text: "run_status", tool_id: "b" },
        { kind: "text", text: "thinking" },
        {
          kind: "tool_result",
          text: "",
          detail: "boom",
          tool_id: "b",
          is_error: true,
        },
        { kind: "tool_result", text: "", detail: "2 PRs", tool_id: "a" },
      ]),
    ).toEqual([
      {
        name: "issue_prs",
        input: "ECOM-1",
        output: "2 PRs",
        error: false,
        done: true,
      },
      {
        name: "run_status",
        input: undefined,
        output: "boom",
        error: true,
        done: true,
      },
    ]);
  });

  it("pairs in order without ids, and leaves an unanswered call open", () => {
    const steps = toolSteps([
      { kind: "tool", text: "first" },
      { kind: "tool", text: "second" },
      { kind: "tool_result", text: "", detail: "one" },
    ]);
    expect(steps.map((s) => [s.name, s.output, s.done])).toEqual([
      ["first", "one", true],
      ["second", undefined, false],
    ]);
  });
});
