import type { RunDailyCount } from "@/types/run";

export const NO_PROJECT = "(unassigned)";

/** UTC date key (YYYY-MM-DD) for an ISO timestamp. */
export function utcDayKey(iso: string): string {
  return iso.slice(0, 10);
}

/** UTC date keys for the last `n` days, newest first, given `now`. */
export function recentDayKeys(now: Date, n: number): string[] {
  const keys: string[] = [];
  for (let i = 0; i < n; i++) {
    const d = new Date(
      Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate() - i),
    );
    keys.push(d.toISOString().slice(0, 10));
  }
  return keys;
}

export interface DayBucket {
  completed: number;
  failed: number;
} // failed folds in cancelled

export interface ProjectSummary {
  project: string; // NO_PROJECT when null/empty
  total: number; // completed across the window
  byDay: Record<string, DayBucket>; // keyed by UTC day key
}

/** Group rows → per-project, per-day buckets. Sorted by total completed desc. */
export function buildProjectSummaries(rows: RunDailyCount[]): ProjectSummary[] {
  const map = new Map<string, ProjectSummary>();
  for (const r of rows) {
    const project = r.project?.trim() || NO_PROJECT;
    const key = utcDayKey(r.day);
    const ps = map.get(project) ?? { project, total: 0, byDay: {} };
    const b = ps.byDay[key] ?? { completed: 0, failed: 0 };
    if (r.status === "completed") {
      b.completed += r.count;
      ps.total += r.count;
    } else {
      b.failed += r.count;
    } // failed + cancelled → red bucket
    ps.byDay[key] = b;
    map.set(project, ps);
  }
  return [...map.values()].sort(
    (a, b) => b.total - a.total || a.project.localeCompare(b.project),
  );
}
