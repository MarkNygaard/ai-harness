/**
 * Notional USD cost basis for token usage — a common dollar yardstick for
 * comparing runs (including subscription models that aren't billed per-token).
 *
 * ⚠️ Keep this price table in sync with the server's authoritative one in
 * `crates/harness-server/src/handlers/token_usage.rs` (`rates_for_model` /
 * `record_cost`). Same families, same per-MTok rates, same no-cache-breakdown
 * heuristic. This is a client-side mirror so the run overview can price per-node
 * usage without a round trip; the server endpoint remains the source of truth.
 */
import type { Usage } from "@/types/run";

interface Rates {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}

/** Per-MTok USD rates by model family, matched on a lowercased substring. */
function ratesFor(model: string): Rates {
  const m = model.toLowerCase();
  if (m.includes("opus"))
    return { input: 5, output: 25, cacheRead: 0.5, cacheWrite: 6.25 };
  if (m.includes("haiku"))
    return { input: 1, output: 5, cacheRead: 0.1, cacheWrite: 1.25 };
  if (m.includes("fable"))
    return { input: 10, output: 50, cacheRead: 1, cacheWrite: 12.5 };
  if (m.includes("sonnet"))
    return { input: 3, output: 15, cacheRead: 0.3, cacheWrite: 3.75 };
  if (m.includes("gpt-5") || m.includes("codex") || m.includes("openai"))
    return { input: 5, output: 30, cacheRead: 0.5, cacheWrite: 5 };
  if (m.includes("kimi") || m.includes("moonshot"))
    return { input: 0.95, output: 4, cacheRead: 0.16, cacheWrite: 0.95 };
  if (m.includes("composer"))
    // Cursor Composer 2.5 standard tier: $0.50 in / $2.50 out / $0.20 cache-read
    // (published). cache_read dominates a coding run, so this is the figure that
    // reconciles notional cost with Cursor's usage dashboard. No write-cache rate
    // is published; keep input-rate as a safe upper bound. (mirrors token_usage.rs)
    return { input: 0.5, output: 2.5, cacheRead: 0.2, cacheWrite: 0.5 };
  // Unknown model → Sonnet-tier fallback (matches the server).
  return { input: 3, output: 15, cacheRead: 0.3, cacheWrite: 3.75 };
}

/**
 * Notional cost of one usage bucket, priced at `model`'s rate. When no cache
 * breakdown is present (cache_read/cache_write both zero — older Claude builds
 * and Codex report all context as input), assume a 90% cache-read hit rate,
 * matching the server's per-record heuristic.
 */
export function usageCost(model: string | null, usage: Usage): number {
  const r = ratesFor(model ?? "");
  const input = usage.input ?? 0;
  const output = usage.output ?? 0;
  const cacheRead = usage.cache_read ?? 0;
  const cacheWrite = usage.cache_write ?? 0;
  const hasCache = cacheRead > 0 || cacheWrite > 0;
  const effInput = hasCache ? input : input * 0.1;
  const effCacheRead = hasCache ? cacheRead : input * 0.9;
  return (
    (effInput / 1e6) * r.input +
    (output / 1e6) * r.output +
    (effCacheRead / 1e6) * r.cacheRead +
    (cacheWrite / 1e6) * r.cacheWrite
  );
}

/** Compact USD label — more decimals for sub-cent amounts. `null` → "—". */
export function formatCost(usd: number | null): string {
  if (usd == null) return "—";
  if (usd === 0) return "$0";
  if (usd < 0.01) return `$${usd.toFixed(4)}`;
  if (usd < 1) return `$${usd.toFixed(3)}`;
  return `$${usd.toFixed(2)}`;
}
