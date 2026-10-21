import { describe, expect, it } from "vitest";
import {
  type ChatAction,
  contextFromPath,
  describeAction,
  describeContext,
  toolCalls,
} from "./chat";

describe("contextFromPath", () => {
  it("names the run or workflow that is open", () => {
    expect(contextFromPath("/runs/abc-123")).toEqual({ run_id: "abc-123" });
    expect(contextFromPath("/editor/idea-to-pr")).toEqual({
      workflow: "idea-to-pr",
    });
  });

  it("ignores pages that are not one run or one workflow", () => {
    expect(contextFromPath("/runs/pair/p1")).toEqual({});
    expect(contextFromPath("/editor/new")).toEqual({});
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

describe("toolCalls", () => {
  it("keeps the calls, not their results", () => {
    expect(
      toolCalls([
        { kind: "tool", text: "issue_prs" },
        { kind: "tool_result", text: "2 PRs" },
        { kind: "text", text: "thinking" },
      ]),
    ).toEqual([{ kind: "tool", text: "issue_prs" }]);
  });
});
