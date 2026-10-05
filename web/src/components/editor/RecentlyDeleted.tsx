import { IconRestore } from "@tabler/icons-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useRestoreWorkflow, useWorkflowTrash } from "@/lib/authoring";
import { titleFromSlug } from "@/lib/workflow-name";
import type { TrashedWorkflow } from "@/types/authoring";

const DAY = 86_400;

/** "today", "yesterday", "3 days ago" — a bin entry's age, in days. */
export function daysAgo(then: number, now: number): string {
  const days = Math.floor((now - then) / DAY);
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  return `${days} days ago`;
}

/** "in 1 day", "in 12 days", "today" — until a bin entry is cleared out. */
export function clearedIn(expires: number, now: number): string {
  const days = Math.ceil((expires - now) / DAY);
  if (days <= 0) return "today";
  return days === 1 ? "in 1 day" : `in ${days} days`;
}

/**
 * Deleted workflows that can still be restored.
 *
 * A delete moves the file to a bin on the server rather than removing it, and
 * the bin keeps it for 14 days. Renders nothing while the bin is empty, which
 * is almost always.
 */
export function RecentlyDeleted() {
  const trash = useWorkflowTrash();
  const restore = useRestoreWorkflow();
  const entries = trash.data ?? [];
  if (entries.length === 0) return null;
  const now = Date.now() / 1000;

  return (
    <section className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          Recently deleted
        </h2>
        <span className="text-[11px] text-muted-foreground">
          kept for 14 days, then cleared out
        </span>
        <span className="ml-auto text-[11px] tabular-nums text-muted-foreground">
          {entries.length}
        </span>
      </div>
      <div className="flex flex-col gap-2">
        {entries.map((t: TrashedWorkflow) => (
          <Card key={t.id}>
            <CardContent className="flex items-center gap-3 py-3">
              <div className="min-w-0 flex-1">
                <div className="flex min-w-0 items-baseline gap-2">
                  <span className="truncate text-sm font-medium text-muted-foreground">
                    {titleFromSlug(t.name)}
                  </span>
                  <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
                    {t.name}
                  </span>
                </div>
                <p className="mt-0.5 text-xs text-muted-foreground">
                  Deleted {daysAgo(t.deleted_at, now)} · cleared out{" "}
                  {clearedIn(t.expires_at, now)}
                  {t.node_count > 0 &&
                    ` · ${t.node_count} step${t.node_count === 1 ? "" : "s"}`}
                </p>
              </div>
              <Button
                variant="outline"
                size="sm"
                disabled={restore.isPending}
                onClick={() => restore.mutate(t.id)}
                title={`Restore ${t.name} under its own name`}
              >
                <IconRestore className="size-4" />
                Restore
              </Button>
            </CardContent>
          </Card>
        ))}
      </div>
      {restore.isError && (
        <p className="text-xs text-destructive">{restore.error.message}</p>
      )}
    </section>
  );
}
