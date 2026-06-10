import { describe, expect, it } from "vitest";
import { formatCost, usageCost } from "./cost";
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
    expect(out("claude-sonnet-4-6")).toBeCloseTo(15, 9);
    expect(out("claude-haiku-4-5")).toBeCloseTo(5, 9);
    expect(out("claude-fable-5")).toBeCloseTo(50, 9);
    expect(out("openai-codex/gpt-5.5")).toBeCloseTo(30, 9);
    expect(out("kimi-code/kimi-for-coding")).toBeCloseTo(4, 9);
    // Unknown → Sonnet-tier fallback (matches the server).
    expect(out("some-future-model")).toBeCloseTo(15, 9);
  });

  it("splits input 10/90 into input/cache-read when no cache breakdown", () => {
    // Sonnet: 1M input, no cache → 0.1M*$3 + 0.9M*$0.30 = 0.30 + 0.27 = 0.57.
    expect(usageCost("claude-sonnet-4-6", u({ input: 1_000_000 }))).toBeCloseTo(
      0.57,
      9,
    );
  });

  it("prices explicit cache tokens directly when present", () => {
    // Sonnet: 1M input + 1M cache_read = 1*$3 + 1*$0.30 = $3.30 (no heuristic).
    const cost = usageCost(
      "claude-sonnet-4-6",
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
