/**
 * Scorers: workflow steps that grade their run (`scorer:` on a node), and
 * the grades the server recorded for them, across runs and workflows.
 */
import { useQuery } from "@tanstack/react-query";
import { apiJson } from "./api";

/** A scorer at a glance (mirrors `harness_persist::ScorerSummary`). */
export interface ScorerSummary {
  scorer: string;
  description: string | null;
  runs: number;
  passed: number;
  total_cost_usd: number;
  last_at: string;
  workflows: string[];
}

/** One day of grades. */
export interface ScorerDay {
  day: string;
  runs: number;
  passed: number;
  cost_usd: number;
}

/** One grade, with the run it graded. */
export interface ScorerResult {
  run_id: string;
  node_id: string;
  scorer: string;
  workflow: string;
  project: string | null;
  run_title: string | null;
  score: number;
  passed: boolean;
  classification: string | null;
  summary: string | null;
  duration_ms: number | null;
  cost_usd: number | null;
  created_at: string;
}

export interface ScorerDetail {
  summary: ScorerSummary;
  days: number;
  daily: ScorerDay[];
  runs: ScorerResult[];
}

export function useScorers() {
  return useQuery<ScorerSummary[], Error>({
    queryKey: ["scorers"],
    queryFn: async ({ signal }) =>
      (await apiJson<{ scorers: ScorerSummary[] }>("/api/scorers", { signal }))
        .scorers,
    retry: false,
  });
}

export function useScorer(name: string, days: number) {
  return useQuery<ScorerDetail, Error>({
    queryKey: ["scorers", name, days],
    enabled: !!name,
    queryFn: ({ signal }) =>
      apiJson<ScorerDetail>(
        `/api/scorers/${encodeURIComponent(name)}?days=${days}`,
        { signal },
      ),
    retry: false,
  });
}

/** A day on the trend: its pass rate (0–100) and cost; null where nothing ran. */
export interface TrendPoint {
  day: string;
  passRate: number | null;
  cost: number | null;
  runs: number;
}

/**
 * Every day of the last `days` up to `today` (YYYY-MM-DD, UTC), so the chart
 * shows the gaps: a day without a grade has no point, rather than a zero
 * that reads as "everything failed".
 */
export function trendPoints(
  daily: ScorerDay[],
  days: number,
  today: string,
): TrendPoint[] {
  const byDay = new Map(daily.map((d) => [d.day, d]));
  const end = Date.parse(`${today}T00:00:00Z`);
  const out: TrendPoint[] = [];
  for (let i = days - 1; i >= 0; i--) {
    const day = new Date(end - i * 86_400_000).toISOString().slice(0, 10);
    const d = byDay.get(day);
    out.push({
      day,
      passRate: d && d.runs > 0 ? Math.round((d.passed / d.runs) * 100) : null,
      cost: d ? d.cost_usd : null,
      runs: d?.runs ?? 0,
    });
  }
  return out;
}

/** "92%", or "—" with no runs. */
export function passRate(passed: number, runs: number): string {
  return runs > 0 ? `${Math.round((passed / runs) * 100)}%` : "—";
}
