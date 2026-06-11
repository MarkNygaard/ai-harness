import { Link } from "react-router-dom";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { useCreateRun } from "@/lib/runs";
import { useProjects } from "@/lib/projects";
import {
  SEVERITY_RANK,
  geoTaskDescription,
  ratingColor,
  type GeoFinding,
  type GeoSeverity,
  type GeoVerdict,
} from "@/lib/geo";

const SEV_VARIANT: Record<GeoSeverity, "failed" | "running" | "secondary"> = {
  critical: "failed",
  high: "failed",
  medium: "running",
  low: "secondary",
};

/**
 * Renders a geo-audit verdict: score dashboard, per-dimension scores, and the
 * findings — each with a one-click "Build this" that fires `idea-to-pr` against
 * the same project to land the fix as a PR.
 */
export function GeoReport({
  verdict,
  project,
}: {
  verdict: GeoVerdict;
  project: string | null;
}) {
  const projects = useProjects();
  // external_url lands with the project-external-url change; read it loosely so
  // this view doesn't hard-depend on that type at compile time.
  const url =
    (
      projects.data?.find((p) => p.name === project) as
        | { external_url?: string | null }
        | undefined
    )?.external_url ?? "";
  const findings = [...verdict.findings].sort(
    (a, b) => SEVERITY_RANK[a.severity] - SEVERITY_RANK[b.severity],
  );

  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-6">
      <div className="flex items-center gap-4 border border-border p-4">
        <div className="flex flex-col items-center">
          <span
            className="text-4xl font-semibold tabular-nums"
            style={{ color: ratingColor(verdict.score) }}
          >
            {verdict.score}
          </span>
          <span className="text-[11px] uppercase tracking-wide text-muted-foreground">
            GEO score
          </span>
        </div>
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <Badge variant="secondary">{verdict.rating}</Badge>
            {url && (
              <a
                href={url}
                target="_blank"
                rel="noreferrer"
                className="truncate text-xs text-accent-orange hover:underline"
              >
                {url}
              </a>
            )}
          </div>
          {verdict.summary && (
            <p className="mt-1 text-[13px] text-muted-foreground">
              {verdict.summary}
            </p>
          )}
        </div>
      </div>

      <section className="flex flex-col gap-2">
        <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          Scores by dimension
        </h3>
        <div className="flex flex-col gap-2 border border-border p-3">
          {verdict.categories.map((c) => (
            <div key={c.key} className="flex items-center gap-3 text-[13px]">
              <div className="w-28 shrink-0 font-medium">{c.key}</div>
              <div className="h-2.5 flex-1 overflow-hidden bg-secondary/50">
                <div
                  className="h-full"
                  style={{
                    width: `${Math.max(0, Math.min(100, c.score))}%`,
                    backgroundColor: ratingColor(c.score),
                  }}
                />
              </div>
              <div className="w-8 shrink-0 text-right tabular-nums text-muted-foreground">
                {c.score}
              </div>
            </div>
          ))}
        </div>
      </section>

      <section className="flex flex-col gap-2">
        <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          Findings ({findings.length}) — click “Build this” to fix via idea-to-pr
        </h3>
        <div className="flex flex-col gap-2">
          {findings.map((f, i) => (
            <FindingRow key={i} finding={f} project={project} url={url} />
          ))}
        </div>
      </section>
    </div>
  );
}

function FindingRow({
  finding,
  project,
  url,
}: {
  finding: GeoFinding;
  project: string | null;
  url: string;
}) {
  const create = useCreateRun();
  return (
    <div className="border-l-2 border-border bg-card p-3 pl-3">
      <div className="flex items-center gap-2">
        <Badge variant={SEV_VARIANT[finding.severity] ?? "secondary"}>
          {finding.severity}
        </Badge>
        <span className="text-[11px] text-muted-foreground">
          {finding.category}
          {finding.effort ? ` · ${finding.effort}` : ""}
        </span>
        <span className="truncate text-sm font-medium">{finding.title}</span>
        <div className="ml-auto shrink-0">
          {create.data ? (
            <Link
              to={`/runs/${create.data.run_id}`}
              className="text-xs text-accent-orange hover:underline"
            >
              Building →
            </Link>
          ) : (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={create.isPending || !project}
              title={!project ? "Run has no project to open a PR against" : ""}
              onClick={() =>
                create.mutate({
                  workflow: "idea-to-pr",
                  project: project ?? undefined,
                  real: true,
                  title: finding.title,
                  description: geoTaskDescription(finding, url),
                })
              }
            >
              {create.isPending ? "Starting…" : "Build this"}
            </Button>
          )}
        </div>
      </div>
      {finding.detail && (
        <p className="mt-1 text-[13px] text-muted-foreground">
          {finding.detail}
        </p>
      )}
      <p className="mt-1 text-[13px]">
        <span className="text-muted-foreground">Fix: </span>
        {finding.fix}
      </p>
      {create.isError && (
        <p className="mt-1 text-[11px] text-destructive">
          {create.error.message}
        </p>
      )}
    </div>
  );
}
