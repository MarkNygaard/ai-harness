/**
 * Notional USD cost basis for token usage — a common dollar yardstick for
 * comparing runs, including subscription models that are not billed per token.
 *
 * The rates are **not written here.** `model-catalog.json` is generated from
 * `crates/harness-runner/src/models.rs`, which the editor's model list and the
 * server's own pricing also read, and a Rust test fails when the generated copy
 * goes stale. This file used to carry a second hand-maintained table with three
 * comments saying "mirrors token_usage.rs" — a comment doing a compiler's job,
 * and nothing that failed when the two drifted apart.
 */
import catalog from "./model-catalog.json";
import type { Usage } from "@/types/run";

interface Rates {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}

const FAMILIES = new Map<string, Rates>(
  catalog.families.map((f) => [
    f.id,
    {
      input: f.rates.input,
      output: f.rates.output,
      cacheRead: f.rates.cache_read,
      cacheWrite: f.rates.cache_write,
    },
  ]),
);

const BY_ID = new Map<string, string>(
  catalog.models.map((m) => [m.id.toLowerCase(), m.family]),
);

function family(id: string): Rates {
  const rates = FAMILIES.get(id);
  if (!rates) throw new Error(`no pricing family "${id}"`);
  return rates;
}

/**
 * Per-MTok rates for a model: exact when it is one the harness lists, and by
 * the id's shape when it is not.
 *
 * Any model string is accepted — a workflow can pin an id nobody listed — so
 * this always answers. The fallback order comes from the generated table rather
 * than being re-derived here, because the order is the subtle part: `gpt-6` and
 * the 5.6 tiers have to be reached before the general `gpt-5` arm that also
 * matches them, and those tiers are 25x apart.
 */
export function ratesFor(model: string): Rates {
  const m = model.toLowerCase();
  const exact = BY_ID.get(m);
  if (exact) return family(exact);
  const hit = catalog.fallbacks.find(([needle]) => m.includes(needle));
  return family(hit ? hit[1] : catalog.default_family);
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
