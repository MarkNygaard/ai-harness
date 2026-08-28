/**
 * React Query hooks for the Linear trigger binding API.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";
import type {
  CreatedLinearIssue,
  CreateLinearIssueInput,
  LinearConnection,
  LinearDiscovery,
  LinearSource,
  LinearSourceInput,
} from "@/types/linear";
import type { Project } from "@/types/project";

/**
 * `?connection=<id>` for the routes that act on one account. Omitted for the
 * default connection, so a single-account install sends the same requests it
 * always did.
 */
function connectionQuery(connection?: string): string {
  return connection && connection !== "default"
    ? `?connection=${encodeURIComponent(connection)}`
    : "";
}

/** Every connected Linear account, and which projects use each. */
export function useLinearConnections() {
  return useQuery<LinearConnection[], Error>({
    queryKey: ["linear", "connections"],
    queryFn: ({ signal }) =>
      apiJson<LinearConnection[]>("/api/linear/connections", { signal }),
    retry: false,
  });
}

/**
 * Add an account. This only creates it — the OAuth app details are saved
 * against it next, and then it is connected.
 */
export function useCreateLinearConnection() {
  const qc = useQueryClient();
  return useMutation<LinearConnection, Error, { label: string }>({
    mutationFn: (body) =>
      apiJson<LinearConnection>("/api/linear/connections", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["linear", "connections"] });
      // Adding a second account pins the projects that were using the first.
      qc.invalidateQueries({ queryKey: ["projects"] });
    },
  });
}

/** Revoke an account's token and remove it. Refused while projects use it. */
export function useDeleteLinearConnection() {
  const qc = useQueryClient();
  return useMutation<{ deleted: boolean; connection: string }, Error, string>({
    mutationFn: (id) =>
      apiJson<{ deleted: boolean; connection: string }>(
        `/api/linear/connections/${encodeURIComponent(id)}`,
        { method: "DELETE" },
      ),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["linear", "connections"] });
      qc.invalidateQueries({ queryKey: ["credentials"] });
    },
  });
}

/** Point a project's Linear traffic at an account (`null` = resolve automatically). */
export function useSetProjectLinearConnection(project: string | null) {
  const qc = useQueryClient();
  return useMutation<Project, Error, string | null>({
    mutationFn: (connection) =>
      apiJson<Project>(
        `/api/projects/${encodeURIComponent(project!)}/linear-connection`,
        {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ connection }),
        },
      ),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["projects"] });
      qc.invalidateQueries({ queryKey: ["linear", "connections"] });
      // The team list belongs to the account, so it has to be re-fetched.
      qc.invalidateQueries({ queryKey: ["linear", "discovery", project] });
    },
  });
}

/**
 * How one connected Linear account authenticates, and what it still needs.
 *
 * `app` = an `actor=app` OAuth install, so the harness's comments and status
 * moves are authored by the application. `personal_key` = the legacy API key,
 * which makes every write read as authored by whoever minted it. `none` = not
 * connected.
 */
export interface LinearOauthStatus {
  /** Which connection this describes. */
  connection: string;
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

export function useLinearOauthStatus(connection?: string) {
  return useQuery<LinearOauthStatus, Error>({
    queryKey: ["linear", "oauth-status", connection ?? "default"],
    queryFn: ({ signal }) =>
      apiJson<LinearOauthStatus>(
        `/api/linear/oauth/status${connectionQuery(connection)}`,
        { signal },
      ),
    retry: false,
  });
}

/**
 * Start the `actor=app` OAuth flow. The authorization URL comes back as JSON
 * (this request carries the API bearer token, which a plain navigation could
 * not), and the caller then sends the browser there.
 */
export function useConnectLinear(connection?: string) {
  return useMutation<{ url: string; callback_url: string }, Error, void>({
    mutationFn: () =>
      apiJson<{ url: string; callback_url: string }>(
        `/api/linear/oauth/start${connectionQuery(connection)}`,
      ),
    onSuccess: ({ url }) => {
      window.location.assign(url);
    },
  });
}

/** Revoke the app token at Linear and clear it, keeping the OAuth client details. */
export function useDisconnectLinear(connection?: string) {
  const qc = useQueryClient();
  return useMutation<{ disconnected: boolean }, Error, void>({
    mutationFn: () =>
      apiJson<{ disconnected: boolean }>(
        `/api/linear/oauth/disconnect${connectionQuery(connection)}`,
        { method: "POST" },
      ),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["linear", "oauth-status"] });
      qc.invalidateQueries({ queryKey: ["linear", "connections"] });
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
