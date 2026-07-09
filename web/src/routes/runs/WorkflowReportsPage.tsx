/**
 * Generic list page for a workflow that declares `ui.nav` — the declaration-
 * driven counterpart to the bespoke GeoAudits / Reviews pages. A new-run form
 * (project + optional title + description) sits on top; below it, the workflow's
 * runs, each linking to its detail page (where the report tab lives). Reached
 * via `/reports/:workflow`.
 */
import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { ArrowRight, FileSearch, Play } from "lucide-react";
import { AppShell } from "@/components/AppShell";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { useCreateRun, useRuns } from "@/lib/runs";
import { useProjects } from "@/lib/projects";
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
  const label = summary?.ui?.nav?.label ?? workflow;

  const runs = useRuns({});
  const rows = (runs.data ?? []).filter((r) => r.workflow_name === workflow);

  return (
    <AppShell title={label}>
      <div className="mx-auto flex max-w-5xl flex-col gap-6 p-6">
        <NewRunForm workflow={workflow} label={label} />
        <section className="flex flex-col gap-2">
          <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            Runs
          </h2>

          {runs.isLoading && (
            <p className="text-sm text-muted-foreground">Loading…</p>
          )}
          {runs.data && rows.length === 0 && (
            <p className="text-sm text-muted-foreground">
              No runs yet — start one above.
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

/**
 * Start a run of this workflow: pick a project, plus an optional title and
 * description (the task input, `$ARGUMENTS`). Kept general so it works for any
 * `ui.nav` workflow — some (e.g. GEO audit) need only the project, others take
 * a description.
 */
function NewRunForm({ workflow, label }: { workflow: string; label: string }) {
  const navigate = useNavigate();
  const create = useCreateRun();
  const projects = useProjects();
  const [project, setProject] = useState("");
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");

  const canRun = !!project && !create.isPending;

  function run() {
    if (!canRun) return;
    create.mutate(
      {
        workflow,
        project,
        real: true,
        title: title.trim() || undefined,
        description: description.trim(),
      },
      { onSuccess: (res) => navigate(`/runs/${res.run_id}`) },
    );
  }

  return (
    <Card>
      <CardContent className="flex flex-col gap-3 py-4">
        <label className="text-xs font-medium text-muted-foreground">
          New <span className="text-foreground">{label}</span> run
          <span className="ml-1 font-mono text-[11px] text-muted-foreground">
            ({workflow})
          </span>{" "}
          — pick a project; title and description are optional
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
        <input
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          placeholder="Title (optional)"
          className="h-8 rounded-md border border-input bg-transparent px-2 text-[12px] outline-none focus:ring-2 focus:ring-ring"
        />
        <textarea
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          rows={2}
          placeholder="Description (optional) — the task input for the workflow"
          className="rounded-md border border-input bg-transparent px-2 py-1.5 text-[12px] outline-none focus:ring-2 focus:ring-ring"
        />
        <div className="flex justify-end">
          <Button type="button" onClick={run} disabled={!canRun}>
            <Play className="h-3.5 w-3.5" />
            {create.isPending ? "Starting…" : "Run"}
          </Button>
        </div>
        {create.isError && (
          <p className="text-[11px] text-destructive">{create.error.message}</p>
        )}
      </CardContent>
    </Card>
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
