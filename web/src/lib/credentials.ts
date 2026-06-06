/**
 * Data layer for provider credentials (`/api/credentials`). The list endpoint
 * only reports whether each provider is configured — secret values are never
 * returned, so the editor form is always blank and only ever *sends* values.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";

export interface ProviderCredential {
  provider: string;
  configured: boolean;
}

export function useCredentials() {
  return useQuery<ProviderCredential[], Error>({
    queryKey: ["credentials"],
    queryFn: ({ signal }) =>
      apiJson<ProviderCredential[]>("/api/credentials", { signal }),
  });
}

export function useSetCredential() {
  const qc = useQueryClient();
  return useMutation<
    { saved: boolean; provider: string },
    Error,
    { provider: string; fields: Record<string, string> }
  >({
    mutationFn: ({ provider, fields }) =>
      apiJson(`/api/credentials/${encodeURIComponent(provider)}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ fields }),
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["credentials"] }),
  });
}

// ---------------------------------------------------------------------------
// Per-project credentials (`/api/projects/{project}/credentials`).
//
// Project-scoped keys override the global ones for that project (with the
// global value as fallback). The server allowlists which providers may be set
// per project. As with the global API, secret values are never returned —
// `configured` only reports presence.
// ---------------------------------------------------------------------------

/** Providers that can be overridden per project, and the field each expects. */
export const PROJECT_CREDENTIALS: { provider: string; field: string }[] = [
  { provider: "linear", field: "api_key" },
  { provider: "github", field: "token" },
];

export function useProjectCredentials(project: string | null) {
  return useQuery<ProviderCredential[], Error>({
    queryKey: ["project-credentials", project],
    enabled: !!project,
    queryFn: ({ signal }) =>
      apiJson<ProviderCredential[]>(
        `/api/projects/${encodeURIComponent(project!)}/credentials`,
        { signal },
      ),
  });
}

export function useSetProjectCredential(project: string | null) {
  const qc = useQueryClient();
  return useMutation<
    unknown,
    Error,
    { provider: string; fields: Record<string, string> }
  >({
    mutationFn: ({ provider, fields }) =>
      apiJson(
        `/api/projects/${encodeURIComponent(project!)}/credentials/${encodeURIComponent(provider)}`,
        {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ fields }),
        },
      ),
    onSuccess: (_data, { provider }) => {
      qc.invalidateQueries({ queryKey: ["project-credentials", project] });
      // Linear discovery uses the key — refetch so trigger dropdowns populate.
      if (provider === "linear") {
        qc.invalidateQueries({ queryKey: ["linear", "discovery", project] });
      }
    },
  });
}

export function useDeleteProjectCredential(project: string | null) {
  const qc = useQueryClient();
  return useMutation<unknown, Error, string>({
    mutationFn: (provider) =>
      apiJson(
        `/api/projects/${encodeURIComponent(project!)}/credentials/${encodeURIComponent(provider)}`,
        { method: "DELETE" },
      ),
    onSuccess: (_data, provider) => {
      qc.invalidateQueries({ queryKey: ["project-credentials", project] });
      if (provider === "linear") {
        qc.invalidateQueries({ queryKey: ["linear", "discovery", project] });
      }
    },
  });
}

/** Kimi-for-Coding OAuth device-login (server-driven). */
export interface KimiConnectStart {
  user_code: string;
  verification_uri: string;
  device_code: string;
  interval: number;
  expires_in: number;
}

export interface KimiPoll {
  status: "pending" | "connected" | "error";
  message?: string;
}

export function startKimiConnect(): Promise<KimiConnectStart> {
  return apiJson<KimiConnectStart>("/api/credentials/kimi/connect/start", {
    method: "POST",
  });
}

export function pollKimiConnect(deviceCode: string): Promise<KimiPoll> {
  return apiJson<KimiPoll>("/api/credentials/kimi/connect/poll", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ device_code: deviceCode }),
  });
}

/** Codex (ChatGPT) OAuth browser/PKCE login (server-driven, paste-redirect). */
export interface CodexConnectStart {
  authorize_url: string;
  state: string;
  verifier: string;
  redirect_uri: string;
}

export function startCodexConnect(): Promise<CodexConnectStart> {
  return apiJson<CodexConnectStart>("/api/credentials/codex/connect/start", {
    method: "POST",
  });
}

/** Exchange the pasted redirect URL (or bare code) for tokens. */
export function completeCodexConnect(
  redirect: string,
  state: string,
  verifier: string,
): Promise<KimiPoll> {
  return apiJson<KimiPoll>("/api/credentials/codex/connect/complete", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ redirect, state, verifier }),
  });
}

export function useDeleteCredential() {
  const qc = useQueryClient();
  return useMutation<{ deleted: boolean; provider: string }, Error, string>({
    mutationFn: (provider) =>
      apiJson(`/api/credentials/${encodeURIComponent(provider)}`, {
        method: "DELETE",
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["credentials"] }),
  });
}
