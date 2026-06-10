import { useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { ArrowRight, Play, Trash2 } from "lucide-react";
import { AppShell } from "@/components/AppShell";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { useCreateRun, useDeleteRun, useRuns } from "@/lib/runs";
import { NO_PROJECT } from "@/lib/dashboard";
import { useProjects } from "@/lib/projects";
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

export function RunsPage() {
  const [params] = useSearchParams();
  const projectFilter = params.get("project")?.trim() || null;
  const unassignedParam = params.get("unassigned");
  const unassignedFilter =
    unassignedParam === "true" || unassignedParam === "1";
  const activeFilterLabel = unassignedFilter ? NO_PROJECT : projectFilter;
  const runs = useRuns({
    project: projectFilter,
    unassigned: unassignedFilter,
  });
  return (
    <AppShell title="Runs">
      <div className="mx-auto flex max-w-5xl flex-col gap-6 p-6">
        <NewRunForm />
        <section className="flex flex-col gap-2">
          <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            Recent runs
          </h2>
          {activeFilterLabel && (
            <p className="text-sm">
              Filtered by project:{" "}
              <span className="font-medium">{activeFilterLabel}</span>{" "}
              <Link to="/runs" className="underline text-muted-foreground">
                clear
              </Link>
            </p>
          )}
          {runs.isLoading && (
            <p className="text-sm text-muted-foreground">Loading…</p>
          )}
          {runs.isError && (
            <p className="text-sm text-destructive">
              Failed to load runs: {runs.error.message}
            </p>
          )}
          {runs.data?.length === 0 && (
            <p className="text-sm text-muted-foreground">
              {activeFilterLabel
                ? "No runs match this project."
                : "No runs yet. Submit a workflow above to start one."}
            </p>
          )}
          <div className="flex flex-col gap-1.5">
            {runs.data?.map((run) => (
              <RunRow key={run.id} run={run} />
            ))}
          </div>
        </section>
      </div>
    </AppShell>
  );
}

function RunRow({ run }: { run: RunSummary }) {
  const del = useDeleteRun();
  function onDelete(e: React.MouseEvent) {
    // The row is a Link — don't navigate when deleting.
    e.preventDefault();
    e.stopPropagation();
    if (
      window.confirm(`Delete run ${run.title || run.id}? This can't be undone.`)
    ) {
      del.mutate(run.id);
    }
  }
  return (
    <Link to={`/runs/${run.id}`} className="group block">
      <Card className="transition-colors group-hover:border-accent-orange/50">
        <CardContent className="flex items-center gap-3 py-2.5">
          <Badge variant={STATUS_VARIANT[run.status] ?? "default"}>
            {run.status}
          </Badge>
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="truncate text-sm font-medium">
                {run.title || run.workflow_name}
              </span>
              {run.project && (
                <Badge variant="outline" className="shrink-0">
                  {run.project}
                </Badge>
              )}
              {run.ab_arm && (
                <Badge variant="secondary" className="shrink-0">
                  A/B · {run.ab_arm.toUpperCase()}
                  {run.ab_label ? ` · ${run.ab_label}` : ""}
                </Badge>
              )}
            </div>
            <div className="truncate font-mono text-[11px] text-muted-foreground">
              {run.title ? `${run.workflow_name} · ` : ""}
              {run.id}
            </div>
          </div>
          <div className="text-right text-[11px] text-muted-foreground">
            <div>{run.node_count} steps</div>
            <div>{relativeTime(run.recorded_at)}</div>
          </div>
          <ArrowRight className="h-4 w-4 text-muted-foreground transition-transform group-hover:translate-x-0.5" />
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label="Delete run"
            title="Delete run"
            disabled={del.isPending}
            onClick={onDelete}
            className="size-7 shrink-0 text-muted-foreground hover:text-destructive"
          >
            <Trash2 className="size-3.5" />
          </Button>
        </CardContent>
      </Card>
    </Link>
  );
}

function NewRunForm() {
  const navigate = useNavigate();
  const create = useCreateRun();
  const projects = useProjects();
  const [project, setProject] = useState("");
  const [workflow, setWorkflow] = useState("idea-to-pr");
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [real, setReal] = useState(false);

  // Selecting a project pre-fills its default workflow (if it declares one).
  function onProjectChange(name: string) {
    setProject(name);
    const def = projects.data?.find((p) => p.name === name)?.default_workflow;
    if (def) setWorkflow(def);
  }

  function submit(e: React.FormEvent) {
    e.preventDefault();
    create.mutate(
      {
        workflow,
        project: project || undefined,
        title: title.trim() || undefined,
        description: description.trim() || undefined,
        real,
      },
      { onSuccess: (res) => navigate(`/runs/${res.run_id}`) },
    );
  }

  return (
    <Card>
      <CardContent className="py-4">
        <form onSubmit={submit} className="flex flex-col gap-3">
          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium text-muted-foreground">
              Project
            </label>
            <select
              value={project}
              onChange={(e) => onProjectChange(e.target.value)}
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
            {projects.data?.length === 0 && (
              <span className="text-[11px] text-muted-foreground">
                No projects yet —{" "}
                <Link
                  to="/projects"
                  className="text-accent-orange hover:underline"
                >
                  register one
                </Link>{" "}
                first.
              </span>
            )}
          </div>
          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium text-muted-foreground">
              Workflow (bundled name or path)
            </label>
            <input
              value={workflow}
              onChange={(e) => setWorkflow(e.target.value)}
              placeholder="idea-to-pr"
              className="h-8 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium text-muted-foreground">
              Title
            </label>
            <input
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="short task name (e.g. Add rate limiting to the API)"
              className="h-8 rounded-md border border-input bg-transparent px-2.5 text-[12px] outline-none focus:ring-2 focus:ring-ring"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium text-muted-foreground">
              Description (the task spec — what you want done)
            </label>
            <textarea
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              rows={5}
              placeholder="Describe the work fully so the agents can decide and carry it out autonomously. Fed to nodes as $ARGUMENTS / $USER_MESSAGE / $TASK_DESCRIPTION."
              className="rounded-md border border-input bg-transparent p-2 text-[12px] outline-none focus:ring-2 focus:ring-ring"
            />
          </div>
          <div className="flex items-center justify-between gap-3">
            <label className="flex items-center gap-2 text-xs text-muted-foreground">
              <input
                type="checkbox"
                checked={real}
                onChange={(e) => setReal(e.target.checked)}
                className="accent-[var(--accent-orange)]"
              />
              Use real agents (otherwise echo)
            </label>
            <Button
              type="submit"
              disabled={create.isPending || !project || !workflow.trim()}
            >
              <Play className="h-3.5 w-3.5" />
              {create.isPending ? "Starting…" : "Run workflow"}
            </Button>
          </div>
          {create.isError && (
            <p className="text-xs text-destructive">{create.error.message}</p>
          )}
        </form>
      </CardContent>
    </Card>
  );
}
