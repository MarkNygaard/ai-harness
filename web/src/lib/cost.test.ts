import { describe, expect, it } from "vitest";
import { formatCost, ratesFor, usageCost } from "./cost";
import type { Usage } from "@/types/run";

const u = (p: Partial<Usage>): Usage => ({
  input: null,
  output: null,
  cache_read: null,
  cache_write: null,
  ...p,
});

describe("usageCost", () => {
  it("prices 1M output tokens at each family's output rate", () => {
    const out = (model: string) => usageCost(model, u({ output: 1_000_000 }));
    expect(out("claude-opus-4-8")).toBeCloseTo(25, 9);
    expect(out("claude-sonnet-5")).toBeCloseTo(15, 9);
    expect(out("claude-haiku-4-5")).toBeCloseTo(5, 9);
    expect(out("claude-fable-5")).toBeCloseTo(50, 9);
    expect(out("openai-codex/gpt-5.5")).toBeCloseTo(30, 9);
    expect(out("gpt-6-astra")).toBeCloseTo(50, 9);
    expect(out("openai-codex/gpt-6-astra")).toBeCloseTo(50, 9);
    // The 5.6 tiers price apart; Sol falls to the generic gpt-5 rate.
    expect(out("openai-codex/gpt-5.6-sol")).toBeCloseTo(30, 9);
    expect(out("openai-codex/gpt-5.6-terra")).toBeCloseTo(12, 9);
    expect(out("gpt-5.6-luna")).toBeCloseTo(1.2, 9);
    expect(out("kimi-code/kimi-for-coding")).toBeCloseTo(4, 9);
    expect(out("composer-2.5")).toBeCloseTo(2.5, 9);
    // Unknown → Sonnet-tier fallback (matches the server).
    expect(out("some-future-model")).toBeCloseTo(15, 9);
  });

  it("splits input 10/90 into input/cache-read when no cache breakdown", () => {
    // Sonnet: 1M input, no cache → 0.1M*$3 + 0.9M*$0.30 = 0.30 + 0.27 = 0.57.
    expect(usageCost("claude-sonnet-5", u({ input: 1_000_000 }))).toBeCloseTo(
      0.57,
      9,
    );
  });

  it("prices explicit cache tokens directly when present", () => {
    // Sonnet: 1M input + 1M cache_read = 1*$3 + 1*$0.30 = $3.30 (no heuristic).
    const cost = usageCost(
      "claude-sonnet-5",
      u({ input: 1_000_000, cache_read: 1_000_000 }),
    );
    expect(cost).toBeCloseTo(3.3, 9);
  });
});

describe("formatCost", () => {
  it("scales precision and handles null/zero", () => {
    expect(formatCost(null)).toBe("—");
    expect(formatCost(0)).toBe("$0");
    expect(formatCost(0.0001234)).toBe("$0.0001");
    expect(formatCost(0.123)).toBe("$0.123");
    expect(formatCost(12.345)).toBe("$12.35");
  });
});

describe("the generated catalog", () => {
  /**
   * The rates come from `model-catalog.json`, generated from the Rust table a
   * Rust test keeps in step. These pin the reading of it — that an id the
   * harness lists resolves exactly, and that one it does not still prices by
   * shape rather than falling to zero.
   */
  it("prices a listed model exactly", () => {
    // Cursor's versioned id and Claude Code's alias are the same model, so the
    // same price — an A/B across the two agents measures the models, not us.
    expect(ratesFor("claude-sonnet-5")).toEqual(ratesFor("sonnet"));
    expect(ratesFor("openai-codex/gpt-6-astra")).toEqual(
      ratesFor("gpt-6-astra"),
    );
  });

  it("keeps the tiers apart, which is what the fallback order is for", () => {
    // All three contain "gpt-5"; reached in the wrong order they would collapse
    // onto one rate, and they are 25x apart end to end.
    expect(ratesFor("gpt-5.6-luna").output).toBeLessThan(
      ratesFor("gpt-5.6-terra").output,
    );
    expect(ratesFor("gpt-5.6-terra").output).toBeLessThan(
      ratesFor("gpt-5.6-sol").output,
    );
    // And "gpt-6-astra" must not be read as a gpt-5.
    expect(ratesFor("gpt-6-astra").output).toBeGreaterThan(
      ratesFor("gpt-5.6-sol").output,
    );
  });

  it("prices an unlisted model rather than treating it as free", () => {
    // A version nobody has listed yet still resolves by shape.
    expect(ratesFor("claude-opus-4.8")).toEqual(ratesFor("opus"));
    // And something entirely unknown falls to the default tier, not to zero,
    // which would read as a run that cost nothing.
    const unknown = ratesFor("who-knows-1");
    expect(unknown.input).toBeGreaterThan(0);
    expect(unknown).toEqual(ratesFor("sonnet"));
  });
});
