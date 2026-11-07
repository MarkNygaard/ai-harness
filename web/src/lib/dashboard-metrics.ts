/**
 * The dashboard's metrics (`/api/dashboard/metrics`): how the harness has
 * been doing over a period, across projects or for one.
 */
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { apiJson } from "./api";

export interface Outcomes {
  completed: number;
  failed: number;
  cancelled: number;
}

/** Pull requests over a period (see the server's `pr_tracking`). */
export interface PrTotals {
  prs_opened: number;
  prs_merged: number;
  /** Median from a run starting to its PR opening. */
  median_to_pr_secs: number | null;
  /** Median from a PR opening to it merging. */
  median_to_merge_secs: number | null;
}

export interface DayMetrics extends Outcomes {
  /** UTC day, `YYYY-MM-DD`. */
  day: string;
  median_secs: number | null;
  cost_usd: number;
  prs_opened: number;
  prs_merged: number;
}

export interface WorkflowMetrics extends Outcomes {
  workflow: string;
  median_secs: number | null;
  cost_usd: number;
}

export interface Totals extends Outcomes, PrTotals {
  runs: number;
  /** Distinct tasks (runs sharing a title) with a completed run. */
  tasks: number;
  median_secs: number | null;
  cost_usd: number;
}

export interface Metrics {
  days: number;
  daily: DayMetrics[];
  workflows: WorkflowMetrics[];
  totals: Totals;
  previous: Totals;
}

/** The periods the dashboard offers. */
export const PERIODS = [7, 30, 90] as const;
export type Period = (typeof PERIODS)[number];

export function useDashboardMetrics(days: Period, project: string | null) {
  const qs = new URLSearchParams({ days: String(days) });
  if (project) qs.set("project", project);
  return useQuery<Metrics, Error>({
    queryKey: ["dashboard-metrics", days, project],
    queryFn: ({ signal }) =>
      apiJson<Metrics>(`/api/dashboard/metrics?${qs}`, { signal }),
    // A new filter keeps the old picture on screen while the new one loads,
    // rather than flashing empty charts.
    placeholderData: keepPreviousData,
    refetchInterval: 30_000,
  });
}

/** Share of finished runs that completed, 0–1; `null` with no runs. */
export function completionRate(t: Outcomes): number | null {
  const n = t.completed + t.failed + t.cancelled;
  return n === 0 ? null : t.completed / n;
}

/** `42s`, `14m`, `1.5h` — a run time a person reads at a glance. */
export function formatDuration(secs: number | null): string {
  if (secs == null) return "—";
  if (secs < 60) return `${Math.round(secs)}s`;
  if (secs < 3600) return `${Math.round(secs / 60)}m`;
  const h = secs / 3600;
  return `${h < 10 ? h.toFixed(1) : Math.round(h)}h`;
}

/** `$0.42`, `$12`, `$1.2K`. */
export function formatUsd(usd: number): string {
  if (usd < 10) return `$${usd.toFixed(2)}`;
  if (usd < 1000) return `$${Math.round(usd)}`;
  return `$${(usd / 1000).toFixed(1)}K`;
}

export function formatPercent(rate: number | null): string {
  return rate == null ? "—" : `${Math.round(rate * 100)}%`;
}

/**
 * The change from the previous period, worded and judged. `good` is whether
 * the change is in the direction that is better (null when neither is).
 */
export interface Delta {
  text: string;
  good: boolean | null;
}

export function delta(
  current: number | null,
  previous: number | null,
  opts: {
    /** Higher is better (`up`), lower is (`down`), or neither (`none`). */
    better: "up" | "down" | "none";
    /** `points`: an absolute change in percentage points, for rates. */
    kind?: "relative" | "points";
  },
): Delta | null {
  if (current == null || previous == null) return null;
  const diff = current - previous;
  if (opts.kind === "points") {
    const pts = Math.round(diff * 100);
    if (pts === 0) return { text: "no change", good: null };
    return {
      text: `${pts > 0 ? "+" : "−"}${Math.abs(pts)} pts`,
      good: opts.better === "none" ? null : pts > 0 === (opts.better === "up"),
    };
  }
  if (previous === 0)
    return current === 0 ? { text: "no change", good: null } : null;
  const pct = Math.round((diff / previous) * 100);
  if (pct === 0) return { text: "no change", good: null };
  return {
    text: `${pct > 0 ? "+" : "−"}${Math.abs(pct)}%`,
    good: opts.better === "none" ? null : pct > 0 === (opts.better === "up"),
  };
}

/** `Nov 7` for a `YYYY-MM-DD` UTC day. */
export function dayLabel(day: string): string {
  const d = new Date(`${day}T00:00:00Z`);
  return d.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    timeZone: "UTC",
  });
}
