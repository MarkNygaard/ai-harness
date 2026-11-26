import { useState } from "react";
import { Link } from "react-router-dom";
import { IconBell, IconBellRinging } from "@tabler/icons-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { EventPicker } from "@/components/notifications/EventPicker";
import {
  describeEvents,
  type NotificationEvent,
  useNotificationsAvailable,
  useSubscribe,
  useSubscriptions,
  useUnsubscribe,
} from "@/lib/notifications";

/** What a person most likely wants to hear about one trigger. */
const DEFAULT_EVENTS: NotificationEvent[] = ["trigger_failed", "run_failed"];

/**
 * Subscribe to one trigger or schedule — the case of wanting to hear about
 * this one and nothing else. Shows "Notifying you" once subscribed, and the
 * same dialog then turns it off.
 *
 * Renders nothing on a harness without accounts, where notifications have
 * nobody to belong to.
 */
export function NotifyMeButton({
  triggerId,
  name,
}: {
  triggerId: string;
  name: string;
}) {
  const available = useNotificationsAvailable();
  const subscriptions = useSubscriptions();
  const subscribe = useSubscribe();
  const unsubscribe = useUnsubscribe();
  const [open, setOpen] = useState(false);
  const [events, setEvents] = useState<NotificationEvent[]>(DEFAULT_EVENTS);

  if (!available) return null;
  const existing = (subscriptions.data ?? []).filter(
    (s) => s.scope_kind === "trigger" && s.scope_value === triggerId,
  );
  const busy = subscribe.isPending || unsubscribe.isPending;

  async function turnOff() {
    for (const s of existing) await unsubscribe.mutateAsync(s.id);
    setOpen(false);
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (next) setEvents(DEFAULT_EVENTS);
        setOpen(next);
      }}
    >
      <DialogTrigger
        render={
          <Button
            size="sm"
            variant="outline"
            disabled={subscriptions.isLoading}
            title={
              existing.length > 0
                ? `You hear about: ${describeEvents(existing.flatMap((s) => s.events))}`
                : "Get notified about this trigger"
            }
          />
        }
      >
        {existing.length > 0 ? (
          <>
            <IconBellRinging /> Notifying you
          </>
        ) : (
          <>
            <IconBell /> Notify me
          </>
        )}
      </DialogTrigger>
      <DialogContent showCloseButton={!busy}>
        <DialogHeader>
          <DialogTitle>Notifications for “{name}”</DialogTitle>
          <DialogDescription>
            Only you are notified, in your inbox. Everything else you follow is
            under{" "}
            <Link to="/settings/notifications" className="underline">
              Settings → Notifications
            </Link>
            .
          </DialogDescription>
        </DialogHeader>
        {existing.length > 0 ? (
          <p className="text-[13px]">
            You are told when:{" "}
            {describeEvents(existing.flatMap((s) => s.events))}.
          </p>
        ) : (
          <EventPicker value={events} onChange={setEvents} />
        )}
        {(subscribe.isError || unsubscribe.isError) && (
          <p role="alert" className="text-[11px] text-destructive">
            {(subscribe.error ?? unsubscribe.error)?.message}
          </p>
        )}
        <DialogFooter>
          <DialogClose
            render={<Button type="button" variant="outline" disabled={busy} />}
          >
            Cancel
          </DialogClose>
          {existing.length > 0 ? (
            <Button
              variant="destructive"
              disabled={busy}
              onClick={() => void turnOff()}
            >
              Stop notifying me
            </Button>
          ) : (
            <Button
              disabled={busy || events.length === 0}
              onClick={() =>
                subscribe.mutate(
                  { events, scope_kind: "trigger", scope_value: triggerId },
                  { onSuccess: () => setOpen(false) },
                )
              }
            >
              {subscribe.isPending ? "Saving…" : "Notify me"}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
