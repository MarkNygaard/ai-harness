import { useState } from "react";
import { useQueries } from "@tanstack/react-query";
import { Link, useNavigate } from "react-router-dom";
import { ArrowRight, Globe } from "lucide-react";
import { AppShell } from "@/components/AppShell";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { apiJson } from "@/lib/api";
import { nodesFromDetail, useCreateRun, useRuns } from "@/lib/runs";
import { useProjects } from "@/lib/projects";
import { parseGeoVerdict, ratingColor } from "@/lib/geo";
import type { RunDetail, RunStatus, RunSummary } from "@/types/run";

const STATUS_VARIANT: Record<
  RunStatus,
  "running" | "success" | "failed" | "skipped"
> = {
  running: "running",
  completed: "success",
  failed: "failed",
  cancelled: "failed",
};

function relativeTime(iso: string): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return iso;
  const secs = Math.round((Date.now() - then) / 1000);
  if (secs < 60) return `${secs}s ago`;
  const mins = Math.round(secs / 60);
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.round(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  return new Date(then).toLocaleDateString();
}

/** Lists GEO-audit runs, each linking to its run detail (GEO tab + score). */
export function GeoAuditsPage() {
  const runs = useRuns({});
  const auditRuns = (runs.data ?? []).filter(
    (r) => r.workflow_name === "geo-audit",
  );

  // Fetch each audit's detail to surface its score (cached per run).
  const details = useQueries({
    queries: auditRuns.map((r) => ({
      queryKey: ["run", r.id],
      queryFn: ({ signal }: { signal: AbortSignal }) =>
        apiJson<RunDetail>(`/api/runs/${r.id}`, { signal }),
      staleTime: 60_000,
    })),
  });
  const scoreById: Record<string, number> = {};
  details.forEach((d, i) => {
    if (!d.data) return;
    const v = parseGeoVerdict(nodesFromDetail(d.data));
    if (v) scoreById[auditRuns[i].id] = v.score;
  });

  return (
    <AppShell title="GEO Audit">
      <div className="mx-auto flex max-w-5xl flex-col gap-6 p-6">
        <NewGeoAuditForm auditRuns={auditRuns} />
        <section className="flex flex-col gap-2">
          <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            GEO audit runs
          </h2>

          {runs.isLoading && (
            <p className="text-sm text-muted-foreground">Loading…</p>
          )}
          {runs.isError && (
            <p className="text-sm text-destructive">
              {runs.error?.message ?? "Failed to load runs."}
            </p>
          )}
          {runs.data && auditRuns.length === 0 && (
            <p className="text-sm text-muted-foreground">
              No GEO audits yet. Trigger the <code>geo-audit</code> workflow for
              a project with an external URL from the{" "}
              <Link to="/runs" className="underline">
                Runs
              </Link>{" "}
              page.
            </p>
          )}

          <div className="flex flex-col gap-1.5">
            {auditRuns.map((run) => (
              <GeoAuditRow key={run.id} run={run} score={scoreById[run.id]} />
            ))}
          </div>
        </section>
      </div>
    </AppShell>
  );
}

/**
 * One-click GEO audit: pick a project and run it. The geo-audit workflow reads
 * the project's external URL, so no task spec is needed. The title carries a
 * per-project run number (`GEO Audit · {project} #{n}`) so each audit is a
 * distinct, readable entry.
 */
function NewGeoAuditForm({ auditRuns }: { auditRuns: RunSummary[] }) {
  const navigate = useNavigate();
  const create = useCreateRun();
  const projects = useProjects();
  const [project, setProject] = useState("");

  const selected = projects.data?.find((p) => p.name === project);
  const externalUrl = selected?.external_url ?? "";
  // Per-project audit count → this run's number (best-effort, from loaded runs).
  const nextNum = auditRuns.filter((r) => r.project === project).length + 1;
  const canRun = !!project && !!externalUrl && !create.isPending;

  function run() {
    if (!canRun) return;
    create.mutate(
      {
        workflow: "geo-audit",
        project,
        real: true,
        title: `GEO Audit · ${project} #${nextNum}`,
      },
      { onSuccess: (res) => navigate(`/runs/${res.run_id}`) },
    );
  }

  return (
    <Card>
      <CardContent className="flex flex-col gap-3 py-4">
        <div className="flex flex-col gap-1.5">
          <label className="text-xs font-medium text-muted-foreground">
            New GEO audit — pick a project
          </label>
          <div className="flex items-center gap-2">
            <select
              value={project}
              onChange={(e) => setProject(e.target.value)}
              className="h-8 flex-1 rounded-md border border-input bg-transparent px-2 text-[12px] outline-none focus:ring-2 focus:ring-ring"
            >
              <option value="" disabled>
                Select a project…
              </option>
              {projects.data?.map((p) => (
                <option key={p.name} value={p.name}>
                  {p.name}
                </option>
              ))}
            </select>
            <Button type="button" onClick={run} disabled={!canRun}>
              <Globe className="h-3.5 w-3.5" />
              {create.isPending
                ? "Starting…"
                : project
                  ? `Run GEO Audit #${nextNum}`
                  : "Run GEO Audit"}
            </Button>
          </div>
        </div>
        {project && !externalUrl && (
          <p className="text-[11px] text-muted-foreground">
            {project} has no external URL — set one on the{" "}
            <Link to="/projects" className="text-accent-orange hover:underline">
              Projects
            </Link>{" "}
            page so the audit can fetch the live site.
          </p>
        )}
        {create.isError && (
          <p className="text-[11px] text-destructive">{create.error.message}</p>
        )}
      </CardContent>
    </Card>
  );
}

function GeoAuditRow({
  run,
  score,
}: {
  run: RunSummary;
  score: number | undefined;
}) {
  return (
    <Link to={`/runs/${run.id}`} className="group block">
      <Card className="transition-colors group-hover:border-accent-orange/50">
        <CardContent className="flex items-center gap-3 py-2.5">
          <Globe className="h-4 w-4 shrink-0 text-muted-foreground" />
          {score != null ? (
            <span
              className="w-9 shrink-0 text-center text-sm font-semibold tabular-nums"
              style={{ color: ratingColor(score) }}
              title="GEO score"
            >
              {score}
            </span>
          ) : (
            <span className="w-9 shrink-0 text-center text-[11px] text-muted-foreground">
              —
            </span>
          )}
          <div className="min-w-0 flex-1">
            <span className="truncate text-sm font-medium">
              {run.title || run.workflow_name}
            </span>
            <div className="mt-1 flex flex-wrap items-center gap-1.5">
              <Badge variant={STATUS_VARIANT[run.status] ?? "default"}>
                {run.status}
              </Badge>
              {run.project && <Badge variant="outline">{run.project}</Badge>}
            </div>
          </div>
          <div className="text-right text-[11px] text-muted-foreground">
            {relativeTime(run.recorded_at)}
          </div>
          <ArrowRight className="h-4 w-4 text-muted-foreground transition-transform group-hover:translate-x-0.5" />
        </CardContent>
      </Card>
    </Link>
  );
}
