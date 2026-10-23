/**
 * Scheduled runs: a workflow started on a cron schedule, with no Linear issue
 * behind it. Anyone can see them; only administrators change them.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./api";

export interface Schedule {
  id: string;
  name: string;
  project: string;
  workflow: string;
  /** Five-field cron, read in `timezone`. */
  cron: string;
  /** IANA zone, e.g. `Europe/Copenhagen`. */
  timezone: string;
  /** What the run is asked to do: its `$ARGUMENTS`. */
  task: string;
  enabled: boolean;
  next_run_at: string | null;
  last_run_at: string | null;
  last_run_id: string | null;
  created_by: string | null;
  created_at: string;
  updated_at: string;
}

export interface ScheduleInput {
  name: string;
  project: string;
  workflow: string;
  cron: string;
  timezone: string;
  task: string;
  enabled: boolean;
}

/** The schedules most people want, so nobody has to write cron for them. */
export const PRESETS: { label: string; cron: string }[] = [
  { label: "Every Monday at 08:00", cron: "0 8 * * 1" },
  { label: "Every weekday at 07:00", cron: "0 7 * * 1-5" },
  { label: "Every day at 07:00", cron: "0 7 * * *" },
  { label: "First of the month at 08:00", cron: "0 8 1 * *" },
  { label: "Every hour", cron: "0 * * * *" },
];

/** A preset's label when `cron` is one, so the list reads in words. */
export function describeCron(cron: string): string | null {
  const normal = cron.trim().split(/\s+/).join(" ");
  return PRESETS.find((p) => p.cron === normal)?.label ?? null;
}

/** The browser's timezone, the default for a new schedule. */
export function browserTimezone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
}

export function useSchedules() {
  return useQuery<{ schedules: Schedule[] }, Error>({
    queryKey: ["schedules"],
    queryFn: ({ signal }) =>
      apiJson<{ schedules: Schedule[] }>("/api/schedules", { signal }),
  });
}

/**
 * The next times an expression fires, or why it is invalid. Only asked once
 * there is something to ask about.
 */
export function useSchedulePreview(cron: string, timezone: string) {
  const ready = cron.trim().split(/\s+/).length === 5 && !!timezone.trim();
  return useQuery<{ next: string[] }, Error>({
    queryKey: ["schedules", "preview", cron.trim(), timezone.trim()],
    enabled: ready,
    retry: false,
    refetchInterval: false,
    queryFn: ({ signal }) =>
      apiJson<{ next: string[] }>("/api/schedules/preview", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ cron: cron.trim(), timezone: timezone.trim() }),
        signal,
      }),
  });
}

export function useSaveSchedule() {
  const qc = useQueryClient();
  return useMutation<Schedule, Error, { id?: string; input: ScheduleInput }>({
    mutationFn: ({ id, input }) =>
      apiJson<Schedule>(id ? `/api/schedules/${id}` : "/api/schedules", {
        method: id ? "PUT" : "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(input),
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["schedules"] }),
  });
}

export function useDeleteSchedule() {
  const qc = useQueryClient();
  return useMutation<void, Error, string>({
    // `apiFetch`: a 204 has no body to parse.
    mutationFn: async (id) => {
      await apiFetch(`/api/schedules/${id}`, { method: "DELETE" });
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ["schedules"] }),
  });
}

export function useRunScheduleNow() {
  const qc = useQueryClient();
  return useMutation<{ run_id: string }, Error, string>({
    mutationFn: (id) =>
      apiJson<{ run_id: string }>(`/api/schedules/${id}/run`, {
        method: "POST",
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["schedules"] });
      qc.invalidateQueries({ queryKey: ["runs"] });
    },
  });
}
