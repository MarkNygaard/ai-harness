/**
 * GEO-audit verdict: the structured output the `geo-audit` workflow's `analyze`
 * node emits (score 0–100 + per-category scores + severity-tagged findings).
 * Parsed from the run's node output so the report view can render a dashboard
 * and turn each finding into an `idea-to-pr` task.
 */
import type { NodeView } from "@/types/run";

export type GeoSeverity = "critical" | "high" | "medium" | "low";
export type GeoEffort = "quick" | "medium" | "strategic";

export interface GeoCategory {
  key: string;
  weight?: number;
  score: number;
  summary?: string;
}

export interface GeoFinding {
  severity: GeoSeverity;
  category: string;
  title: string;
  detail?: string;
  fix: string;
  effort?: GeoEffort;
}

export interface GeoVerdict {
  score: number;
  rating: string;
  summary?: string;
  categories: GeoCategory[];
  findings: GeoFinding[];
}

/** Severity order (worst first) for sorting + display. */
export const SEVERITY_RANK: Record<GeoSeverity, number> = {
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

/** The GEO verdict from a run's `analyze` node, or null if not a parseable audit. */
function asVerdict(output: string | undefined): GeoVerdict | null {
  if (!output) return null;
  const v = extractJson(output) as Partial<GeoVerdict> | null;
  if (
    v &&
    typeof v.score === "number" &&
    Array.isArray(v.findings) &&
    Array.isArray(v.categories)
  ) {
    return v as GeoVerdict;
  }
  return null;
}

/**
 * The composite GEO verdict from a run. Prefers the `synthesize` node (the
 * fan-out workflow) then `analyze` (the MVP single-node workflow), and falls
 * back to scanning any node whose output parses to a full verdict — so the view
 * is decoupled from the workflow's node naming.
 */
export function parseGeoVerdict(nodes: NodeView[]): GeoVerdict | null {
  for (const id of ["synthesize", "analyze"]) {
    const v = asVerdict(nodes.find((n) => n.id === id)?.output);
    if (v) return v;
  }
  for (const n of nodes) {
    const v = asVerdict(n.output);
    if (v) return v;
  }
  return null;
}

/** Rating band → semantic colour token. */
export function ratingColor(score: number): string {
  if (score >= 75) return "var(--status-success)";
  if (score >= 60) return "var(--accent-orange)";
  if (score >= 40) return "var(--status-running)";
  return "var(--status-failed)";
}

/**
 * Compose the `idea-to-pr` task description for a finding — enough for the
 * implementer to land the fix as a PR against the project's repo.
 */
export function geoTaskDescription(f: GeoFinding, url: string): string {
  return [
    `GEO audit finding for ${url} — ${f.category} / ${f.severity}: ${f.title}`,
    "",
    f.detail ?? "",
    "",
    `Fix: ${f.fix}`,
    "",
    "Implement this in the project's source so the live site improves its " +
      "GEO / AI-search readiness. Keep the change focused on this finding.",
  ]
    .join("\n")
    .trim();
}
