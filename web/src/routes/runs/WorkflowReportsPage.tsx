/**
 * Generic list page for a workflow that declares `ui.nav` — the declaration-
 * driven counterpart to the bespoke GeoAudits / Reviews pages. Lists that
 * workflow's runs, each linking to its detail page (where the report tab lives).
 * Reached via `/reports/:workflow`.
 */
import { Link, useParams } from "react-router-dom";
import { ArrowRight, FileSearch } from "lucide-react";
import { AppShell } from "@/components/AppShell";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { useRuns } from "@/lib/runs";
import { useWorkflowList } from "@/lib/authoring";
import type { RunStatus, RunSummary } from "@/types/run";

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

export function WorkflowReportsPage() {
  const { workflow = "" } = useParams();
  const list = useWorkflowList();
  const summary = list.data?.find((w) => w.name === workflow);
  const title = summary?.ui?.nav?.label ?? workflow;

  const runs = useRuns({});
  const rows = (runs.data ?? []).filter((r) => r.workflow_name === workflow);

  return (
    <AppShell title={title}>
      <div className="mx-auto flex max-w-5xl flex-col gap-6 p-6">
        {summary?.description && (
          <p className="text-sm text-muted-foreground">{summary.description}</p>
        )}
        <section className="flex flex-col gap-2">
          <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            Runs
          </h2>

          {runs.isLoading && (
            <p className="text-sm text-muted-foreground">Loading…</p>
          )}
          {runs.data && rows.length === 0 && (
            <p className="text-sm text-muted-foreground">
              No runs yet. Trigger the <code>{workflow}</code> workflow from the{" "}
              <Link to="/runs" className="underline">
                Runs
              </Link>{" "}
              page.
            </p>
          )}

          <div className="flex flex-col gap-1.5">
            {rows.map((run) => (
              <RunRow key={run.id} run={run} />
            ))}
          </div>
        </section>
      </div>
    </AppShell>
  );
}

function RunRow({ run }: { run: RunSummary }) {
  return (
    <Link to={`/runs/${run.id}`} className="group block">
      <Card className="transition-colors group-hover:border-accent-orange/50">
        <CardContent className="flex items-center gap-3 py-2.5">
          <FileSearch className="h-4 w-4 shrink-0 text-muted-foreground" />
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
