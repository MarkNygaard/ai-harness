import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { ArrowRight, Play } from "lucide-react";
import { AppShell } from "@/components/AppShell";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { useCreateRun, useRuns } from "@/lib/runs";
import type { RunStatus, RunSummary } from "@/types/run";

const STATUS_VARIANT: Record<RunStatus, "running" | "success" | "failed" | "skipped"> = {
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
  const runs = useRuns();
  return (
    <AppShell
      active="runs"
      breadcrumb={[{ label: "harness" }, { label: "Runs", current: true }]}
      searchPlaceholder="Search runs…"
    >
      <div className="mx-auto flex max-w-5xl flex-col gap-6 p-6">
        <NewRunForm />
        <section className="flex flex-col gap-2">
          <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            Recent runs
          </h2>
          {runs.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
          {runs.isError && (
            <p className="text-sm text-destructive">Failed to load runs: {runs.error.message}</p>
          )}
          {runs.data?.length === 0 && (
            <p className="text-sm text-muted-foreground">
              No runs yet. Submit a workflow above to start one.
            </p>
          )}
          <div className="flex flex-col gap-1.5">
            {runs.data?.map((run) => <RunRow key={run.id} run={run} />)}
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
          <Badge variant={STATUS_VARIANT[run.status] ?? "default"}>{run.status}</Badge>
          <div className="min-w-0 flex-1">
            <div className="truncate text-sm font-medium">{run.workflow_name}</div>
            <div className="truncate font-mono text-[11px] text-muted-foreground">{run.id}</div>
          </div>
          <div className="text-right text-[11px] text-muted-foreground">
            <div>{run.node_count} steps</div>
            <div>{relativeTime(run.recorded_at)}</div>
          </div>
          <ArrowRight className="h-4 w-4 text-muted-foreground transition-transform group-hover:translate-x-0.5" />
        </CardContent>
      </Card>
    </Link>
  );
}

function NewRunForm() {
  const navigate = useNavigate();
  const create = useCreateRun();
  const [workflow, setWorkflow] = useState("examples/hello.yaml");
  const [args, setArgs] = useState("");
  const [real, setReal] = useState(false);

  function submit(e: React.FormEvent) {
    e.preventDefault();
    create.mutate(
      { workflow, args: args || undefined, real },
      { onSuccess: (res) => navigate(`/runs/${res.run_id}`) },
    );
  }

  return (
    <Card>
      <CardContent className="py-4">
        <form onSubmit={submit} className="flex flex-col gap-3">
          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium text-muted-foreground">Workflow path</label>
            <input
              value={workflow}
              onChange={(e) => setWorkflow(e.target.value)}
              placeholder="examples/hello.yaml"
              className="h-8 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium text-muted-foreground">Arguments (optional)</label>
            <input
              value={args}
              onChange={(e) => setArgs(e.target.value)}
              placeholder="passed as $ARGUMENTS / $USER_MESSAGE"
              className="h-8 rounded-md border border-input bg-transparent px-2.5 text-[12px] outline-none focus:ring-2 focus:ring-ring"
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
            <Button type="submit" disabled={create.isPending || !workflow.trim()}>
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
