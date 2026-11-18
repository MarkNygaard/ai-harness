/**
 * Improvement suggestions (`/api/improvements`): what a workflow's recent runs
 * in a project suggest changing, and where each opted-in workflow stands
 * towards its next review.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";

export type SuggestionStatus = "new" | "applied" | "dismissed";

/** One run that shows the problem (mirrors the server's evidence entries). */
export interface SuggestionEvidence {
  run_id: string;
  node?: string | null;
  note: string;
}

/** A suggestion (mirrors `harness_persist::ImprovementSuggestion`). */
export interface ImprovementSuggestion {
  id: string;
  review_id: string;
  workflow: string;
  project: string;
  /** `repo`: a change to the project's repo, made by a run as a PR.
   *  `workflow`: a change to the workflow, made in the editor. */
  target: "repo" | "workflow" | string;
  category: string;
  /** The step it is about, when it is about one. */
  node: string | null;
  title: string;
  problem: string;
  change: string;
  evidence: SuggestionEvidence[];
  status: SuggestionStatus;
  /** The run that made the change, for a repo suggestion applied from here. */
  applied_run_id: string | null;
  /** How many reviews have raised it. */
  seen_count: number;
  last_seen_at: string;
  created_at: string;
  updated_at: string;
}

/** One review of a workflow's runs in a project. */
export interface ImprovementReview {
  id: string;
  workflow: string;
  project: string;
  run_id: string | null;
  run_ids: string[];
  status: string;
  summary: string | null;
  error: string | null;
  created_at: string;
  finished_at: string | null;
}

/** Where an opted-in workflow stands in one project. */
export interface WorkflowStanding {
  workflow: string;
  project: string;
  /** A review every this many finished runs. */
  every: number;
  runs_since_review: number;
  review: ImprovementReview | null;
}

export interface ImprovementsFilter {
  workflow?: string;
  project?: string;
  status?: SuggestionStatus;
}

export interface ImprovementsData {
  suggestions: ImprovementSuggestion[];
  workflows: WorkflowStanding[];
}

function query(filter: ImprovementsFilter): string {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(filter)) if (v) q.set(k, v);
  const s = q.toString();
  return s ? `?${s}` : "";
}

export function useImprovements(
  filter: ImprovementsFilter = {},
  enabled = true,
) {
  return useQuery<ImprovementsData, Error>({
    queryKey: ["improvements", filter],
    enabled,
    queryFn: ({ signal }) =>
      apiJson<ImprovementsData>(`/api/improvements${query(filter)}`, {
        signal,
      }),
    retry: false,
  });
}

function json(method: string, body: unknown): RequestInit {
  return {
    method,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  };
}

/** Dismiss, reopen or mark applied. */
export function useSetSuggestionStatus() {
  const qc = useQueryClient();
  return useMutation<
    ImprovementSuggestion,
    Error,
    { id: string; status: SuggestionStatus }
  >({
    mutationFn: ({ id, status }) =>
      apiJson(
        `/api/improvements/suggestions/${encodeURIComponent(id)}`,
        json("PUT", { status }),
      ),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["improvements"] }),
  });
}

/** Start a run that makes a repo suggestion's change as a pull request. */
export function useApplySuggestion() {
  const qc = useQueryClient();
  return useMutation<
    { suggestion: ImprovementSuggestion; run_id: string },
    Error,
    string
  >({
    mutationFn: (id) =>
      apiJson(
        `/api/improvements/suggestions/${encodeURIComponent(id)}/apply`,
        json("POST", {}),
      ),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["improvements"] }),
  });
}

/** Review a workflow's latest runs in a project now, without waiting. */
export function useReviewNow() {
  const qc = useQueryClient();
  return useMutation<
    { started?: unknown; running?: unknown },
    Error,
    { workflow: string; project: string }
  >({
    mutationFn: (body) =>
      apiJson("/api/improvements/review", json("POST", body)),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["improvements"] }),
  });
}

/** What a person can do with a suggestion, in the order the card shows it. */
export type SuggestionAction =
  "apply" | "open-editor" | "mark-applied" | "dismiss" | "reopen";

export function actionsFor(s: ImprovementSuggestion): SuggestionAction[] {
  if (s.status !== "new") return ["reopen"];
  // A repo change is made by a run; a workflow change is made by a person in
  // the editor, who then says so.
  return s.target === "repo"
    ? ["apply", "dismiss"]
    : ["open-editor", "mark-applied", "dismiss"];
}

/** The editor, opened on the step a suggestion is about. */
export function editorLink(
  s: Pick<ImprovementSuggestion, "workflow" | "node">,
) {
  const base = `/settings/workflows/${encodeURIComponent(s.workflow)}`;
  // A suggestion may name several steps ("review-correctness /
  // review-architecture"); only a single id can be selected.
  const node = s.node?.trim();
  return node && /^[A-Za-z0-9_-]+$/.test(node)
    ? `${base}?node=${encodeURIComponent(node)}`
    : base;
}

/** Every workflow and project the page can filter on, sorted. */
export function filterOptions(data: ImprovementsData | undefined): {
  workflows: string[];
  projects: string[];
} {
  const workflows = new Set<string>();
  const projects = new Set<string>();
  for (const w of data?.workflows ?? []) {
    workflows.add(w.workflow);
    projects.add(w.project);
  }
  for (const s of data?.suggestions ?? []) {
    workflows.add(s.workflow);
    projects.add(s.project);
  }
  return {
    workflows: [...workflows].sort(),
    projects: [...projects].sort(),
  };
}
