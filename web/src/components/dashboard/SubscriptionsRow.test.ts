import { describe, expect, it } from "vitest";
import type { SubscriptionUsage } from "@/lib/usage";
import { groupByProvider, pooledWindows } from "./SubscriptionsRow";

function sub(over: Partial<SubscriptionUsage>): SubscriptionUsage {
  return {
    cli: "claude",
    label: "Claude Code",
    available: true,
    error: null,
    windows: [],
    ...over,
  };
}

describe("pooledWindows", () => {
  it("weighs each account by its plan", () => {
    const pooled = pooledWindows([
      // Max 20x, half used; Pro, empty: together 20·50 / 21 ≈ 47.6% used.
      sub({
        weight: 20,
        windows: [{ label: "Weekly", usedPct: 50, resetsAt: null }],
      }),
      sub({
        weight: 1,
        windows: [{ label: "Weekly", usedPct: 0, resetsAt: null }],
      }),
    ]);
    expect(pooled).toHaveLength(1);
    expect(pooled[0].usedPct).toBeCloseTo((20 * 50) / 21);
  });

  it("leaves out an account whose usage could not be read", () => {
    const pooled = pooledWindows([
      sub({
        weight: 5,
        windows: [{ label: "5-hour", usedPct: 40, resetsAt: null }],
      }),
      sub({ weight: 20, available: false, error: "HTTP 401" }),
    ]);
    expect(pooled[0].usedPct).toBe(40);
  });

  it("puts the weekly window first", () => {
    const pooled = pooledWindows([
      sub({
        windows: [
          { label: "5-hour", usedPct: 1, resetsAt: null },
          { label: "Weekly", usedPct: 2, resetsAt: null },
        ],
      }),
    ]);
    expect(pooled.map((w) => w.label)).toEqual(["Weekly", "5-hour"]);
  });
});

describe("groupByProvider", () => {
  it("keeps a provider's accounts together, in order", () => {
    const groups = groupByProvider([
      sub({ account: "a" }),
      sub({ cli: "codex", label: "ChatGPT" }),
      sub({ account: "b" }),
    ]);
    expect(groups.map((g) => g.map((s) => s.account ?? s.cli))).toEqual([
      ["a", "b"],
      ["codex"],
    ]);
  });
});
