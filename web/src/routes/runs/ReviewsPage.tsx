import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { ArrowRight, ScanSearch } from "lucide-react";
import { AppShell } from "@/components/AppShell";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { useCreateRun, useRuns } from "@/lib/runs";
import { useProjects } from "@/lib/projects";
import type { RunStatus, RunSummary } from "@/types/run";

const REVIEW_WORKFLOW = "review-area";

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

/** Lists `review-area` runs, each linking to its run detail (Review tab). */
export function ReviewsPage() {
  const runs = useRuns({});
  const reviewRuns = (runs.data ?? []).filter(
    (r) => r.workflow_name === REVIEW_WORKFLOW,
  );

  return (
    <AppShell title="Code Review">
      <div className="mx-auto flex max-w-5xl flex-col gap-6 p-6">
        <NewReviewForm reviewRuns={reviewRuns} />
        <section className="flex flex-col gap-2">
          <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            Review runs
          </h2>

          {runs.isLoading && (
            <p className="text-sm text-muted-foreground">Loading…</p>
          )}
          {runs.isError && (
            <p className="text-sm text-destructive">
              {runs.error?.message ?? "Failed to load runs."}
            </p>
          )}
          {runs.data && reviewRuns.length === 0 && (
            <p className="text-sm text-muted-foreground">
              No reviews yet. Start one above, or trigger the{" "}
              <code>review-area</code> workflow from the{" "}
              <Link to="/runs" className="underline">
                Runs
              </Link>{" "}
              page.
            </p>
          )}

          <div className="flex flex-col gap-1.5">
            {reviewRuns.map((run) => (
              <ReviewRow key={run.id} run={run} />
            ))}
          </div>
        </section>
      </div>
    </AppShell>
  );
}

/**
 * Start a `review-area` run: pick a project and describe the area to review
 * (the task input — e.g. "the checkout flow"). The scout + Fable review scope
 * to that area across every repo in the project.
 */
function NewReviewForm({ reviewRuns }: { reviewRuns: RunSummary[] }) {
  const navigate = useNavigate();
  const create = useCreateRun();
  const projects = useProjects();
  const [project, setProject] = useState("");
  const [target, setTarget] = useState("");

  const nextNum = reviewRuns.filter((r) => r.project === project).length + 1;
  const canRun = !!project && target.trim().length > 0 && !create.isPending;

  function run() {
    if (!canRun) return;
    create.mutate(
      {
        workflow: REVIEW_WORKFLOW,
        project,
        real: true,
        title: `Review · ${project} #${nextNum}`,
        description: target.trim(),
      },
      { onSuccess: (res) => navigate(`/runs/${res.run_id}`) },
    );
  }

  return (
    <Card>
      <CardContent className="flex flex-col gap-3 py-4">
        <div className="flex flex-col gap-1.5">
          <label className="text-xs font-medium text-muted-foreground">
            New review — pick a project and describe the area
          </label>
          <select
            value={project}
            onChange={(e) => setProject(e.target.value)}
            className="h-8 rounded-md border border-input bg-transparent px-2 text-[12px] outline-none focus:ring-2 focus:ring-ring"
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
          <textarea
            value={target}
            onChange={(e) => setTarget(e.target.value)}
            rows={2}
            placeholder="What to review — e.g. “the checkout flow” or “the auth token refresh across frontend and backend”"
            className="rounded-md border border-input bg-transparent px-2 py-1.5 text-[12px] outline-none focus:ring-2 focus:ring-ring"
          />
          <div className="flex justify-end">
            <Button type="button" onClick={run} disabled={!canRun}>
              <ScanSearch className="h-3.5 w-3.5" />
              {create.isPending
                ? "Starting…"
                : project
                  ? `Run review #${nextNum}`
                  : "Run review"}
            </Button>
          </div>
        </div>
        {create.isError && (
          <p className="text-[11px] text-destructive">{create.error.message}</p>
        )}
      </CardContent>
    </Card>
  );
}

function ReviewRow({ run }: { run: RunSummary }) {
  return (
    <Link to={`/runs/${run.id}`} className="group block">
      <Card className="transition-colors group-hover:border-accent-orange/50">
        <CardContent className="flex items-center gap-3 py-2.5">
          <ScanSearch className="h-4 w-4 shrink-0 text-muted-foreground" />
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
