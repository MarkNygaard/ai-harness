import { useState } from "react";
import { Link } from "react-router-dom";
import { IconTrash } from "@tabler/icons-react";
import { SettingsShell } from "@/components/SettingsShell";
import { EventPicker } from "@/components/notifications/EventPicker";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useWorkflowList } from "@/lib/authoring";
import {
  describeEvents,
  type NotificationEvent,
  type NotificationSubscription,
  SCOPES,
  type SubscriptionScope,
  useNotificationsAvailable,
  useSubscribe,
  useSubscriptions,
  useUnsubscribe,
} from "@/lib/notifications";
import { useProjects } from "@/lib/projects";
import {
  triggerHref,
  triggerName,
  triggerProject,
  useTriggers,
} from "@/lib/triggers";

/** `/settings/notifications` — what you asked to be told about. */
export function NotificationsPage() {
  const available = useNotificationsAvailable();
  return (
    <SettingsShell title="Notifications">
      <div className="mx-auto flex max-w-3xl flex-col gap-5 p-6">
        <p className="max-w-prose text-xs text-muted-foreground">
          Nothing is sent unless you ask for it here, or with{" "}
          <span className="font-medium">Notify me</span> on a trigger&rsquo;s
          page. Notifications land in your{" "}
          <Link to="/inbox" className="underline">
            inbox
          </Link>
          .
        </p>
        {available ? (
          <>
            <SubscriptionList />
            <NewSubscription />
          </>
        ) : (
          <p className="text-sm text-muted-foreground">
            Notifications belong to a person, so they need sign-in, which this
            harness doesn&rsquo;t have yet.
          </p>
        )}
      </div>
    </SettingsShell>
  );
}

/** What a subscription is about, in words, with a link where there is one. */
function ScopeLabel({ s }: { s: NotificationSubscription }) {
  const triggers = useTriggers();
  switch (s.scope_kind) {
    case "mine":
      return <>Runs you start and triggers you created</>;
    case "project":
      return (
        <>
          Project <span className="font-mono">{s.scope_value}</span>
        </>
      );
    case "workflow":
      return (
        <>
          Workflow <span className="font-mono">{s.scope_value}</span>
        </>
      );
    case "trigger": {
      const t = triggers.data?.find((t) => t.key === s.scope_value);
      return t ? (
        <>
          Trigger{" "}
          <Link to={triggerHref(t)} className="underline">
            {triggerName(t)}
          </Link>
        </>
      ) : (
        <>A trigger that no longer exists</>
      );
    }
  }
}

function SubscriptionList() {
  const subscriptions = useSubscriptions();
  const unsubscribe = useUnsubscribe();
  const rows = subscriptions.data ?? [];
  return (
    <section className="flex flex-col gap-2">
      <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        You are notified about
      </h2>
      <Card>
        <CardContent className="p-0">
          {subscriptions.isLoading ? (
            <p className="px-4 py-3.5 text-xs text-muted-foreground">
              Loading…
            </p>
          ) : subscriptions.isError ? (
            <p className="px-4 py-3.5 text-xs text-destructive">
              {subscriptions.error.message}
            </p>
          ) : rows.length === 0 ? (
            <p className="px-4 py-3.5 text-xs text-muted-foreground">
              Nothing yet.
            </p>
          ) : (
            rows.map((s) => (
              <div
                key={s.id}
                className="flex items-center justify-between gap-4 border-t border-border px-4 py-3 first:border-t-0"
              >
                <div className="min-w-0">
                  <div className="text-[13px] font-medium">
                    <ScopeLabel s={s} />
                  </div>
                  <div className="text-[11px] text-muted-foreground">
                    {describeEvents(s.events)}
                  </div>
                </div>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={unsubscribe.isPending}
                  onClick={() => unsubscribe.mutate(s.id)}
                  title="Stop these notifications"
                >
                  <IconTrash /> Remove
                </Button>
              </div>
            ))
          )}
        </CardContent>
      </Card>
    </section>
  );
}

function NewSubscription() {
  const subscribe = useSubscribe();
  const projects = useProjects();
  const workflows = useWorkflowList();
  const triggers = useTriggers();
  const [events, setEvents] = useState<NotificationEvent[]>(["run_failed"]);
  const [scope, setScope] = useState<SubscriptionScope>("mine");
  const [value, setValue] = useState("");

  const options: { value: string; label: string }[] =
    scope === "project"
      ? (projects.data ?? []).map((p) => ({ value: p.name, label: p.name }))
      : scope === "workflow"
        ? (workflows.data ?? []).map((w) => ({ value: w.name, label: w.name }))
        : scope === "trigger"
          ? (triggers.data ?? []).map((t) => ({
              value: t.key,
              label: `${triggerName(t)} · ${triggerProject(t)}`,
            }))
          : [];
  const needsValue = scope !== "mine";
  const ready = events.length > 0 && (!needsValue || value !== "");

  return (
    <section className="flex flex-col gap-2">
      <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        Add
      </h2>
      <Card>
        <CardContent className="flex flex-col gap-4 p-4">
          <div className="flex flex-col gap-1.5">
            <Label>About</Label>
            <div className="flex flex-col gap-2 sm:flex-row">
              <Select
                value={scope}
                onValueChange={(v) => {
                  if (!v) return;
                  setScope(v as SubscriptionScope);
                  setValue("");
                }}
              >
                <SelectTrigger className="w-full sm:w-56">
                  <SelectValue>
                    {(v: string | null) =>
                      SCOPES.find((s) => s.value === v)?.label ?? v ?? ""
                    }
                  </SelectValue>
                </SelectTrigger>
                <SelectContent>
                  {SCOPES.map((s) => (
                    <SelectItem key={s.value} value={s.value}>
                      {s.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {needsValue && (
                <Select value={value} onValueChange={(v) => setValue(v ?? "")}>
                  <SelectTrigger className="w-full">
                    <SelectValue>
                      {(v: string | null) =>
                        options.find((o) => o.value === v)?.label ?? "Choose…"
                      }
                    </SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    {options.map((o) => (
                      <SelectItem key={o.value} value={o.value}>
                        {o.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
            </div>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label>When</Label>
            <EventPicker value={events} onChange={setEvents} />
          </div>
          {subscribe.isError && (
            <p role="alert" className="text-[11px] text-destructive">
              {subscribe.error.message}
            </p>
          )}
          <div>
            <Button
              disabled={!ready || subscribe.isPending}
              onClick={() =>
                subscribe.mutate(
                  { events, scope_kind: scope, scope_value: value },
                  {
                    onSuccess: () => {
                      setValue("");
                    },
                  },
                )
              }
            >
              {subscribe.isPending ? "Saving…" : "Notify me"}
            </Button>
          </div>
        </CardContent>
      </Card>
    </section>
  );
}
