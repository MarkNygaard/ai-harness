import { useState } from "react";
import { History } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import {
  useRestoreVersion,
  useWorkflowVersion,
  useWorkflowVersions,
} from "@/lib/authoring";

/** "just now", "12 minutes ago", "3 hours ago", "2 days ago". */
export function savedAgo(then: number, now: number): string {
  const secs = Math.max(0, now - then);
  const unit = (n: number, word: string) =>
    `${n} ${word}${n === 1 ? "" : "s"} ago`;
  if (secs < 60) return "just now";
  if (secs < 3600) return unit(Math.floor(secs / 60), "minute");
  if (secs < 86_400) return unit(Math.floor(secs / 3600), "hour");
  return unit(Math.floor(secs / 86_400), "day");
}

/**
 * A workflow's earlier versions, each viewable and restorable.
 *
 * A save keeps what it replaces, and a restore is a save, so restoring the
 * wrong version is undone by restoring again from this same list.
 */
export function VersionHistoryDialog({
  name,
  onRestored,
}: {
  name: string;
  /** Called after a restore, so the editor reloads what is now current. */
  onRestored: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [viewing, setViewing] = useState<string | null>(null);
  const versions = useWorkflowVersions(name, open);
  const yaml = useWorkflowVersion(name, viewing);
  const restore = useRestoreVersion(name);
  const now = Date.now() / 1000;

  const doRestore = (id: string, when: string) => {
    if (
      !window.confirm(
        `Restore the version saved ${when}? The current version is kept in the history, so this can be undone. Unsaved changes in the editor are lost.`,
      )
    )
      return;
    restore.mutate(id, {
      onSuccess: () => {
        setOpen(false);
        setViewing(null);
        onRestored();
      },
    });
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        setOpen(o);
        if (!o) setViewing(null);
      }}
    >
      <DialogTrigger
        render={
          <Button
            variant="outline"
            size="sm"
            title="Earlier versions of this workflow"
          />
        }
      >
        <History className="h-3.5 w-3.5" />
        History
      </DialogTrigger>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>History</DialogTitle>
          <DialogDescription>
            Each save keeps the version it replaces. Edits less than five
            minutes apart count as one, and the newest 20 are kept.
          </DialogDescription>
        </DialogHeader>

        {versions.isLoading && (
          <p className="text-xs text-muted-foreground">Loading…</p>
        )}
        {versions.isError && (
          <p className="text-xs text-destructive">{versions.error.message}</p>
        )}
        {versions.data?.length === 0 && (
          <p className="text-xs text-muted-foreground">
            No earlier versions yet. One is kept the next time this workflow is
            saved over.
          </p>
        )}

        <div className="flex min-w-0 flex-col gap-2">
          {(versions.data ?? []).map((v) => {
            const when = savedAgo(v.saved_at, now);
            const exact = new Date(v.saved_at * 1000).toLocaleString();
            return (
              <div
                key={v.id}
                className="flex min-w-0 flex-col gap-2 rounded-md border border-border px-3 py-2"
              >
                <div className="flex items-center gap-2">
                  <div className="min-w-0 flex-1">
                    <span className="text-[13px]" title={exact}>
                      Saved {when}
                    </span>
                    <span className="ml-2 text-[11px] text-muted-foreground">
                      {v.node_count} step{v.node_count === 1 ? "" : "s"}
                    </span>
                  </div>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => setViewing(viewing === v.id ? null : v.id)}
                  >
                    {viewing === v.id ? "Hide" : "View"}
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={restore.isPending}
                    onClick={() => doRestore(v.id, when)}
                  >
                    Restore
                  </Button>
                </div>
                {viewing === v.id && (
                  <pre className="max-h-80 overflow-auto rounded-md bg-muted p-2 font-mono text-[11px] leading-4">
                    {yaml.isLoading ? "Loading…" : (yaml.data?.yaml ?? "")}
                  </pre>
                )}
              </div>
            );
          })}
        </div>
        {restore.isError && (
          <p className="text-xs text-destructive">{restore.error.message}</p>
        )}
      </DialogContent>
    </Dialog>
  );
}
