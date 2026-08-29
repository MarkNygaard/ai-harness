/**
 * Signing in with an identity provider.
 *
 * One implementation for anything speaking OIDC discovery — Entra, Google,
 * Okta, Keycloak, Authentik — configured by issuer URL.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";

/** What the sign-in page may know before anybody has signed in. */
export interface SsoPublicStatus {
  /** Only an armed provider is offered; a half-configured one is a button that always fails. */
  enabled: boolean;
  label: string | null;
}

export function useSsoPublicStatus() {
  return useQuery<SsoPublicStatus, Error>({
    queryKey: ["sso", "public"],
    queryFn: ({ signal }) =>
      apiJson<SsoPublicStatus>("/api/auth/oidc/status", { signal }),
    retry: false,
    refetchInterval: false,
  });
}

export interface SsoConfig {
  issuer: string | null;
  client_id: string | null;
  client_secret_set: boolean;
  allowed_domains: string | null;
  label: string | null;
  enabled: boolean;
  /** Register this with the provider, exactly. */
  callback_url: string | null;
}

export function useSsoConfig(enabled: boolean) {
  return useQuery<SsoConfig, Error>({
    queryKey: ["sso", "config"],
    enabled,
    queryFn: ({ signal }) =>
      apiJson<SsoConfig>("/api/settings/sso", { signal }),
    retry: false,
    refetchInterval: false,
  });
}

export type SsoInput = Partial<{
  issuer: string;
  client_id: string;
  /** Omit to leave the stored one alone. */
  client_secret: string;
  allowed_domains: string;
  label: string;
  enabled: boolean;
}>;

export function useSaveSso() {
  const qc = useQueryClient();
  return useMutation<SsoConfig, Error, SsoInput>({
    mutationFn: (body) =>
      apiJson<SsoConfig>("/api/settings/sso", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: (data) => {
      qc.setQueryData(["sso", "config"], data);
      qc.invalidateQueries({ queryKey: ["sso", "public"] });
    },
  });
}

/**
 * Start a round trip that proves the settings work.
 *
 * The provider is armed by the callback, not by this — "these settings look
 * right" and "this works" are different claims.
 */
export function useTestSso() {
  return useMutation<{ url: string }, Error, void>({
    mutationFn: () =>
      apiJson<{ url: string }>("/api/settings/sso/test", { method: "POST" }),
    onSuccess: ({ url }) => window.location.assign(url),
  });
}

/** Begin signing in. */
export function useStartSso() {
  return useMutation<{ url: string }, Error, { next?: string } | void>({
    mutationFn: (vars) => {
      const next = vars && "next" in vars ? vars.next : undefined;
      const query = next ? `?next=${encodeURIComponent(next)}` : "";
      return apiJson<{ url: string }>(`/api/auth/oidc/start${query}`);
    },
    onSuccess: ({ url }) => window.location.assign(url),
  });
}
