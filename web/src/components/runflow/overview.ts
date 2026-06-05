import type { NodeStatus, NodeView, Usage } from "@/types/run";
import type { Category } from "@/lib/categories";
import { elapsedMs, statusColor, sumUsage, totalTokens } from "./format";

/**
 * The colour for a node's bars: its category colour when the node has a known
 * category, else the semantic status colour (the confirmed fallback).
 */
export function nodeColor(
  status: NodeStatus,
  category: string | null,
  colors: Map<string, string>,
): string {
  if (category) {
    const c = colors.get(category);
    if (c) return c;
  }
  return statusColor(status);
}

/** A category's share of wall-clock time, for the time-by-category bar. */
export interface CategorySegment {
  id: string;
  label: string;
  color: string;
  ms: number;
}

/**
 * Sum each step's duration by its category (uncategorized steps are excluded —
 * they have no category to attribute time to), ordered by the registry's
 * ordinal. An unknown category id falls back to its id + a neutral colour.
 */
export function timeByCategory(
  nodes: NodeView[],
  now: number,
  categories: Category[],
): CategorySegment[] {
  const meta = new Map(
    categories.map((c, i) => [
      c.id,
      { label: c.label, color: c.color, ord: c.ordinal ?? i },
    ]),
  );
  const ms = new Map<string, number>();
  for (const n of nodes) {
    if (!n.category) continue;
    const d = elapsedMs(n.started_at, n.ended_at, now);
    if (d == null) continue;
    ms.set(n.category, (ms.get(n.category) ?? 0) + d);
  }
  return [...ms.entries()]
    .map(([id, total]) => {
      const m = meta.get(id);
      return {
        id,
        label: m?.label ?? id,
        color: m?.color ?? "var(--status-skipped)",
        ms: total,
        ord: m?.ord ?? 999,
      };
    })
    .sort((a, b) => a.ord - b.ord || b.ms - a.ms)
    .map(({ ord: _ord, ...seg }) => seg);
}

/** Aggregate token usage grouped by model (falling back to provider). */
export function usageByModel(
  nodes: NodeView[],
): Array<{ key: string; steps: number; usage: Usage; total: number }> {
  const groups = new Map<string, NodeView[]>();
  for (const n of nodes) {
    if (totalTokens(n.usage) === 0 && n.usage.cache_read == null) continue;
    const key = n.model ?? n.provider ?? "unknown";
    const list = groups.get(key) ?? [];
    list.push(n);
    groups.set(key, list);
  }
  return [...groups.entries()]
    .map(([key, list]) => {
      const usage = sumUsage(list.map((n) => n.usage));
      return { key, steps: list.length, usage, total: totalTokens(usage) };
    })
    .sort((a, b) => b.total - a.total);
}

/** A token category for the stacked "by type" breakdown bar. */
export interface TokenSegment {
  key: "input" | "cache_read" | "cache_write" | "output";
  label: string;
  value: number;
  /** CSS color token. */
  color: string;
}

/** Break a summed usage into ordered, non-zero segments for a stacked bar. */
export function usageByType(usage: Usage): TokenSegment[] {
  // Muted, Factory-style palette (teal / slate / peach / olive) — deliberately
  // desaturated and independent of the semantic status colors.
  const defs: Array<Omit<TokenSegment, "value">> = [
    { key: "input", label: "Input", color: "oklch(0.64 0.07 200)" },
    { key: "cache_read", label: "Cache read", color: "oklch(0.68 0.03 250)" },
    { key: "cache_write", label: "Cache write", color: "oklch(0.76 0.09 70)" },
    { key: "output", label: "Output", color: "oklch(0.75 0.09 130)" },
  ];
  return defs
    .map((d) => ({ ...d, value: usage[d.key] ?? 0 }))
    .filter((s) => s.value > 0);
}

/** Earliest start / latest end across all timed nodes (ms epoch). */
export interface RunWindow {
  startMs: number;
  endMs: number;
  spanMs: number;
}

export function runWindow(nodes: NodeView[], now: number): RunWindow | null {
  let startMs = Infinity;
  let endMs = -Infinity;
  for (const n of nodes) {
    if (!n.started_at) continue;
    const s = Date.parse(n.started_at);
    if (Number.isNaN(s)) continue;
    const e = n.ended_at ? Date.parse(n.ended_at) : now;
    startMs = Math.min(startMs, s);
    endMs = Math.max(endMs, Number.isNaN(e) ? s : e);
  }
  if (!Number.isFinite(startMs) || !Number.isFinite(endMs)) return null;
  return { startMs, endMs, spanMs: Math.max(1, endMs - startMs) };
}

/** A positioned bar on the milestone waterfall timeline. */
export interface WaterfallRow {
  id: string;
  status: NodeView["status"];
  category: string | null;
  durationMs: number | null;
  /** Left offset as a fraction [0,1] of the run window. */
  offset: number;
  /** Width as a fraction [0,1] of the run window. */
  width: number;
}

/**
 * Lay timed nodes out as a Gantt waterfall over the run window, sorted by start.
 * Nodes that never started are omitted (nothing to place on the timeline).
 */
export function waterfall(nodes: NodeView[], now: number): WaterfallRow[] {
  const win = runWindow(nodes, now);
  if (!win) return [];
  return nodes
    .filter((n) => n.started_at && !Number.isNaN(Date.parse(n.started_at)))
    .map((n) => {
      const start = Date.parse(n.started_at!);
      const dur = elapsedMs(n.started_at, n.ended_at, now) ?? 0;
      return {
        id: n.id,
        status: n.status,
        category: n.category,
        durationMs: elapsedMs(n.started_at, n.ended_at, now),
        offset: (start - win.startMs) / win.spanMs,
        width: Math.max(dur / win.spanMs, 0.004),
      };
    })
    .sort((a, b) => a.offset - b.offset);
}

/** Per-step wall-clock durations, sorted longest-first (for the time bars). */
export interface StepTime {
  id: string;
  status: NodeView["status"];
  category: string | null;
  durationMs: number;
}

export function timeByStep(nodes: NodeView[], now: number): StepTime[] {
  return nodes
    .map((n) => ({
      id: n.id,
      status: n.status,
      category: n.category,
      durationMs: elapsedMs(n.started_at, n.ended_at, now),
    }))
    .filter((r): r is StepTime => r.durationMs != null)
    .sort((a, b) => b.durationMs - a.durationMs);
}
