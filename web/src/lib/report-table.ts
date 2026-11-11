/**
 * The arithmetic behind a `layout: table` report: which categories there are
 * and in what order, the totals for the tiles, and the filtered, sorted rows.
 * Kept apart from the component so it can be tested without rendering.
 */
import { SEVERITY_RANK, type WorkflowFinding } from "./report";

/** A category's tile: how many findings, and the time they account for. */
export interface CategoryTotal {
  category: string;
  count: number;
  /** Sum of `duration_ms`; null when no finding in it has one. */
  durationMs: number | null;
}

export type SortKey = "title" | "location" | "score" | "duration";
export interface Sort {
  key: SortKey;
  dir: "asc" | "desc";
}

/** A finding with no category is filed under this one. */
export const UNCATEGORIZED = "other";

export function categoryOf(f: WorkflowFinding): string {
  return f.category?.trim() || UNCATEGORIZED;
}

/**
 * One tile per category, most severe first: a category ranks by its most
 * severe finding, so "delete" (high) comes before "merge" (low) whatever the
 * names. Ties fall back to the name.
 */
export function categoryTotals(findings: WorkflowFinding[]): CategoryTotal[] {
  const rank = new Map<string, number>();
  const totals = new Map<string, CategoryTotal>();
  for (const f of findings) {
    const c = categoryOf(f);
    const r = SEVERITY_RANK[f.severity ?? ""] ?? 9;
    rank.set(c, Math.min(rank.get(c) ?? 9, r));
    const t = totals.get(c) ?? { category: c, count: 0, durationMs: null };
    t.count += 1;
    if (typeof f.duration_ms === "number") {
      t.durationMs = (t.durationMs ?? 0) + f.duration_ms;
    }
    totals.set(c, t);
  }
  return [...totals.values()].sort(
    (a, b) =>
      (rank.get(a.category) ?? 9) - (rank.get(b.category) ?? 9) ||
      a.category.localeCompare(b.category),
  );
}

/** Which optional columns any finding fills, so empty ones aren't drawn. */
export function columnsOf(findings: WorkflowFinding[]) {
  return {
    location: findings.some((f) => !!f.location),
    score: findings.some((f) => typeof f.score === "number"),
    duration: findings.some((f) => typeof f.duration_ms === "number"),
  };
}

/**
 * The rows to show: in `category` (null = all), matching every word of
 * `query` in the title, location or detail, sorted by `sort`. Rows without a
 * value for the sort column go last either way, so a sort never opens on a
 * page of blanks.
 */
export function visibleRows(
  findings: WorkflowFinding[],
  category: string | null,
  query: string,
  sort: Sort,
): WorkflowFinding[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const rows = findings.filter((f) => {
    if (category !== null && categoryOf(f) !== category) return false;
    if (words.length === 0) return true;
    const text = [f.title, f.summary, f.location, f.detail]
      .filter(Boolean)
      .join(" ")
      .toLowerCase();
    return words.every((w) => text.includes(w));
  });
  const value = (f: WorkflowFinding): string | number | undefined => {
    switch (sort.key) {
      case "title":
        return (f.title ?? f.summary ?? "").toLowerCase() || undefined;
      case "location":
        return f.location?.toLowerCase() || undefined;
      case "score":
        return typeof f.score === "number" ? f.score : undefined;
      case "duration":
        return typeof f.duration_ms === "number" ? f.duration_ms : undefined;
    }
  };
  const sign = sort.dir === "asc" ? 1 : -1;
  return [...rows].sort((a, b) => {
    const va = value(a);
    const vb = value(b);
    if (va === undefined || vb === undefined) {
      return va === vb ? 0 : va === undefined ? 1 : -1;
    }
    const c =
      typeof va === "number" && typeof vb === "number"
        ? va - vb
        : String(va).localeCompare(String(vb));
    return c * sign;
  });
}

/** The sort a table opens on: lowest score first where there are scores. */
export function defaultSort(findings: WorkflowFinding[]): Sort {
  return columnsOf(findings).score
    ? { key: "score", dir: "asc" }
    : { key: "title", dir: "asc" };
}

/** "12 ms", "3.4 s", "2 min 5 s". */
export function formatMs(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)} s`;
  const m = Math.floor(s / 60);
  return `${m} min ${Math.round(s - m * 60)} s`;
}
