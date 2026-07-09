/**
 * Generic workflow **report** verdict — the declarative counterpart to the
 * bespoke `lib/geo.ts` / `lib/review.ts`. Any workflow that declares
 * `ui.report` in its YAML gets a report tab rendered from a node's JSON output,
 * shaped as `{ summary?, score?, rating?, findings[] }`. Read-only for now
 * (no per-finding triage — that arrives when the finding-stores are unified).
 */
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
