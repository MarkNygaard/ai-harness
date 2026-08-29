/**
 * Signing in with an identity provider.
 *
 * One implementation for anything speaking OIDC discovery — Entra, Google,
 * Okta, Keycloak, Authentik — configured by issuer URL.
 */
import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";

export interface SsoOutcome {
  /** "tested", "error", "denied" — whatever the callback reported. */
  status: string;
  message: string | null;
}

/**
 * The outcome of a provider round trip, taken off the URL and then removed
 * from it.
 *
 * Every page a callback can land on needs this. It used to live only on the
 * settings page, so a refused *sign-in* — which lands on /login, not settings
 * — dropped the explanation entirely and showed the person the form again with
 * nothing to say why. Shared, so the two cannot drift apart again.
 *
 * The parameters are stripped with `replaceState` so a reload does not
 * resurrect a stale error, and so the address is not left carrying someone's
 * email around.
 */
export function useSsoOutcome(): SsoOutcome | null {
  const [outcome, setOutcome] = useState<SsoOutcome | null>(null);
  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const status = params.get("sso");
    if (!status) return;
    setOutcome({ status, message: params.get("sso_message") });
    params.delete("sso");
    params.delete("sso_message");
    const query = params.toString();
    window.history.replaceState(
      {},
      "",
      `${window.location.pathname}${query ? `?${query}` : ""}`,
    );
  }, []);
  return outcome;
}

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

// ── GitHub ───────────────────────────────────────────────────────────────────
//
// A separate provider, not a variant of the OIDC one: no ID token, and
// organisation membership rather than an email domain is the allowlist.

/** Where the allowlist comes from. Never "anybody with a GitHub account". */
export type GithubAudience = "org" | "existing";

export interface GithubSsoConfig {
  client_id: string | null;
  client_secret_set: boolean;
  audience: GithubAudience;
  org: string | null;
  team: string | null;
  enabled: boolean;
  callback_url: string | null;
}

export function useGithubSsoConfig(enabled: boolean) {
  return useQuery<GithubSsoConfig, Error>({
    queryKey: ["sso", "github", "config"],
    enabled,
    queryFn: ({ signal }) =>
      apiJson<GithubSsoConfig>("/api/settings/sso/github", { signal }),
    retry: false,
    refetchInterval: false,
  });
}

export type GithubSsoInput = Partial<{
  client_id: string;
  /** Omit to leave the stored one alone. */
  client_secret: string;
  audience: GithubAudience;
  org: string;
  team: string;
}>;

export function useSaveGithubSso() {
  const qc = useQueryClient();
  return useMutation<GithubSsoConfig, Error, GithubSsoInput>({
    mutationFn: (body) =>
      apiJson<GithubSsoConfig>("/api/settings/sso/github", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: (data) => {
      qc.setQueryData(["sso", "github", "config"], data);
      qc.invalidateQueries({ queryKey: ["sso", "github", "public"] });
    },
  });
}

export function useTestGithubSso() {
  return useMutation<{ url: string }, Error, void>({
    mutationFn: () =>
      apiJson<{ url: string }>("/api/settings/sso/github/test", {
        method: "POST",
      }),
    onSuccess: ({ url }) => window.location.assign(url),
  });
}

export function useGithubSsoPublicStatus() {
  return useQuery<{ enabled: boolean }, Error>({
    queryKey: ["sso", "github", "public"],
    queryFn: ({ signal }) =>
      apiJson<{ enabled: boolean }>("/api/auth/github/status", { signal }),
    retry: false,
    refetchInterval: false,
  });
}

export function useStartGithubSso() {
  return useMutation<{ url: string }, Error, { next?: string } | void>({
    mutationFn: (vars) => {
      const next = vars && "next" in vars ? vars.next : undefined;
      const query = next ? `?next=${encodeURIComponent(next)}` : "";
      return apiJson<{ url: string }>(`/api/auth/github/start${query}`);
    },
    onSuccess: ({ url }) => window.location.assign(url),
  });
}
