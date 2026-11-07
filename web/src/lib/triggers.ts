/**
 * Triggers: everything that starts a run by itself.
 *
 * A trigger is *when* (a Linear status, a webhook, a time) plus *what* (a
 * project and a workflow). Linear triggers and webhooks are the Triggers page's
 * own model (`/api/triggers`); schedules keep their store and join the list
 * here.
 *
 * A Linear trigger only *starts* the run. Unlike a classic project binding it
 * has no status map: where the issue goes next is the workflow's business,
 * through its `linear:` steps. Which of the two starts Linear runs is one
 * installation-wide switch — see {@link useTriggerSystem}.
 */
import { useMemo } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./api";
import { describeCron, type Schedule, useSchedules } from "./schedules";

export type TriggerKind = "linear" | "webhook" | "schedule";

/** Display metadata per kind, in the order the kind picker offers them. */
export const TRIGGER_KINDS: {
  kind: TriggerKind;
  label: string;
  description: string;
}[] = [
  {
    kind: "linear",
    label: "Linear",
    description:
      "Start a run when an issue enters a status and is delegated to the harness, or carries a label. The workflow moves the issue from there.",
  },
  {
    kind: "webhook",
    label: "Webhook",
    description:
      "Start a run when another service calls a URL — GitHub, Sentry, a deploy, anything that can send a signed request.",
  },
  {
    kind: "schedule",
    label: "Schedule",
    description:
      "Start a run at set times, such as every Monday at 08:00, with no issue behind it.",
  },
];

export function kindMeta(kind: TriggerKind) {
  return TRIGGER_KINDS.find((k) => k.kind === kind)!;
}

// ── The model ──────────────────────────────────────────────────────────────

export interface LinearTriggerConfig {
  team_id: string;
  team_name: string;
  state_id: string;
  state_name: string;
  linear_project_id?: string | null;
  linear_project_name?: string | null;
  /** Also start on issues with this label that nobody delegated. */
  label?: string | null;
  base_branch?: string | null;
  max_concurrent_runs: number;
}

export type WebhookAuth = "signature" | "token";

export interface WebhookFilter {
  /** `body.<a>.<b>` or `headers.<name>`. */
  path: string;
  equals: string[];
}

export interface WebhookTriggerConfig {
  auth: WebhookAuth;
  filters: WebhookFilter[];
  /** `{{body.x}}` / `{{headers.name}}` placeholders; empty = the payload. */
  task_template: string;
  delivery_header?: string | null;
  base_branch?: string | null;
}

interface RunTriggerBase {
  id: string;
  name: string;
  project: string;
  workflow: string;
  enabled: boolean;
  last_fired_at: string | null;
  last_run_id: string | null;
  /** Why the last attempt didn't start a run, until one does. */
  last_error: string | null;
  /**
   * An older Linear trigger on the same status, which fires instead of this
   * one. Only a pair saved before overlaps were refused can be in this state.
   */
  shadowed_by?: { id: string; name: string } | null;
  created_by: string | null;
  created_at: string;
  updated_at: string;
}

/** A row of `/api/triggers`. */
export type RunTrigger =
  | (RunTriggerBase & { kind: "linear"; config: LinearTriggerConfig })
  | (RunTriggerBase & {
      kind: "webhook";
      config: WebhookTriggerConfig;
      /** Where deliveries go; null without a public URL. */
      url: string | null;
    });

export interface TriggerBody {
  name: string;
  kind: "linear" | "webhook";
  project: string;
  workflow: string;
  enabled: boolean;
  config: LinearTriggerConfig | WebhookTriggerConfig;
}

/** Which system starts Linear runs. */
export type TriggerSystem = "classic" | "triggers";

// ── Data ───────────────────────────────────────────────────────────────────

export function useRunTriggers() {
  return useQuery<RunTrigger[], Error>({
    queryKey: ["triggers", "list"],
    queryFn: async ({ signal }) =>
      (await apiJson<{ triggers: RunTrigger[] }>("/api/triggers", { signal }))
        .triggers,
  });
}

export function useRunTrigger(id: string | null) {
  return useQuery<RunTrigger, Error>({
    queryKey: ["triggers", "one", id],
    enabled: !!id,
    queryFn: ({ signal }) =>
      apiJson<RunTrigger>(`/api/triggers/${encodeURIComponent(id!)}`, {
        signal,
      }),
  });
}

export function useSaveTrigger() {
  const qc = useQueryClient();
  return useMutation<
    { trigger: RunTrigger; secret: string | null },
    Error,
    { id?: string; body: TriggerBody }
  >({
    mutationFn: async ({ id, body }) => {
      const init = {
        method: id ? "PUT" : "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      };
      if (id) {
        const trigger = await apiJson<RunTrigger>(
          `/api/triggers/${encodeURIComponent(id)}`,
          init,
        );
        return { trigger, secret: null };
      }
      return apiJson("/api/triggers", init);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ["triggers"] }),
  });
}

export function useDeleteTrigger() {
  const qc = useQueryClient();
  return useMutation<void, Error, string>({
    // `apiFetch`: a 204 has no body to parse.
    mutationFn: async (id) => {
      await apiFetch(`/api/triggers/${encodeURIComponent(id)}`, {
        method: "DELETE",
      });
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ["triggers"] }),
  });
}

/** A new webhook secret; the old one stops working at once. */
export function useRotateSecret() {
  return useMutation<{ secret: string }, Error, string>({
    mutationFn: (id) =>
      apiJson(`/api/triggers/${encodeURIComponent(id)}/secret`, {
        method: "POST",
      }),
  });
}

