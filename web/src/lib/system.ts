/**
 * System maintenance data layer (`/api/system/*`) — currently the Claude Code
 * CLI version check and in-app update. The update installs into the container's
 * persistent `$HOME/.local`, so it survives restarts on a volume-backed home.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";

export interface ClaudeVersionInfo {
  /** Version of the on-PATH `claude` binary, e.g. "2.1.223". */
  installed: string | null;
  /** Latest version on npm; null when the registry was unreachable. */
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

export function useClaudeCodeVersion() {
  return useQuery<ClaudeVersionInfo, Error>({
    queryKey: ["claude-code-version"],
    queryFn: ({ signal }) =>
      apiJson<ClaudeVersionInfo>("/api/system/claude-version", { signal }),
    // The npm-registry lookup is slow-changing and hits the network — opt out
    // of the app-wide 5s refetch; check on mount and keep it fresh for 30m.
    refetchInterval: false,
    staleTime: 1000 * 60 * 30,
  });
}

export function useUpdateClaudeCode() {
  const qc = useQueryClient();
  return useMutation<ClaudeUpdateResult, Error, void>({
    mutationFn: () =>
      apiJson<ClaudeUpdateResult>("/api/system/claude-update", {
        method: "POST",
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["claude-code-version"] });
    },
  });
}
