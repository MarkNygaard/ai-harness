import { describe, expect, it } from "vitest";
import { type ClaudeAccount, describePlan } from "./claude-accounts";

function acct(over: Partial<ClaudeAccount>): ClaudeAccount {
  return {
    id: "a",
    label: "A",
    tier: null,
    weight: 1,
    weight_source: "default",
    connected: true,
    ...over,
  };
}

describe("describePlan", () => {
  it("names the plan the credential carries", () => {
    expect(
      describePlan(
        acct({
          tier: "default_claude_max_5x",
          weight: 5,
          weight_source: "credential",
        }),
      ),
    ).toBe("Max 5x");
    expect(
      describePlan(
        acct({
          tier: "default_claude_max_20x",
          weight: 20,
          weight_source: "credential",
        }),
      ),
    ).toBe("Max 20x");
    expect(
      describePlan(
        acct({ tier: "default_claude_pro", weight_source: "credential" }),
      ),
    ).toBe("Pro");
  });

  it("says when the size was set by hand, or is unknown", () => {
    expect(describePlan(acct({ weight: 10, weight_source: "override" }))).toBe(
      "size 10 (set here)",
    );
    expect(describePlan(acct({}))).toBe("plan unknown, counted as Pro");
  });
});
