/**
 * System maintenance data layer (`/api/system/*`) — which agent CLIs the image
 * actually has, what version each reports, and the in-app Claude Code update. The update installs into the container's
 * persistent `$HOME/.local`, so it survives restarts on a volume-backed home.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";

export interface ProviderHealth {
  /** Credential-store key: "claude", "codex", "pi", "cursor". */
  provider: string;
  /** The executable the harness spawns for it. */
  binary: string;
  /** Whether that executable resolves on PATH inside the container. */
  on_path: boolean;
  /** Version it reports, when it is there to be asked. */
  version: string | null;
  /** Latest on npm. Only for the CLIs installed from there. */
  latest: string | null;
  update_available: boolean;
  /**
   * This CLI can be installed from here at all.
   *
   * Distinct from `update_available`, which also means we know something newer
   * exists. `cursor-agent` comes from a vendor script with no version endpoint,
   * so it can be reinstalled but never announced as out of date.
   */
  reinstallable: boolean;
  /** Set when the latest-version lookup failed (e.g. no egress). */
  error: string | null;
}

export interface ClaudeUpdateResult {
  ok: boolean;
  installed: string | null;
  latest: string | null;
  update_available: boolean;
  /**
   * Nothing was installed because runs are in flight — it is queued for the
   * next idle moment instead. `ok` is still true: the request succeeded.
   */
  queued: boolean;
  /** Install log on success, error detail on failure, plan when queued. */
  message: string;
}

/** One finished queued update, kept for a day so the page can mention it. */
export interface CompletedCliUpdate {
  provider: string;
  ok: boolean;
  version: string | null;
  /** Failure detail; null when it worked. */
  message: string | null;
  /** RFC 3339. */
  at: string;
}

/** What the update queue is doing right now. */
export interface CliUpdateStatus {
  /** Runs in flight anywhere — this replica and every other. */
  active_runs: number;
  /** An install is underway, here or on another replica. */
  installing: boolean;
  /** Providers waiting for the cluster to go idle. */
  pending: string[];
  /** What the queue did recently, newest first. */
  completed: CompletedCliUpdate[];
  /** Set when the queue's own state could not be read (no database). */
  error: string | null;
}

/**
 * Per-provider CLI presence and version.
 *
 * Each entry costs a process spawn on the server, and the npm lookup hits the
 * network, so this opts out of the app-wide 5s refetch: checked on mount, kept
 * fresh for 30 minutes. Nothing here changes without a deploy or an update.
 *
 * `enabled` exists because the route is admin-only: the app-wide update notice
 * mounts on every page, and asking as a member would spend a rejected request
 * per page load to learn nothing.
 */
export function useProviderHealth(enabled = true) {
  return useQuery<ProviderHealth[], Error>({
    queryKey: ["provider-health"],
    queryFn: ({ signal }) =>
      apiJson<ProviderHealth[]>("/api/system/providers", { signal }),
    refetchInterval: false,
    staleTime: 1000 * 60 * 30,
    enabled,
  });
}

/**
 * What the update queue is doing: runs in flight, an install underway, what is
 * waiting for idle, and what it last did.
 *
 * Separate from `useProviderHealth` because it moves on a different clock — a
 * version only changes when someone installs one, while the run count changes
 * constantly — so this takes the app-wide refetch and that one does not.
 */
export function useCliUpdateStatus(enabled = true) {
  return useQuery<CliUpdateStatus, Error>({
    queryKey: ["cli-update-status"],
    queryFn: ({ signal }) =>
      apiJson<CliUpdateStatus>("/api/system/cli-update", { signal }),
    enabled,
  });
}

/**
 * Install the latest CLI for one provider — or, when runs are in flight, queue
 * it for the moment the last one finishes.
 *
 * The server decides which: an `npm install` replaces the package tree under
 * whatever is running, so it will not do that while an agent is live. Check
 * `result.queued` to know which happened.
 *
 * Claude Code and Codex are both npm packages installed the same way, so one
 * mutation covers both; `omp` and `cursor-agent` come from elsewhere and report
 * `update_available: false`, so no button is ever offered for them.
 */
export function useUpdateAgentCli() {
  const qc = useQueryClient();
  return useMutation<ClaudeUpdateResult, Error, string>({
    mutationFn: (provider: string) =>
      apiJson<ClaudeUpdateResult>(
        `/api/system/cli-update/${encodeURIComponent(provider)}`,
        { method: "POST" },
      ),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["provider-health"] });
      qc.invalidateQueries({ queryKey: ["cli-update-status"] });
    },
  });
}

/** Drop a queued update, for whoever changed their mind before it ran. */
export function useCancelAgentCliUpdate() {
  const qc = useQueryClient();
  return useMutation<CliUpdateStatus, Error, string>({
    mutationFn: (provider: string) =>
      apiJson<CliUpdateStatus>(
        `/api/system/cli-update/${encodeURIComponent(provider)}`,
        { method: "DELETE" },
      ),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["cli-update-status"] });
    },
  });
}

/**
 * Whether to offer the update/reinstall button for one agent CLI.
 *
 * `reinstallable` on its own is not a reason. It is true of every CLI the
 * harness knows how to install, so OR-ing it into the condition put an
 * "Update to 2.1.268" button next to `v2.1.268` — an update to the version
 * already installed, which never went away however many times you pressed it.
 *
 * It earns the button only when there is no `latest` to compare against. That
 * is the vendor-script case: Cursor publishes no version endpoint, so without
 * this it would be stuck on whatever the image baked in with no way to move it
 * short of a rebuild. The button then reads "Reinstall" rather than claiming an
 * update nobody checked for.
 *
 * Split out from the JSX because the bug it fixes was invisible in review and
 * obvious in a screenshot.
 */
export function offersCliUpdate(
  health: ProviderHealth | undefined,
  queue: CliUpdateStatus | undefined,
  provider: string,
): boolean {
  if (queue?.pending.includes(provider)) return true;
  if (health?.update_available) return true;
  return Boolean(health?.reinstallable && !health?.latest);
}
