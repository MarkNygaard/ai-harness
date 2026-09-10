import { describe, expect, it } from "vitest";
import { isMetered } from "./models";

describe("isMetered", () => {
  /** The case the badge exists for: one Cursor list, two billing regimes. */
  it("splits Cursor's own models from the ones it charges API price for", () => {
    expect(isMetered("cursor", "composer-2.5")).toBe(false);
    expect(isMetered("cursor", "grok-4.6")).toBe(false);
    expect(isMetered("cursor", "claude-opus-5")).toBe(true);
    expect(isMetered("cursor", "gpt-5.6-sol")).toBe(true);
  });

  /** The same id bills differently depending on which agent reaches it. */
  it("is keyed on the agent, not the model alone", () => {
    expect(isMetered("claude", "sonnet")).toBe(false);
    expect(isMetered("anthropic-api", "sonnet")).toBe(true);
  });

  it("treats subscription CLIs as included", () => {
    expect(isMetered("codex", "gpt-6-astra")).toBe(false);
    expect(isMetered("pi", "kimi-code/kimi-for-coding")).toBe(false);
  });

  /**
   * A workflow can pin any string. Silence is the right answer for one nobody
   * listed — warning about a cost nobody established would be a guess shown as
   * a fact.
   */
  it("says nothing about a model it does not know", () => {
    expect(isMetered("cursor", "some-future-model")).toBe(false);
    expect(isMetered("nonesuch", "sonnet")).toBe(false);
    expect(isMetered(undefined, "sonnet")).toBe(false);
    expect(isMetered("cursor", undefined)).toBe(false);
  });
});
