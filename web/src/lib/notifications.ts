/**
 * Notifications: what you asked to hear about, and the inbox it lands in.
 *
 * Nothing is on by default — every notification comes from a subscription
 * somebody made. They need accounts, so these hooks stay idle on a harness
 * without sign-in.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./api";
import { useAuthStatus } from "./auth";

export type NotificationEvent =
  "run_failed" | "run_cancelled" | "run_completed" | "trigger_failed";

export type SubscriptionScope = "trigger" | "workflow" | "project" | "mine";

/** Every event, in the order a person reads them, with what each means. */
export const EVENTS: {
  value: NotificationEvent;
  label: string;
  description: string;
}[] = [
  {
    value: "trigger_failed",
    label: "Trigger couldn't start a run",
    description:
      "A trigger or schedule fired, but no run started: a bad webhook secret or signature, a schedule error, a Linear claim that failed.",
  },
  {
    value: "run_failed",
    label: "Run failed",
    description: "A run ended with a failed step.",
  },
  {
    value: "run_cancelled",
    label: "Run cancelled",
    description:
      "A run was stopped: by a person, or by the server when its worker went away.",
  },
  {
    value: "run_completed",
    label: "Run finished",
    description: "A run completed successfully.",
  },
];

export const SCOPES: { value: SubscriptionScope; label: string }[] = [
  { value: "mine", label: "My runs and triggers" },
  { value: "project", label: "A project" },
  { value: "workflow", label: "A workflow" },
  { value: "trigger", label: "One trigger or schedule" },
];

export interface NotificationSubscription {
  id: string;
  user_id: string;
  events: NotificationEvent[];
  scope_kind: SubscriptionScope;
  scope_value: string;
  channels: NotificationChannel[];
  /** Where a `slack_channel` subscription posts. */
  slack_channel: string | null;
  created_at: string;
}

export interface SubscriptionInput {
  events: NotificationEvent[];
  scope_kind: SubscriptionScope;
  scope_value: string;
  /** Besides the inbox, which every subscription has. */
  channels?: NotificationChannel[];
  slack_channel?: string | null;
}

export type NotificationChannel =
  "in_app" | "email" | "slack_dm" | "slack_channel";

/** Where a notification can go besides the inbox, and what each means. */
export const CHANNELS: {
  value: Exclude<NotificationChannel, "in_app">;
  label: string;
  /** Which of the harness's connections it needs. */
  needs: "email" | "slack";
}[] = [
  { value: "email", label: "Email", needs: "email" },
  { value: "slack_dm", label: "Slack direct message", needs: "slack" },
  { value: "slack_channel", label: "Slack channel", needs: "slack" },
];

/** What this harness can deliver with, and where email goes. */
export interface ChannelAvailability {
  email: { available: boolean; address: string };
  slack: { available: boolean };
}

export interface Notification {
  id: number;
  event: NotificationEvent;
  title: string;
  body: string;
  /** Where in this UI it points, e.g. `/runs/<id>`. */
  link: string | null;
  run_id: string | null;
  trigger_id: string | null;
  project: string | null;
  created_at: string;
  read_at: string | null;
  /** Why it did not reach email or Slack, when it didn't. */
  delivery_error: string | null;
}

interface InboxResponse {
  notifications: Notification[];
  unread: number;
}

/** Notifications belong to a signed-in person. */
export function useNotificationsAvailable(): boolean {
  const auth = useAuthStatus().data;
  return auth?.mode === "accounts" && !!auth.user;
}

const INBOX_KEY = ["notifications", "inbox"];
const SUBSCRIPTIONS_KEY = ["notifications", "subscriptions"];

/** The inbox, newest first. Polls like the rest of the UI. */
export function useInbox() {
  const enabled = useNotificationsAvailable();
  return useQuery<InboxResponse, Error>({
    queryKey: INBOX_KEY,
    enabled,
    queryFn: ({ signal }) =>
      apiJson<InboxResponse>("/api/me/notifications?limit=100", { signal }),
    retry: false,
    refetchInterval: 15_000,
  });
}

/** Mark the given notifications read, or every one when `ids` is omitted. */
export function useMarkRead() {
  const qc = useQueryClient();
  return useMutation<{ marked: number }, Error, number[] | undefined>({
    mutationFn: (ids) =>
      apiJson<{ marked: number }>("/api/me/notifications/read", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(ids ? { ids } : {}),
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: INBOX_KEY }),
  });
}

export function useSubscriptions() {
  const enabled = useNotificationsAvailable();
  return useQuery<NotificationSubscription[], Error>({
    queryKey: SUBSCRIPTIONS_KEY,
    enabled,
    queryFn: async ({ signal }) =>
      (
        await apiJson<{ subscriptions: NotificationSubscription[] }>(
          "/api/me/notification-subscriptions",
          { signal },
        )
      ).subscriptions,
    retry: false,
    refetchInterval: false,
  });
}

export function useSubscribe() {
  const qc = useQueryClient();
  return useMutation<NotificationSubscription, Error, SubscriptionInput>({
    mutationFn: (body) =>
      apiJson<NotificationSubscription>("/api/me/notification-subscriptions", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: SUBSCRIPTIONS_KEY }),
  });
}

export function useUnsubscribe() {
  const qc = useQueryClient();
  return useMutation<void, Error, string>({
    // 204 with no body, so not `apiJson`.
    mutationFn: async (id) => {
      await apiFetch(
        `/api/me/notification-subscriptions/${encodeURIComponent(id)}`,
        { method: "DELETE" },
      );
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: SUBSCRIPTIONS_KEY }),
  });
}

/** "Run failed, Run cancelled" — the events of a subscription, as read. */
export function describeEvents(events: NotificationEvent[]): string {
  return EVENTS.filter((e) => events.includes(e.value))
    .map((e) => e.label)
    .join(", ");
}

export function useNotificationChannels() {
  const enabled = useNotificationsAvailable();
  return useQuery<ChannelAvailability, Error>({
    queryKey: ["notifications", "channels"],
    enabled,
    queryFn: ({ signal }) =>
      apiJson<ChannelAvailability>("/api/me/notification-channels", {
        signal,
      }),
    retry: false,
    refetchInterval: false,
  });
}

/** Send yourself a test on one channel. Rejects with what went wrong. */
export function useSendTest() {
  return useMutation<
    { sent: boolean },
    Error,
    { channel: Exclude<NotificationChannel, "in_app">; slack_channel?: string }
  >({
    mutationFn: (body) =>
      apiJson<{ sent: boolean }>("/api/me/notifications/test", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
  });
}

/** "Inbox, Email, Slack #builds" — where a subscription delivers. */
export function describeChannels(s: NotificationSubscription): string {
  const out = ["Inbox"];
  if (s.channels.includes("email")) out.push("Email");
  if (s.channels.includes("slack_dm")) out.push("Slack DM");
  if (s.channels.includes("slack_channel") && s.slack_channel)
    out.push(`Slack #${s.slack_channel}`);
  return out.join(", ");
}
