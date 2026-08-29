import { useMemo, useState } from "react";
import { useParams, useNavigate } from "react-router-dom";
import { ReactFlowProvider } from "@xyflow/react";
import { Info } from "lucide-react";
import { AppShell } from "@/components/AppShell";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from "@/components/ui/sheet";
import { Markdown } from "@/components/Markdown";
import { RunFlow } from "@/components/runflow/RunFlow";
import { TaskOverview } from "@/components/runflow/TaskOverview";
import { WorkflowReport } from "@/components/report/WorkflowReport";
import { useCancelRun, useRerunRun, useRunView } from "@/lib/runs";
import { parseWorkflowVerdict, useWorkflowUi } from "@/lib/report";
import type { RunStatus } from "@/types/run";
const STATUS_VARIANT: Record<
  RunStatus,
  "running" | "success" | "failed" | "skipped"
> = {
  running: "running",
  completed: "success",
  failed: "failed",
  cancelled: "failed",
};

type Panel = "graph" | "overview" | "report";

export function RunDetailPage() {
  const { id = null } = useParams();
  const run = useRunView(id);
  const navigate = useNavigate();
  const [panel, setPanel] = useState<Panel>("graph");
  const cancel = useCancelRun();
  const rerun = useRerunRun();
  // A workflow that declares `ui.report` gets a report tab, rendered from its
  // verdict node's JSON output.
  const declaredReport = useWorkflowUi(run.workflow)?.report ?? null;
  const report = useMemo(
    () =>
      declaredReport
        ? parseWorkflowVerdict(run.nodes, declaredReport.verdict_node)
        : null,
    [run.nodes, declaredReport],
  );

  const done = run.nodes.filter((n) =>
    ["success", "failed", "skipped", "cancelled"].includes(n.status),
  ).length;

  return (
    <AppShell title={run.title ?? run.workflow ?? id ?? "run"}>
      <div className="flex h-full flex-col">
        <div className="flex flex-none items-center gap-3 border-b border-border px-6 py-3">
          <Badge variant={STATUS_VARIANT[run.status] ?? "default"}>
            {run.status}
          </Badge>
          {run.live && (
            <span className="flex items-center gap-1.5 text-xs text-status-running">
              <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-status-running" />
              Live
            </span>
          )}
          {/* The title is in the header bar above, which is this page's one
              place for it. This line carries only what that bar cannot: which
              workflow ran, and the run's own id. */}
          <div
            className="min-w-0 truncate font-mono text-[11px] text-muted-foreground"
            title={run.title ?? undefined}
          >
            {run.title && run.workflow ? `${run.workflow} · ` : ""}
            {id}
          </div>
          <div className="ml-auto text-xs text-muted-foreground">
            {done}/{run.nodes.length} steps
          </div>
          {run.live && id && (
            <Button
              variant="outline"
              size="sm"
              disabled={cancel.isPending}
              onClick={() => cancel.mutate(id)}
            >
              {cancel.isPending ? "Stopping…" : "Stop"}
            </Button>
          )}
          {!run.live && id && (
            <Button
              variant="outline"
              size="sm"
              disabled={rerun.isPending}
              title="Start a fresh run of this workflow with the same inputs"
              onClick={() =>
                rerun.mutate(id, {
                  onSuccess: (res) => navigate(`/runs/${res.run_id}`),
                })
              }
            >
              {rerun.isPending ? "Starting…" : "Rerun"}
            </Button>
          )}
          <Sheet>
            <SheetTrigger
              render={
                <Button variant="outline" size="sm">
                  <Info className="size-3.5" />
                  Details
                </Button>
              }
            />
            <SheetContent className="flex flex-col gap-0 p-0">
              <SheetHeader>
                <SheetTitle>Run details</SheetTitle>
              </SheetHeader>
              <Separator />
              <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 p-4 text-sm">
                <Field label="Project" value={run.project ?? "—"} />
                <Field label="Workflow" value={run.workflow ?? "—"} />
                <Field label="Run ID" value={id ?? "—"} mono />
                <Field label="Status" value={run.status} />
                <Field
                  label="Recorded"
                  value={
                    run.recordedAt
                      ? new Date(run.recordedAt).toLocaleString()
                      : "—"
                  }
                />
              </dl>
              <Separator />
              <div className="min-h-0 flex-1 overflow-y-auto p-4">
                <div className="mb-1 text-xs font-medium text-muted-foreground">
                  Description
                </div>
                {run.description ? (
                  <Markdown>{run.description}</Markdown>
                ) : (
                  <p className="text-sm italic text-muted-foreground">
                    No description
                  </p>
                )}
              </div>
            </SheetContent>
          </Sheet>
          <div className="flex overflow-hidden rounded-md border border-border text-xs">
            <PanelTab
              label="Graph"
              active={panel === "graph"}
              onClick={() => setPanel("graph")}
            />
            <PanelTab
              label="Overview"
              active={panel === "overview"}
              onClick={() => setPanel("overview")}
            />
            {report && declaredReport && (
              <PanelTab
                label={declaredReport.label}
                active={panel === "report"}
                onClick={() => setPanel("report")}
              />
            )}
          </div>
        </div>

        <div className="min-h-0 flex-1">
          {panel === "graph" ? (
            <ReactFlowProvider>
              <RunFlow nodes={run.nodes} />
            </ReactFlowProvider>
          ) : panel === "report" && report && declaredReport ? (
            <div className="h-full overflow-y-auto p-6">
              <WorkflowReport
                verdict={report}
                scored={declaredReport.scored}
                project={run.project}
                runId={id}
                workflow={run.workflow}
                verdictNode={declaredReport.verdict_node}
                actions={declaredReport.actions ?? []}
                status={declaredReport.status ?? "none"}
              />
            </div>
          ) : (
            <div className="h-full overflow-y-auto p-6">
              <TaskOverview nodes={run.nodes} />
            </div>
          )}
        </div>
      </div>
    </AppShell>
  );
}

function PanelTab({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={
        active
          ? "bg-secondary px-3 py-1 font-medium text-secondary-foreground"
          : "px-3 py-1 text-muted-foreground hover:bg-secondary/50"
      }
    >
      {label}
    </button>
  );
}

function Field({
  label,
  value,
  mono,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <>
      <dt className="text-muted-foreground">{label}</dt>
      <dd className={mono ? "truncate font-mono text-xs" : "truncate"}>
        {value}
      </dd>
    </>
  );
}
