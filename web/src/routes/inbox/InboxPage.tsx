import { Link } from "react-router-dom";
import { IconChecks, IconSettings } from "@tabler/icons-react";
import { AppShell } from "@/components/AppShell";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { cn } from "@/lib/utils";
import {
  type Notification,
  useInbox,
  useMarkRead,
  useNotificationsAvailable,
} from "@/lib/notifications";

/** "5 min ago", "yesterday 14:02", or a date for anything older. */
function when(iso: string): string {
  const at = new Date(iso);
  const mins = Math.round((Date.now() - at.getTime()) / 60_000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins} min ago`;
  if (mins < 24 * 60) return `${Math.round(mins / 60)} h ago`;
  return at.toLocaleString(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
}

/** `/inbox` — what you asked to be told about, newest first. */
export function InboxPage() {
  const available = useNotificationsAvailable();
  const inbox = useInbox();
  const markRead = useMarkRead();
  const rows = inbox.data?.notifications ?? [];
  const unread = inbox.data?.unread ?? 0;

  return (
    <AppShell
      title="Inbox"
      viewActions={
        available && (
          <>
            <Button
              size="sm"
              variant="outline"
              disabled={unread === 0 || markRead.isPending}
              onClick={() => markRead.mutate(undefined)}
            >
              <IconChecks /> Mark all read
            </Button>
            <Button
              size="sm"
              variant="outline"
              render={<Link to="/settings/notifications" />}
            >
              <IconSettings /> Choose what you hear about
            </Button>
          </>
        )
      }
    >
      <div className="mx-auto flex max-w-3xl flex-col gap-4 p-6">
        <p className="max-w-prose text-xs text-muted-foreground sm:pr-80">
          Runs and triggers you follow. Opening one marks it read.
        </p>
        {!available ? (
          <p className="text-sm text-muted-foreground">
            Notifications belong to a person, so they need sign-in.
          </p>
        ) : inbox.isLoading ? (
          <p className="text-xs text-muted-foreground">Loading…</p>
        ) : inbox.isError ? (
          <p className="text-sm text-destructive">{inbox.error.message}</p>
        ) : rows.length === 0 ? (
          <Card>
            <CardContent className="px-4 py-6 text-center text-sm text-muted-foreground">
              Nothing here.{" "}
              <Link to="/settings/notifications" className="underline">
                Choose what to be notified about
              </Link>
              , or use <span className="font-medium">Notify me</span> on a
              trigger.
            </CardContent>
          </Card>
        ) : (
          <Card>
            <CardContent className="p-0">
              {rows.map((n) => (
                <InboxRow
                  key={n.id}
                  n={n}
                  onOpen={() => {
                    if (!n.read_at) markRead.mutate([n.id]);
                  }}
                />
              ))}
            </CardContent>
          </Card>
        )}
      </div>
    </AppShell>
  );
}

function InboxRow({ n, onOpen }: { n: Notification; onOpen: () => void }) {
  const unread = !n.read_at;
  const body = (
    <div className="flex min-w-0 items-start gap-3">
      <span
        aria-hidden
        className={cn(
          "mt-1.5 size-2 shrink-0 rounded-full",
          unread ? "bg-primary" : "bg-transparent",
        )}
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline justify-between gap-3">
          <span
            className={cn(
              "truncate text-[13px]",
              unread ? "font-semibold" : "font-medium text-muted-foreground",
            )}
          >
            {n.title}
          </span>
          <span className="shrink-0 text-[11px] text-muted-foreground">
            {when(n.created_at)}
          </span>
        </div>
        {n.body && (
          <div className="mt-0.5 line-clamp-2 text-[12px] text-muted-foreground">
            {n.body}
          </div>
        )}
        {n.delivery_error && (
          <div className="mt-0.5 text-[11px] text-destructive">
            Not delivered everywhere: {n.delivery_error}
          </div>
        )}
      </div>
      {unread && <span className="sr-only">Unread</span>}
    </div>
  );
  const rowCls =
    "block border-t border-border px-4 py-3 first:border-t-0 hover:bg-muted/50";
  return n.link ? (
    <Link to={n.link} onClick={onOpen} className={rowCls}>
      {body}
    </Link>
  ) : (
    <button
      type="button"
      onClick={onOpen}
      className={cn(rowCls, "w-full text-left")}
    >
      {body}
    </button>
  );
}
