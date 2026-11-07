/**
 * Triggers: everything that starts a run on its own.
 *
 * A trigger is a *source* (what happens) plus a *target* (the project and
 * workflow it starts). Today there are two sources, each with its own store and
 * API — Linear bindings and schedules — and this module is where they become
 * one list. It owns no data of its own: the rows are the same ones the Linear
 * and schedule hooks return, wrapped in a common shape.
 *
 * Merged here rather than behind a server endpoint on purpose. The one thing a
 * server-side list would add is a Linear binding's status *name*, which needs a
 * Linear discovery call — the query that exhausted the workspace's rate limit
 * when a page polled it. The list resolves names per project instead, through
 * the cached, unpolled discovery hook.
 */
import { useMemo } from "react";
import type { LinearSource, LinearTeam } from "@/types/linear";
import { useAllLinearSources } from "./linear";
import { describeCron, type Schedule, useSchedules } from "./schedules";

export type TriggerKind = "linear" | "schedule";

/**
 * Display metadata per kind, in the order the kind picker offers them. The
 * icons live in `TriggerKindIcon`, so this module stays free of components.
 */
export const TRIGGER_KINDS: {
  kind: TriggerKind;
  label: string;
  description: string;
}[] = [
  {
    kind: "linear",
    label: "Linear",
    description:
      "Start a run when an issue in a team's status is delegated to the harness, or carries a label.",
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

/**
 * Whether a trigger will start anything. `dry-run` is a Linear binding that is
 * enabled but not live: it logs what it would claim and starts nothing.
 */
export type TriggerState = "on" | "dry-run" | "off";

interface TriggerBase {
  /** Unique across kinds and stable across reloads. */
  key: string;
  project: string;
  workflow: string;
  state: TriggerState;
}

export type Trigger =
  | (TriggerBase & { kind: "linear"; binding: LinearSource })
  | (TriggerBase & { kind: "schedule"; schedule: Schedule });

export function fromBinding(b: LinearSource): Trigger {
  return {
    kind: "linear",
    key: `linear/${b.project}/${b.workflow}`,
    project: b.project,
    workflow: b.workflow,
    state: !b.enabled ? "off" : b.live ? "on" : "dry-run",
    binding: b,
  };
}

export function fromSchedule(s: Schedule): Trigger {
  return {
    kind: "schedule",
    key: `schedule/${s.id}`,
    project: s.project,
    workflow: s.workflow,
    state: s.enabled ? "on" : "off",
    schedule: s,
  };
}

/**
 * Both sources as one list: by workflow, then project, then kind. Not grouped
 * by project — what a trigger *starts* is the first thing anyone scans for.
 */
export function toTriggers(
  bindings: LinearSource[],
  schedules: Schedule[],
): Trigger[] {
  return [...bindings.map(fromBinding), ...schedules.map(fromSchedule)].sort(
    (a, b) =>
      a.workflow.localeCompare(b.workflow) ||
      a.project.localeCompare(b.project) ||
      a.kind.localeCompare(b.kind) ||
      a.key.localeCompare(b.key),
  );
}

export interface TriggerFilter {
  kind?: TriggerKind | null;
  project?: string | null;
  workflow?: string | null;
  /** Free text; every word must appear somewhere in the trigger. */
  q?: string | null;
}

function haystack(t: Trigger): string {
  const parts = [t.kind, t.project, t.workflow];
  if (t.kind === "linear") {
    const b = t.binding;
    parts.push(
      b.team_name,
      b.trigger_label ?? "",
      b.linear_project_name ?? "",
      b.base_branch ?? "",
    );
  } else {
    const s = t.schedule;
    parts.push(s.name, s.cron, describeCron(s.cron) ?? "", s.timezone, s.task);
  }
  return parts.join(" ").toLowerCase();
}

export function filterTriggers(rows: Trigger[], f: TriggerFilter): Trigger[] {
  const terms = (f.q ?? "").toLowerCase().split(/\s+/).filter(Boolean);
  return rows.filter(
    (t) =>
      (!f.kind || t.kind === f.kind) &&
      (!f.project || t.project === f.project) &&
      (!f.workflow || t.workflow === f.workflow) &&
      (terms.length === 0 || terms.every((term) => haystack(t).includes(term))),
  );
}

/**
 * What sets a Linear binding off, in words: the team and status, narrowed by
 * Linear project, and whether a label also starts it. `teams` resolves the
 * status id to its name; without it (discovery not loaded or failed) the
 * sentence still reads, just less specifically.
 */
export function describeLinearWhen(
  b: LinearSource,
  teams: LinearTeam[] | undefined,
): string {
  const state = teams
    ?.find((t) => t.id === b.team_id)
    ?.states.find((s) => s.id === b.source_state_id)?.name;
  let out = `${b.team_name} · ${state ?? "its start status"}`;
  if (b.linear_project_id) {
    out += ` · only ${b.linear_project_name ?? "one Linear project"}`;
  }
  out += b.trigger_label
    ? ` · delegated or labelled ${b.trigger_label}`
    : " · delegated";
  return out;
}

/** A schedule's timing, in words when it matches a preset. */
export function describeScheduleWhen(s: Schedule): string {
  return `${describeCron(s.cron) ?? s.cron} · ${s.timezone}`;
}

/**
 * Where a trigger is edited.
 *
 * `linearHome` is the per-browser choice between this page and the project's
 * own Linear tab (see `trigger-prefs`). Schedules only ever live here.
 */
export function triggerHref(
  t: Trigger,
  linearHome: "triggers" | "project",
): string {
  if (t.kind === "schedule") {
    return `/triggers/schedule/${encodeURIComponent(t.schedule.id)}`;
  }
  return linearHome === "project"
    ? `/settings/projects/${encodeURIComponent(t.project)}/linear`
    : `/triggers/linear/${encodeURIComponent(t.project)}/${encodeURIComponent(t.workflow)}`;
}

/**
 * Every trigger, from both sources. Loading until both have answered; a source
 * that failed contributes no rows but doesn't hide the other's.
 */
export function useTriggers() {
  const linear = useAllLinearSources();
  const schedules = useSchedules();
  const isLoading = linear.isLoading || schedules.isLoading;
  const rows = useMemo(
    () =>
      isLoading
        ? undefined
        : toTriggers(linear.data ?? [], schedules.data?.schedules ?? []),
    [isLoading, linear.data, schedules.data],
  );
  return {
    data: rows,
    isLoading,
    errors: [linear.error, schedules.error].filter(
      (e): e is Error => e != null,
    ),
  };
}
