/**
 * React Query hooks for the Linear trigger binding API.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";
import type {
  CreatedLinearIssue,
  CreateLinearIssueInput,
  LinearDiscovery,
  LinearSource,
  LinearSourceInput,
} from "@/types/linear";

/**
 * How the harness authenticates against Linear. One workspace, one install —
 * the credential is global (Credentials page), not per project.
 *
 * `app` = an `actor=app` OAuth install, so the harness's comments and status
 * moves are authored by the application. `personal_key` = the legacy API key,
 * which makes every write read as authored by whoever minted it. `none` = not
 * connected.
 */
export interface LinearOauthStatus {
  mode: "app" | "personal_key" | "none";
  workspace_name: string | null;
  workspace_url_key: string | null;
  token_scope: string | null;
  expires_at_ms: number | null;
  /** Last refresh failure — set means the workspace must be reconnected. */
  refresh_error: string | null;
  /** Whether an OAuth client_id + client_secret are stored (connect needs them). */
  client_configured: boolean;
  /** The redirect URL to register in the Linear OAuth app. */
  callback_url: string | null;
  /**
   * Whether the stored token carries `app:assignable` + `app:mentionable`. False
   * on an install made before those were requested: the poller still works, but
   * the app cannot be delegated to until it is reconnected.
   */
  agent_scopes_granted: boolean;
  /** Whether the webhook signing secret is stored (delegation needs it). */
  webhook_secret_configured: boolean;
  /** The URL to register as the OAuth app's webhook. */
  webhook_url: string | null;
  /** The app's own user id in the workspace. */
  app_user_id: string | null;
}

export function useLinearOauthStatus() {
  return useQuery<LinearOauthStatus, Error>({
    queryKey: ["linear", "oauth-status"],
    queryFn: ({ signal }) =>
      apiJson<LinearOauthStatus>("/api/linear/oauth/status", { signal }),
    retry: false,
  });
}

/**
 * Start the `actor=app` OAuth flow. The authorization URL comes back as JSON
 * (this request carries the API bearer token, which a plain navigation could
 * not), and the caller then sends the browser there.
 */
export function useConnectLinear() {
  return useMutation<{ url: string; callback_url: string }, Error, void>({
    mutationFn: () =>
      apiJson<{ url: string; callback_url: string }>("/api/linear/oauth/start"),
    onSuccess: ({ url }) => {
      window.location.assign(url);
    },
  });
}

/** Revoke the app token at Linear and clear it, keeping the OAuth client details. */
export function useDisconnectLinear() {
  const qc = useQueryClient();
  return useMutation<{ disconnected: boolean }, Error, void>({
    mutationFn: () =>
      apiJson<{ disconnected: boolean }>("/api/linear/oauth/disconnect", {
        method: "POST",
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["linear", "oauth-status"] });
      qc.invalidateQueries({ queryKey: ["credentials"] });
      qc.invalidateQueries({ queryKey: ["linear", "discovery"] });
    },
  });
}

export function useLinearDiscovery(project: string | null) {
  return useQuery<LinearDiscovery, Error>({
    queryKey: ["linear", "discovery", project],
    enabled: !!project,
    queryFn: ({ signal }) =>
      apiJson<LinearDiscovery>(
        `/api/projects/${encodeURIComponent(project!)}/linear/discovery`,
        { signal },
      ),
    retry: false,
    staleTime: 60_000,
  });
}

export function useLinearSources(project: string | null) {
  return useQuery<LinearSource[], Error>({
    queryKey: ["linear", "sources", project],
    enabled: !!project,
    queryFn: ({ signal }) =>
      apiJson<LinearSource[]>(
        `/api/projects/${encodeURIComponent(project!)}/linear-sources`,
        { signal },
      ),
  });
}

export function useLinearSource(
  project: string | null,
  workflow: string | null,
) {
  return useQuery<LinearSource | null, Error>({
    queryKey: ["linear", "source", project, workflow],
    enabled: !!project && !!workflow,
    queryFn: ({ signal }) =>
      apiJson<LinearSource | null>(
        `/api/projects/${encodeURIComponent(project!)}/linear-source?workflow=${encodeURIComponent(workflow!)}`,
        { signal },
      ),
  });
}

/** Create a Linear issue from a task/finding against the project's binding. */
export function useCreateLinearIssue(project: string | null) {
  return useMutation<CreatedLinearIssue, Error, CreateLinearIssueInput>({
    mutationFn: (body) =>
      apiJson<CreatedLinearIssue>(
        `/api/projects/${encodeURIComponent(project!)}/linear-issues`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        },
      ),
  });
}

export function useSaveLinearSource(project: string | null) {
  const qc = useQueryClient();
  return useMutation<LinearSource, Error, LinearSourceInput>({
    mutationFn: (body) =>
      apiJson<LinearSource>(
        `/api/projects/${encodeURIComponent(project!)}/linear-source`,
        {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        },
      ),
    onSuccess: (_data, variables) => {
      qc.invalidateQueries({
        queryKey: ["linear", "source", project, variables.workflow],
      });
      qc.invalidateQueries({ queryKey: ["linear", "sources", project] });
    },
  });
}

export function useDeleteLinearSource(project: string | null) {
  const qc = useQueryClient();
  return useMutation<{ deleted: boolean; workflow: string }, Error, string>({
    mutationFn: (workflow) =>
      apiJson<{ deleted: boolean; workflow: string }>(
        `/api/projects/${encodeURIComponent(project!)}/linear-source?workflow=${encodeURIComponent(workflow)}`,
        { method: "DELETE" },
      ),
    onSuccess: (_data, workflow) => {
      qc.invalidateQueries({
        queryKey: ["linear", "source", project, workflow],
      });
      qc.invalidateQueries({ queryKey: ["linear", "sources", project] });
    },
  });
}
