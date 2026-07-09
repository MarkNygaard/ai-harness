/**
 * Generic workflow **report** verdict — the declarative counterpart to the
 * bespoke `lib/geo.ts` / `lib/review.ts`. Any workflow that declares
 * `ui.report` in its YAML gets a report tab rendered from a node's JSON output,
 * shaped as `{ summary?, score?, rating?, findings[] }`. Read-only for now
 * (no per-finding triage — that arrives when the finding-stores are unified).
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";
import { useWorkflowList } from "./authoring";
import type { WorkflowUi } from "@/types/authoring";
import type { NodeView } from "@/types/run";

export interface WorkflowFinding {
  title?: string;
  summary?: string;
  severity?: string;
  category?: string;
  detail?: string;
  fix?: string;
  /** Primary site, e.g. `folder/path:line`. */
  location?: string;
}

export interface WorkflowVerdict {
  summary?: string;
  /** Present for `scored` reports (GEO-style). */
  score?: number;
  rating?: string;
  findings: WorkflowFinding[];
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
 * to land the fix as a PR in the right repo/folder.
 */
export function findingTaskDescription(f: WorkflowFinding): string {
  const title = f.title ?? f.summary ?? "Finding";
  const tags = [f.category, f.severity].filter(Boolean).join(" / ");
  return [
    tags ? `Finding — ${tags}: ${title}` : `Finding: ${title}`,
    f.location ? `Location: ${f.location}` : "",
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
