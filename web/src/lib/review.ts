/**
 * Review verdict: the structured output the `review-area` workflow's
 * `deep-review` node emits (a summary + severity-tagged findings, each with a
 * concrete fix phrased as an implementer task). Parsed from the run's node
 * output so the report view can render the findings and turn each into an
 * `idea-to-pr` task or a Linear issue. Mirror of `lib/geo.ts`, minus the score.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";
import type { NodeView } from "@/types/run";

export type ReviewSeverity = "critical" | "high" | "medium" | "low";
export type ReviewEffort = "quick" | "medium" | "strategic";

export interface ReviewFinding {
  severity: ReviewSeverity;
  category: string;
  title: string;
  detail?: string;
  fix: string;
  /** Primary site, `folder/path:line`. */
  location?: string;
  effort?: ReviewEffort;
}

export interface ReviewVerdict {
  summary?: string;
  findings: ReviewFinding[];
}

/** Severity order (worst first) for sorting + display. */
export const SEVERITY_RANK: Record<ReviewSeverity, number> = {
  critical: 0,
  high: 1,
  medium: 2,
  low: 3,
};

/**
 * Best-effort JSON parse of an agent's output: direct parse, else the outermost
 * `{…}` span (agents sometimes wrap JSON in prose / fences). Mirrors the Rust
 * `extract_json` leniency.
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

/** The review verdict from a node's output, or null if it isn't one. */
function asVerdict(output: string | undefined): ReviewVerdict | null {
  if (!output) return null;
  const v = extractJson(output) as Partial<ReviewVerdict> | null;
  if (v && Array.isArray(v.findings)) {
    return { summary: v.summary, findings: v.findings as ReviewFinding[] };
  }
  return null;
}

/**
 * The review verdict from a run. Prefers the `deep-review` node, then falls back
 * to scanning any node whose output parses to a verdict — so the view is
 * decoupled from the workflow's node naming.
 */
export function parseReviewVerdict(nodes: NodeView[]): ReviewVerdict | null {
  const primary = asVerdict(nodes.find((n) => n.id === "deep-review")?.output);
  if (primary) return primary;
  for (const n of nodes) {
    const v = asVerdict(n.output);
    if (v) return v;
  }
  return null;
}

/**
 * Compose the `idea-to-pr` task description for a finding — enough for the
 * implementer to land the fix as a PR in the right repo/folder.
 */
export function reviewTaskDescription(f: ReviewFinding): string {
  return [
    `Code-review finding — ${f.category} / ${f.severity}: ${f.title}`,
    f.location ? `Location: ${f.location}` : "",
    "",
    f.detail ?? "",
    "",
    `Fix: ${f.fix}`,
    "",
    "Implement this fix in the project's source and open a PR. In a multi-repo " +
      "project, make the change in the repo/folder named in the location above. " +
      "Keep the change focused on this finding.",
  ]
    .filter((l) => l !== "")
    .join("\n")
    .trim();
}

/** Stable key for a finding within a run's report (`category::title`). */
export function findingKey(f: ReviewFinding): string {
  return `${f.category}::${f.title}`;
}

// ── Per-finding triage state (persisted server-side, keyed by review run) ────

/** What the user did with a finding in the report. */
export type ReviewFindingAction = "built" | "issued" | "ignored";

/** A remembered finding action (mirrors the server's `ReviewFindingState`). */
export interface ReviewFindingState {
  finding_key: string;
  action: ReviewFindingAction;
  ref_run_id: string | null;
  issue_identifier: string | null;
  issue_url: string | null;
}

/** Body for recording a finding's state. */
export interface SetReviewFindingState {
  finding_key: string;
  action: ReviewFindingAction;
  ref_run_id?: string;
  issue_identifier?: string;
  issue_url?: string;
}

/** Remembered finding states for a review run, keyed by `finding_key`. */
export function useReviewFindingStates(runId: string | null) {
  return useQuery<ReviewFindingState[], Error>({
    queryKey: ["review-findings", runId],
    enabled: !!runId,
    queryFn: ({ signal }) =>
      apiJson<ReviewFindingState[]>(
        `/api/runs/${encodeURIComponent(runId!)}/review-findings`,
        { signal },
      ),
  });
}

/** Record a finding's state (Build this / Create issue / Ignore). */
export function useSetReviewFindingState(runId: string | null) {
  const qc = useQueryClient();
  return useMutation<ReviewFindingState, Error, SetReviewFindingState>({
    mutationFn: (body) =>
      apiJson<ReviewFindingState>(
        `/api/runs/${encodeURIComponent(runId!)}/review-findings`,
        {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        },
      ),
    onSuccess: () =>
      qc.invalidateQueries({ queryKey: ["review-findings", runId] }),
  });
}

/** Forget a finding's state — the "Rebuild" / "Unignore" action. */
export function useClearReviewFindingState(runId: string | null) {
  const qc = useQueryClient();
  return useMutation<unknown, Error, string>({
    mutationFn: (key) =>
      apiJson(
        `/api/runs/${encodeURIComponent(runId!)}/review-findings?key=${encodeURIComponent(key)}`,
        { method: "DELETE" },
      ),
    onSuccess: () =>
      qc.invalidateQueries({ queryKey: ["review-findings", runId] }),
  });
}
