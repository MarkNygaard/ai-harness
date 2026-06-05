import type { NodeView, Usage } from "@/types/run";
import { elapsedMs, sumUsage, totalTokens } from "./format";

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
  const defs: Array<Omit<TokenSegment, "value">> = [
    {
      key: "input",
      label: "Input",
      color: "var(--accent-blue, var(--primary))",
    },
    { key: "cache_read", label: "Cache read", color: "var(--status-skipped)" },
    { key: "cache_write", label: "Cache write", color: "var(--accent-orange)" },
    { key: "output", label: "Output", color: "var(--status-success)" },
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
        durationMs: elapsedMs(n.started_at, n.ended_at, now),
        offset: (start - win.startMs) / win.spanMs,
        width: Math.max(dur / win.spanMs, 0.004),
      };
    })
    .sort((a, b) => a.offset - b.offset);
}

/** Per-step wall-clock durations, sorted longest-first (for the time bars). */
export function timeByStep(
  nodes: NodeView[],
  now: number,
): Array<{ id: string; status: NodeView["status"]; durationMs: number }> {
  return nodes
    .map((n) => ({
      id: n.id,
      status: n.status,
      durationMs: elapsedMs(n.started_at, n.ended_at, now),
    }))
    .filter(
      (
        r,
      ): r is { id: string; status: NodeView["status"]; durationMs: number } =>
        r.durationMs != null,
    )
    .sort((a, b) => b.durationMs - a.durationMs);
}
