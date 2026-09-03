import { beforeEach, describe, expect, it } from "vitest";
import {
  AGENTS,
  AGENT_UPDATES_SEEN_KEY,
  describeAgentUpdates,
  markAgentVersionsSeen,
  pendingAgentUpdates,
  readSeenAgentVersions,
} from "./agents";
import type { ProviderHealth } from "./system";

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

/** A `/api/system/providers` row, with only the fields the notice reads set. */
function health(over: Partial<ProviderHealth>): ProviderHealth {
  return {
    provider: "claude",
    binary: "claude",
    on_path: true,
    version: "2.0.9",
    latest: "2.1.4",
    update_available: true,
    error: null,
    ...over,
  };
}

/**
 * Which updates are worth interrupting an administrator about.
 *
 * The rule that matters is the one about versions: a dismissal has to be
 * specific to what was dismissed, or the first person to wave the notice away
 * silences every update this installation will ever have.
 */
describe("pendingAgentUpdates", () => {
  beforeEach(() => localStorage.clear());

  it("offers an update nobody has been shown", () => {
    expect(pendingAgentUpdates([health({})], {})).toEqual([
      {
        provider: "claude",
        label: "Claude Code",
        latest: "2.1.4",
        installed: "2.0.9",
      },
    ]);
  });

  it("stays quiet about the version already dismissed", () => {
    expect(pendingAgentUpdates([health({})], { claude: "2.1.4" })).toEqual([]);
  });

  it("speaks up again when a newer version than the dismissed one lands", () => {
    const updates = pendingAgentUpdates([health({ latest: "2.2.0" })], {
      claude: "2.1.4",
    });
    expect(updates.map((u) => u.latest)).toEqual(["2.2.0"]);
  });

  it("ignores a CLI the server cannot install", () => {
    // `omp` and `cursor-agent` come from outside npm and always report false,
    // so a notice about them would have no button behind it.
    const rows = [
      health({ provider: "pi", update_available: false, latest: null }),
      health({ provider: "cursor", update_available: false, latest: "1.0.0" }),
    ];
    expect(pendingAgentUpdates(rows, {})).toEqual([]);
  });

  it("has nothing to say before the health query answers", () => {
    expect(pendingAgentUpdates(undefined, {})).toEqual([]);
  });

  it("labels a provider it does not know by its own name", () => {
    const updates = pendingAgentUpdates([health({ provider: "gemini" })], {});
    expect(updates[0].label).toBe("gemini");
  });
});

describe("the dismissal record", () => {
  beforeEach(() => localStorage.clear());

  it("round-trips through storage, per provider", () => {
    markAgentVersionsSeen([
      {
        provider: "claude",
        label: "Claude Code",
        latest: "2.1.4",
        installed: "2.0.9",
      },
    ]);
    markAgentVersionsSeen([
      { provider: "codex", label: "Codex", latest: "0.9.0", installed: null },
    ]);
    expect(readSeenAgentVersions()).toEqual({
      claude: "2.1.4",
      codex: "0.9.0",
    });
  });

  it("reads as nothing-seen when storage holds something else", () => {
    // Anything but the shape this app wrote — an older version of it, or a key
    // collision — must show the notice rather than swallow it.
    localStorage.setItem(AGENT_UPDATES_SEEN_KEY, '["claude"]');
    expect(readSeenAgentVersions()).toEqual({});
    localStorage.setItem(AGENT_UPDATES_SEEN_KEY, "not json");
    expect(readSeenAgentVersions()).toEqual({});
    localStorage.setItem(AGENT_UPDATES_SEEN_KEY, '{"claude": 3}');
    expect(readSeenAgentVersions()).toEqual({});
  });
});

describe("describeAgentUpdates", () => {
  it("names the one agent, and what is installed", () => {
    expect(
      describeAgentUpdates([
        {
          provider: "claude",
          label: "Claude Code",
          latest: "2.1.4",
          installed: "2.0.9",
        },
      ]),
    ).toEqual({
      title: "Claude Code 2.1.4 is available",
      detail: "This container runs 2.0.9.",
    });
  });

  it("counts them, and lists which, when there are several", () => {
    const { title, detail } = describeAgentUpdates([
      {
        provider: "claude",
        label: "Claude Code",
        latest: "2.1.4",
        installed: "2.0.9",
      },
      {
        provider: "codex",
        label: "Codex",
        latest: "0.9.0",
        installed: "0.8.1",
      },
    ]);
    expect(title).toBe("2 agent updates are available");
    expect(detail).toBe("Claude Code 2.1.4, Codex 0.9.0");
  });

  it("says so rather than lying when the CLI would not report a version", () => {
    const { detail } = describeAgentUpdates([
      { provider: "codex", label: "Codex", latest: "0.9.0", installed: null },
    ]);
    expect(detail).toBe("The installed version did not report itself.");
  });
});
