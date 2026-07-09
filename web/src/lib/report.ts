/**
 * Generic workflow **report** verdict — the single report model behind every
 * `ui.report` workflow (GEO audit, review, and any custom one). Rendered from a
 * node's JSON output shaped as `{ summary?, score?, rating?, categories?[],
 * findings[] }`. `scored` reports (e.g. GEO) additionally show a score, a
 * per-dimension `categories` breakdown, and a score-history sparkline.
 */
import {
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { apiJson } from "./api";
import { useWorkflowList } from "./authoring";
import { nodesFromDetail, useRuns } from "./runs";
import type { WorkflowUi } from "@/types/authoring";
import type { NodeView, RunDetail } from "@/types/run";

export interface WorkflowFinding {
  title?: string;
  summary?: string;
  severity?: string;
  category?: string;
  detail?: string;
  fix?: string;
  /** Primary site, e.g. `folder/path:line`. */
  location?: string;
  /** Relative effort to address (e.g. `quick` | `medium` | `strategic`). */
  effort?: string;
}

/** A per-dimension score in a `scored` verdict (GEO-style breakdown). */
export interface WorkflowCategory {
  key: string;
  score: number;
  weight?: number;
  summary?: string;
}

export interface WorkflowVerdict {
  summary?: string;
  /** Present for `scored` reports (GEO-style). */
  score?: number;
  rating?: string;
  /** Per-dimension scores, shown as bars in a `scored` report. */
  categories?: WorkflowCategory[];
  findings: WorkflowFinding[];
}

/** Rating band (0–100 score) → semantic colour token. */
export function ratingColor(score: number): string {
  if (score >= 75) return "var(--status-success)";
  if (score >= 60) return "var(--accent-orange)";
  if (score >= 40) return "var(--status-running)";
  return "var(--status-failed)";
}

/** Severity order (worst first) for known levels; unknown severities sort last. */
export const SEVERITY_RANK: Record<string, number> = {
  critical: 0,
  high: 1,
  medium: 2,
  low: 3,
  info: 4,
};

/**
 * Best-effort JSON parse of an agent's output: direct parse, else the outermost
 * `{…}` span (agents sometimes wrap JSON in prose / fences). Mirrors the Rust
 * `extract_json` leniency and the geo/review parsers.
 */
function extractJson(raw: string): unknown {
  const t = raw.trim();
  try {
    return JSON.parse(t);
  } catch {
    const s = t.indexOf("{");
    const e = t.lastIndexOf("}");
    if (s >= 0 && e > s) {
      try {
        return JSON.parse(t.slice(s, e + 1));
      } catch {
        return null;
      }
    }
    return null;
  }
}

/** A verdict from a node's output, or null if its shape isn't a verdict. */
function asVerdict(output: string | undefined): WorkflowVerdict | null {
  if (!output) return null;
  const v = extractJson(output) as Record<string, unknown> | null;
  if (!v || !Array.isArray(v.findings)) return null;
  return {
    summary: typeof v.summary === "string" ? v.summary : undefined,
    score: typeof v.score === "number" ? v.score : undefined,
    rating: typeof v.rating === "string" ? v.rating : undefined,
    categories: Array.isArray(v.categories)
      ? (v.categories as WorkflowCategory[])
      : undefined,
    findings: v.findings as WorkflowFinding[],
  };
}

/**
 * The verdict from a run. Prefers the declared `verdictNode`, then falls back to
 * scanning any node whose output parses to a verdict — so the report is
 * decoupled from exact node naming.
 */
export function parseWorkflowVerdict(
  nodes: NodeView[],
  verdictNode?: string | null,
): WorkflowVerdict | null {
  if (verdictNode) {
    const primary = asVerdict(nodes.find((n) => n.id === verdictNode)?.output);
    if (primary) return primary;
  }
  for (const n of nodes) {
    const v = asVerdict(n.output);
    if (v) return v;
  }
  return null;
}

/** The declared `ui` block for a workflow, from the cached authoring list. */
export function useWorkflowUi(
  name: string | null | undefined,
): WorkflowUi | null {
  const list = useWorkflowList();
  if (!name) return null;
  return list.data?.find((w) => w.name === name)?.ui ?? null;
}

/** Stable key for a finding within a run's report (`category::title`). */
export function findingKey(f: WorkflowFinding): string {
  const title = f.title ?? f.summary ?? "";
  return `${f.category ?? ""}::${title}`;
}

/**
 * The `idea-to-pr` task description for a finding — enough for the implementer
 * to land the fix as a PR in the right repo/folder. `externalUrl` (a project's
 * live-site URL) is added as context when present (e.g. GEO-audit findings).
 */
export function findingTaskDescription(
  f: WorkflowFinding,
  externalUrl?: string | null,
): string {
  const title = f.title ?? f.summary ?? "Finding";
  const tags = [f.category, f.severity].filter(Boolean).join(" / ");
  return [
    tags ? `Finding — ${tags}: ${title}` : `Finding: ${title}`,
    f.location ? `Location: ${f.location}` : "",
    externalUrl ? `Live site: ${externalUrl}` : "",
    "",
    f.detail ?? "",
    "",
    f.fix ? `Fix: ${f.fix}` : "",
    "",
    "Implement this fix in the project's source and open a PR. In a multi-repo " +
      "project, make the change in the repo/folder named in the location above. " +
      "Keep the change focused on this finding.",
  ]
    .filter((l) => l !== "")
    .join("\n")
    .trim();
}

/** One report's score at a point in time, for the score-history sparkline. */
export interface ScorePoint {
  runId: string;
  at: string;
  score: number;
}

const MAX_HISTORY = 12;

/**
 * A workflow's score over time for a project — derived from its past runs (no
 * new storage): fetch each run's detail and read its verdict score. Points are
 * oldest → newest so the audit → fix → re-audit loop is visible. Generic over
 * the workflow (the counterpart to the former `useGeoHistory`).
 */
export function useReportHistory(
  workflow: string | null,
  project: string | null,
  verdictNode?: string | null,
): { points: ScorePoint[]; loading: boolean } {
  const runs = useRuns({ project: project ?? undefined });
  const scoredRuns = (runs.data ?? [])
    .filter((r) => r.workflow_name === workflow)
    .slice(0, MAX_HISTORY); // runs arrive newest-first
  const details = useQueries({
    queries: scoredRuns.map((r) => ({
      queryKey: ["run", r.id],
      queryFn: ({ signal }: { signal: AbortSignal }) =>
        apiJson<RunDetail>(`/api/runs/${r.id}`, { signal }),
      staleTime: 60_000,
    })),
  });
  const points: ScorePoint[] = [];
  details.forEach((d, i) => {
    if (!d.data) return;
    const v = parseWorkflowVerdict(nodesFromDetail(d.data), verdictNode);
    if (v && typeof v.score === "number") {
      points.push({
        runId: scoredRuns[i].id,
        at: scoredRuns[i].recorded_at,
        score: v.score,
      });
    }
  });
  points.sort((a, b) => Date.parse(a.at) - Date.parse(b.at));
  return {
    points,
    loading: runs.isLoading || details.some((d) => d.isLoading),
  };
}

// ── Per-finding triage state (persisted server-side, keyed by run) ───────────

/** What the user did with a finding in the report. */
export type FindingAction = "built" | "issued" | "ignored";

/** A remembered finding action (mirrors the server's `FindingState`). */
export interface FindingState {
  finding_key: string;
  action: FindingAction;
  ref_run_id: string | null;
  issue_identifier: string | null;
  issue_url: string | null;
}

/** Body for recording a finding's state. */
export interface SetFindingState {
  finding_key: string;
  action: FindingAction;
  ref_run_id?: string;
  issue_identifier?: string;
  issue_url?: string;
}

/** Remembered finding states for a report run, keyed by `finding_key`. */
export function useFindingStates(runId: string | null) {
  return useQuery<FindingState[], Error>({
    queryKey: ["findings", runId],
    enabled: !!runId,
    queryFn: ({ signal }) =>
      apiJson<FindingState[]>(
        `/api/runs/${encodeURIComponent(runId!)}/findings`,
        { signal },
      ),
  });
}

/** Record a finding's state (Build this / Create issue / Ignore). */
export function useSetFindingState(runId: string | null) {
  const qc = useQueryClient();
  return useMutation<FindingState, Error, SetFindingState>({
    mutationFn: (body) =>
      apiJson<FindingState>(
        `/api/runs/${encodeURIComponent(runId!)}/findings`,
        {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        },
      ),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["findings", runId] }),
  });
}

/** Forget a finding's state — the "Rebuild" / "Unignore" action. */
export function useClearFindingState(runId: string | null) {
  const qc = useQueryClient();
  return useMutation<unknown, Error, string>({
    mutationFn: (key) =>
      apiJson(
        `/api/runs/${encodeURIComponent(runId!)}/findings?key=${encodeURIComponent(key)}`,
        { method: "DELETE" },
      ),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["findings", runId] }),
  });
}
