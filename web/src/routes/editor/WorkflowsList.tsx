import { Link } from "react-router-dom";
import { IconBinaryTree2, IconPlus } from "@tabler/icons-react";
import { AppShell } from "@/components/AppShell";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useWorkflowList } from "@/lib/authoring";
import type { WorkflowSummary } from "@/types/authoring";

/** Browse bundled + project workflows; click to edit, or create a new one. */
export function WorkflowsList() {
  const workflows = useWorkflowList();

  return (
    <AppShell
      title="Workflows"
      actions={
        <Button size="sm" render={<Link to="/editor/new" />}>
          <IconPlus className="size-4" />
          New workflow
        </Button>
      }
    >
      <div className="mx-auto flex max-w-4xl flex-col gap-4 p-6">
        <p className="text-sm text-muted-foreground">
          Edit a workflow in the visual builder. Bundled defaults are read-only templates —
          saving one creates an editable project copy that shadows it.
        </p>

        {workflows.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
        {workflows.isError && (
          <p className="text-sm text-destructive">Failed to load workflows: {workflows.error.message}</p>
        )}
        {workflows.data?.length === 0 && (
          <p className="text-sm text-muted-foreground">No workflows yet.</p>
        )}

        <div className="flex flex-col gap-2">
          {workflows.data?.map((wf) => <WorkflowRow key={wf.name} wf={wf} />)}
        </div>
      </div>
    </AppShell>
  );
}

function WorkflowRow({ wf }: { wf: WorkflowSummary }) {
  return (
    <Link to={`/editor/${encodeURIComponent(wf.name)}`} className="group block">
      <Card className="transition-colors group-hover:border-accent-orange/50">
        <CardContent className="flex items-center gap-3 py-3">
          <IconBinaryTree2 className="size-5 shrink-0 text-accent-orange" />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="truncate font-mono text-sm font-medium">{wf.name}</span>
              <Badge variant={wf.source === "bundled" ? "outline" : "secondary"}>{wf.source}</Badge>
            </div>
            {wf.description && (
              <p className="mt-0.5 line-clamp-2 text-xs text-muted-foreground">{wf.description}</p>
            )}
          </div>
          <div className="shrink-0 text-right text-xs text-muted-foreground">
            {wf.node_count} step{wf.node_count === 1 ? "" : "s"}
          </div>
        </CardContent>
      </Card>
    </Link>
  );
}
