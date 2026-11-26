import { useState } from "react";
import { Link } from "react-router-dom";
import { IconTrash } from "@tabler/icons-react";
import { SettingsShell } from "@/components/SettingsShell";
import { EventPicker } from "@/components/notifications/EventPicker";
import {
  type ChannelChoice,
  ChannelPicker,
  channelChoiceComplete,
  NO_CHANNELS,
} from "@/components/notifications/ChannelPicker";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
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
  describeChannels,
  describeEvents,
  type NotificationEvent,
  type NotificationSubscription,
  SCOPES,
  type SubscriptionScope,
  useNotificationChannels,
  useNotificationsAvailable,
  useSendTest,
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
            <TryChannels />
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
                    {describeEvents(s.events)} → {describeChannels(s)}
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
  const [channels, setChannels] = useState<ChannelChoice>(NO_CHANNELS);

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
  const ready =
    events.length > 0 &&
    (!needsValue || value !== "") &&
    channelChoiceComplete(channels);

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
          <div className="flex flex-col gap-1.5">
            <Label>Where</Label>
            <ChannelPicker value={channels} onChange={setChannels} />
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
                  {
                    events,
                    scope_kind: scope,
                    scope_value: value,
                    channels: channels.channels,
                    slack_channel: channels.slackChannel.trim() || null,
                  },
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

/**
 * Send yourself a test on each channel that can deliver here — the way to find
 * out that Slack cannot see your address before a real failure goes missing.
 */
function TryChannels() {
  const available = useNotificationChannels().data;
  const test = useSendTest();
  const [channel, setChannel] = useState("");
  if (!available || (!available.email.available && !available.slack.available))
    return null;
  const sent = test.isSuccess && !test.isPending;
  return (
    <section className="flex flex-col gap-2">
      <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        Try your channels
      </h2>
      <Card>
        <CardContent className="flex flex-col gap-3 p-4">
          <div className="flex flex-wrap items-center gap-2">
            {available.email.available && (
              <Button
                variant="outline"
                disabled={test.isPending}
                onClick={() => test.mutate({ channel: "email" })}
              >
                Email me a test
              </Button>
            )}
            {available.slack.available && (
              <>
                <Button
                  variant="outline"
                  disabled={test.isPending}
                  onClick={() => test.mutate({ channel: "slack_dm" })}
                >
                  Slack me a test
                </Button>
                <Input
                  className="w-40"
                  placeholder="#channel"
                  aria-label="Slack channel to test"
                  value={channel}
                  onChange={(e) => setChannel(e.target.value)}
                />
                <Button
                  variant="outline"
                  disabled={test.isPending || channel.trim() === ""}
                  onClick={() =>
                    test.mutate({
                      channel: "slack_channel",
                      slack_channel: channel,
                    })
                  }
                >
                  Post a test there
                </Button>
              </>
            )}
          </div>
          {test.isPending && (
            <p className="text-[11px] text-muted-foreground">Sending…</p>
          )}
          {sent && (
            <p className="text-[11px] text-muted-foreground">
              Sent. Check that it arrived.
            </p>
          )}
          {test.isError && (
            <p role="alert" className="text-[11px] text-destructive">
              {test.error.message}
            </p>
          )}
        </CardContent>
      </Card>
    </section>
  );
}
