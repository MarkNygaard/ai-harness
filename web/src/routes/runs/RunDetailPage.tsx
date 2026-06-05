import { useState } from "react";
import { useParams } from "react-router-dom";
import { ReactFlowProvider } from "@xyflow/react";
import { AppShell } from "@/components/AppShell";
import { Badge } from "@/components/ui/badge";
import { RunFlow } from "@/components/runflow/RunFlow";
import { TaskOverview } from "@/components/runflow/TaskOverview";
import { useRunView } from "@/lib/runs";
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

type Panel = "graph" | "overview";

export function RunDetailPage() {
  const { id = null } = useParams();
  const run = useRunView(id);
  const [panel, setPanel] = useState<Panel>("graph");

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
          <div className="min-w-0">
            <div className="truncate text-sm font-semibold">
              {run.title ?? run.workflow ?? "Workflow"}
            </div>
            <div className="truncate font-mono text-[11px] text-muted-foreground">
              {run.title && run.workflow ? `${run.workflow} · ` : ""}
              {id}
            </div>
          </div>
          <div className="ml-auto text-xs text-muted-foreground">
            {done}/{run.nodes.length} steps
          </div>
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
          </div>
        </div>

        <div className="min-h-0 flex-1">
          {panel === "graph" ? (
            <ReactFlowProvider>
              <RunFlow nodes={run.nodes} />
            </ReactFlowProvider>
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
