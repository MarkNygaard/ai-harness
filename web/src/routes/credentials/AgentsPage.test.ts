import { describe, expect, it } from "vitest";
import { AGENTS } from "./AgentsPage";

/**
 * The Agents page is keyed on what a workflow types in `provider:`, and each
 * row names the accounts that back it. Both halves are hand-maintained tables
 * that have to agree with the server's, so these pin the facts that would
 * otherwise drift silently into a row that lies.
 */
describe("the agent table", () => {
  it("offers exactly the providers a workflow node can name", () => {
    // Matches AGENT_CLIS in crates/harness-server/src/http/system_routes.rs and
    // DispatchAgent in crates/harness-runner/src/dispatch.rs.
    expect(AGENTS.map((a) => a.provider)).toEqual([
      "claude",
      "codex",
      "pi",
      "cursor",
    ]);
  });

  it("shows Pi backed by both of the accounts that can run it", () => {
    // The reason the page exists: Pi runs kimi-code/* on a Kimi plan and
    // openai-codex/* on a ChatGPT one, so a single-subscription row would be
    // wrong about one of them — and either alone is enough to run it.
    const pi = AGENTS.find((a) => a.provider === "pi");
    expect(pi?.subscriptions).toEqual(["pi", "codex"]);
  });

  it("backs every agent with at least one account that exists", () => {
    const known = new Set(["claude", "codex", "pi", "cursor"]);
    for (const agent of AGENTS) {
      expect(agent.subscriptions.length).toBeGreaterThan(0);
      for (const sub of agent.subscriptions) {
        expect(known).toContain(sub);
      }
    }
  });
});
