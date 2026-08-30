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
  /** Latest on npm. Claude Code only, since it is the one we can install. */
  latest: string | null;
  update_available: boolean;
  /** Set when the latest-version lookup failed (e.g. no egress). */
  error: string | null;
}

export interface ClaudeUpdateResult {
  ok: boolean;
  installed: string | null;
  latest: string | null;
  update_available: boolean;
  /** Install log on success, error detail on failure. */
  message: string;
}

/**
 * Per-provider CLI presence and version.
 *
 * Each entry costs a process spawn on the server, and the npm lookup hits the
 * network, so this opts out of the app-wide 5s refetch: checked on mount, kept
 * fresh for 30 minutes. Nothing here changes without a deploy or an update.
 */
export function useProviderHealth() {
  return useQuery<ProviderHealth[], Error>({
    queryKey: ["provider-health"],
    queryFn: ({ signal }) =>
      apiJson<ProviderHealth[]>("/api/system/providers", { signal }),
    refetchInterval: false,
    staleTime: 1000 * 60 * 30,
  });
}

/**
 * Install the latest CLI for one provider.
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
    },
  });
}