/**
 * Which system starts Linear runs: each project's classic bindings, or the
 * Linear triggers here. Installation-wide; readable by anyone signed in.
 */
export function useTriggerSystem() {
  return useQuery<TriggerSystem, Error>({
    queryKey: ["triggers", "system"],
    queryFn: async ({ signal }) =>
      (
        await apiJson<{ system: TriggerSystem }>("/api/triggers/system", {
          signal,
        })
      ).system,
    staleTime: 60_000,
    refetchInterval: false,
  });
}

export function useSetTriggerSystem() {
  const qc = useQueryClient();
  return useMutation<{ system: TriggerSystem }, Error, TriggerSystem>({
    mutationFn: (system) =>
      apiJson("/api/triggers/system", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ system }),
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["triggers"] }),
  });
}

// ── One list ───────────────────────────────────────────────────────────────

/**
 * Whether a trigger will start anything. `idle` is a Linear trigger while the
 * installation still runs classic bindings: saved and enabled, not in charge.
 */
export type TriggerState = "on" | "off" | "idle";

export type Trigger =
  | { kind: "linear" | "webhook"; key: string; trigger: RunTrigger }
  | { kind: "schedule"; key: string; schedule: Schedule };

export function triggerName(t: Trigger): string {
  return t.kind === "schedule" ? t.schedule.name : t.trigger.name;
}
export function triggerProject(t: Trigger): string {
  return t.kind === "schedule" ? t.schedule.project : t.trigger.project;
}
export function triggerWorkflow(t: Trigger): string {
  return t.kind === "schedule" ? t.schedule.workflow : t.trigger.workflow;
}

export function triggerState(t: Trigger, system: TriggerSystem): TriggerState {
  const enabled =
    t.kind === "schedule" ? t.schedule.enabled : t.trigger.enabled;
  if (!enabled) return "off";
  if (t.kind === "linear" && system === "classic") return "idle";
  return "on";
}

/** Both stores as one list: by workflow, then project, then name. */
export function toTriggers(
  runTriggers: RunTrigger[],
  schedules: Schedule[],
): Trigger[] {
  const rows: Trigger[] = [
    ...runTriggers.map((t): Trigger => ({
      kind: t.kind,
      key: `${t.kind}/${t.id}`,
      trigger: t,
    })),
    ...schedules.map((s): Trigger => ({
      kind: "schedule",
      key: `schedule/${s.id}`,
      schedule: s,
    })),
  ];
  return rows.sort(
    (a, b) =>
      triggerWorkflow(a).localeCompare(triggerWorkflow(b)) ||
      triggerProject(a).localeCompare(triggerProject(b)) ||
      triggerName(a).localeCompare(triggerName(b)) ||
      a.key.localeCompare(b.key),
  );
}

/** What sets a trigger off, in words. */
export function describeWhen(t: Trigger): string {
  if (t.kind === "schedule") {
    return `${describeCron(t.schedule.cron) ?? t.schedule.cron} · ${t.schedule.timezone}`;
  }
  const r = t.trigger;
  if (r.kind === "linear") {
    const c = r.config;
    let out = `${c.team_name} · ${c.state_name}`;
    if (c.linear_project_id) {
      out += ` · only ${c.linear_project_name ?? "one Linear project"}`;
    }
    out += c.label ? ` · delegated or labelled ${c.label}` : " · delegated";
    return out;
  }
  const f = r.config.filters;
  return f.length === 0
    ? "any signed delivery"
    : f.map((x) => `${x.path} = ${x.equals.join(" | ")}`).join(" and ");
}

export interface TriggerFilter {
  kind?: TriggerKind | null;
  project?: string | null;
  workflow?: string | null;
  /** Free text; every word must appear somewhere in the trigger. */
  q?: string | null;
}

function haystack(t: Trigger): string {
  const parts = [
    t.kind,
    triggerName(t),
    triggerProject(t),
    triggerWorkflow(t),
    describeWhen(t),
  ];
  if (t.kind === "schedule") parts.push(t.schedule.cron, t.schedule.task);
  else if (t.trigger.kind === "webhook")
    parts.push(t.trigger.config.task_template);
  return parts.join(" ").toLowerCase();
}

export function filterTriggers(rows: Trigger[], f: TriggerFilter): Trigger[] {
  const terms = (f.q ?? "").toLowerCase().split(/\s+/).filter(Boolean);
  return rows.filter(
    (t) =>
      (!f.kind || t.kind === f.kind) &&
      (!f.project || triggerProject(t) === f.project) &&
      (!f.workflow || triggerWorkflow(t) === f.workflow) &&
      (terms.length === 0 || terms.every((term) => haystack(t).includes(term))),
  );
}

/** Where a trigger is edited. */
export function triggerHref(t: Trigger): string {
  return t.kind === "schedule"
    ? `/triggers/schedule/${encodeURIComponent(t.schedule.id)}`
    : `/triggers/${encodeURIComponent(t.trigger.id)}`;
}

/** Every trigger, from both stores. Loading until both have answered. */
export function useTriggers() {
  const runTriggers = useRunTriggers();
  const schedules = useSchedules();
  const isLoading = runTriggers.isLoading || schedules.isLoading;
  const rows = useMemo(
    () =>
      isLoading
        ? undefined
        : toTriggers(runTriggers.data ?? [], schedules.data?.schedules ?? []),
    [isLoading, runTriggers.data, schedules.data],
  );
  return {
    data: rows,
    isLoading,
    // A store that failed contributes no rows but doesn't hide the other's.
    errors: [runTriggers.error, schedules.error].filter(
      (e): e is Error => e != null,
    ),
  };
}
