import { useState } from "react";
import { IconDownload, IconRefresh, IconTrash } from "@tabler/icons-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  InstallNameConflict,
  useInstallWorkflow,
  useLibrary,
  useUninstallWorkflow,
} from "@/lib/library";
import type { LibraryEntry } from "@/types/library";

/**
 * Browse the workflow library and install from it.
 *
 * Two groups, because the trust in them differs: **Official** is published by
 * the project, **Community** by anyone and always shown with its author. A
 * badge alone would leave that distinction to be noticed rather than read.
 *
 * The list is only fetched while the dialog is open — it is a remote service,
 * and nobody browsing the editor has asked to reach it.
 */
export function LibraryDialog() {
  const [open, setOpen] = useState(false);
  const library = useLibrary(open);
  const entries = library.data ?? [];
  const official = entries.filter((e) => e.official);
  const community = entries.filter((e) => !e.official);

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger
        render={
          <Button variant="outline" size="sm">
            <IconDownload className="size-4" />
            Library
          </Button>
        }
      />
      <DialogContent className="flex max-h-[80vh] flex-col gap-0 sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>Workflow library</DialogTitle>
        </DialogHeader>

        <div className="min-h-0 flex-1 overflow-y-auto px-1 py-2">
          <p className="px-3 pb-3 text-xs text-muted-foreground">
            Installing makes a copy on this harness. It never changes on its own
            — when the original moves on you are offered an update.
          </p>

          {library.isLoading && (
            <p className="px-3 py-6 text-sm text-muted-foreground">Loading…</p>
          )}
          {library.isError && (
            <p className="px-3 py-6 text-sm text-destructive">
              {library.error.message}
            </p>
          )}
          {!library.isLoading && !library.isError && entries.length === 0 && (
            <p className="px-3 py-6 text-sm text-muted-foreground">
              The library has nothing published yet.
            </p>
          )}

          <Group title="Official" note="published by the project">
            {official.map((e) => (
              <Row key={e.slug} entry={e} />
            ))}
          </Group>
          <Group title="Community" note="published by people using the harness">
            {community.map((e) => (
              <Row key={e.slug} entry={e} />
            ))}
          </Group>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** A titled group. Renders nothing when empty, so a library with no community
 *  entries does not show an empty heading. */
function Group({
  title,
  note,
  children,
}: {
  title: string;
  note: string;
  children: React.ReactNode;
}) {
  const items = Array.isArray(children) ? children : [children];
  if (items.filter(Boolean).length === 0) return null;
  return (
    <section className="flex flex-col">
      <div className="flex items-baseline gap-2 px-3 pb-1 pt-3">
        <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          {title}
        </h3>
        <span className="text-[11px] text-muted-foreground">{note}</span>
      </div>
      {children}
    </section>
  );
}

function Row({ entry }: { entry: LibraryEntry }) {
  const install = useInstallWorkflow();
  const uninstall = useUninstallWorkflow();
  // Set when the server refuses a name; holds what the person is choosing
  // instead. Nothing is written until they confirm.
  const [renaming, setRenaming] = useState<string | null>(null);

  const busy = install.isPending || uninstall.isPending;
  const conflict =
    install.error instanceof InstallNameConflict
      ? install.error.conflict
      : null;
  // Any other failure is not a question for the person, so it reads as a plain
  // error rather than as a prompt.
  const failure =
    install.error && !conflict
      ? install.error.message
      : (uninstall.error?.message ?? null);

  function run(name?: string) {
    install.mutate(
      { slug: entry.slug, name },
      {
        onError: (e) => {
          if (e instanceof InstallNameConflict) {
            setRenaming(e.conflict.suggested_name ?? "");
          }
        },
        onSuccess: () => setRenaming(null),
      },
    );
  }

  return (
    <div className="flex flex-col gap-2 border-t border-border/60 px-3 py-2.5 first:border-t-0">
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-sm font-medium">{entry.title}</span>
            <span className="font-mono text-[10px] text-muted-foreground">
              {entry.slug}
            </span>
            {entry.installed_as && (
              <Badge variant="secondary" className="text-[10px]">
                installed
              </Badge>
            )}
            {entry.update_available && (
              <Badge variant="outline" className="text-[10px]">
                update available
              </Badge>
            )}
          </div>
          <p className="mt-0.5 line-clamp-2 text-xs text-muted-foreground">
            {entry.description}
          </p>
          <div className="mt-1 flex flex-wrap items-center gap-2 text-[11px] text-muted-foreground">
            {!entry.official && <span>by {entry.publisher}</span>}
            <span>
              {entry.installs} install{entry.installs === 1 ? "" : "s"}
            </span>
            {entry.installed_version !== null && (
              <span>
                v{entry.installed_version}
                {entry.update_available && ` → v${entry.latest_version}`}
              </span>
            )}
            {/* The local name is only worth showing when it differs — a slug
                installed under its own name tells nobody anything. */}
            {entry.installed_as && entry.installed_as !== entry.slug && (
              <span>as {entry.installed_as}</span>
            )}
          </div>
        </div>

        <div className="flex shrink-0 items-center gap-1">
          {entry.update_available && (
            <Button size="sm" disabled={busy} onClick={() => run()}>
              <IconRefresh className="size-3.5" />
              Update
            </Button>
          )}
          {!entry.installed_as && (
            <Button
              size="sm"
              variant="outline"
              disabled={busy || entry.latest_version === null}
              title={
                entry.latest_version === null
                  ? "This workflow has no published version yet"
                  : undefined
              }
              onClick={() => run()}
            >
              Install
            </Button>
          )}
          {entry.installed_as && (
            <Button
              size="sm"
              variant="ghost"
              disabled={busy}
              aria-label={`Uninstall ${entry.title}`}
              title="Remove this workflow from this harness"
              onClick={() => {
                if (
                  window.confirm(
                    `Remove ${entry.installed_as}? The workflow file is deleted from this harness.`,
                  )
                ) {
                  uninstall.mutate(entry.slug);
                }
              }}
            >
              <IconTrash className="size-3.5" />
            </Button>
          )}
        </div>
      </div>

      {/* The name is taken. Asked rather than worked around: installing as
          `geo-audit-2` unannounced leaves somebody with a workflow whose name
          they did not choose and no idea why. */}
      {conflict && renaming !== null && (
        <div className="flex flex-col gap-2 rounded-md border border-border bg-muted/40 p-2.5">
          <p className="text-xs">
            This harness already has a workflow called{" "}
            <span className="font-mono">{conflict.conflict}</span>. Install
            under a different name:
          </p>
          <div className="flex items-center gap-2">
            <Input
              value={renaming}
              autoFocus
              aria-label="Name to install under"
              onChange={(e) => setRenaming(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && renaming.trim()) run(renaming.trim());
                if (e.key === "Escape") setRenaming(null);
              }}
            />
            <Button
              size="sm"
              disabled={busy || !renaming.trim()}
              onClick={() => run(renaming.trim())}
            >
              Install
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => setRenaming(null)}
              disabled={busy}
            >
              Cancel
            </Button>
          </div>
        </div>
      )}

      {failure && <p className="text-xs text-destructive">{failure}</p>}
    </div>
  );
}
