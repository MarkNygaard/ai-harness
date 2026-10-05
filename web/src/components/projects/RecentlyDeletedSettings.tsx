import { useState } from "react";
import { IconRestore } from "@tabler/icons-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { clearedIn, daysAgo } from "@/lib/bin-age";
import { useRestoreTrash, useTrash } from "@/lib/projects";
import type { TrashEntry } from "@/types/project";

const secs = (iso: string) => Date.parse(iso) / 1000;

/**
 * Deleted projects and Linear bindings that can still be restored.
 *
 * A delete moves the row to a bin rather than removing it, and the bin keeps
 * it for 14 days. Credentials and tokens are not here: deleting one of those
 * is meant to make it stop working. Renders nothing while the bin is empty.
 */
export function RecentlyDeletedSettings() {
  const trash = useTrash();
  const restore = useRestoreTrash();
  const [warning, setWarning] = useState<string | null>(null);
  const entries = trash.data ?? [];
  if (entries.length === 0 && !warning) return null;
  const now = Date.now() / 1000;

  return (
    <section className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          Recently deleted
        </h2>
        <span className="text-[11px] text-muted-foreground">
          projects and Linear bindings, kept for 14 days
        </span>
        <span className="ml-auto text-[11px] tabular-nums text-muted-foreground">
          {entries.length}
        </span>
      </div>
      <div className="flex flex-col gap-2">
        {entries.map((t: TrashEntry) => (
          <Card key={t.id}>
            <CardContent className="flex items-center gap-3 py-3">
              <div className="min-w-0 flex-1">
                <div className="flex min-w-0 items-center gap-2">
                  <span className="truncate font-mono text-[13px] text-muted-foreground">
                    {t.label}
                  </span>
                  <Badge variant="outline" className="shrink-0 text-[10px]">
                    {t.kind === "project" ? "project" : "Linear binding"}
                  </Badge>
                </div>
                <p className="mt-0.5 text-xs text-muted-foreground">
                  Deleted {daysAgo(secs(t.deleted_at), now)}
                  {t.deleted_actor && ` by ${t.deleted_actor}`} · cleared out{" "}
                  {clearedIn(secs(t.expires_at), now)}
                </p>
              </div>
              <Button
                variant="outline"
                size="sm"
                disabled={restore.isPending}
                onClick={() =>
                  restore.mutate(t.id, {
                    onSuccess: (res) => setWarning(res.warning),
                  })
                }
                title={
                  t.kind === "project"
                    ? `Restore ${t.label} and clone its repo again`
                    : `Restore the ${t.workflow} binding on ${t.project}`
                }
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
      {warning && <p className="text-xs text-muted-foreground">{warning}</p>}
    </section>
  );
}
