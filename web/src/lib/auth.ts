/**
 * Who is signed in, and how this harness decides that.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";

/**
 * `open` and `token` are what the harness has always done — no accounts, and
 * the shared bearer token respectively. `accounts` means named people sign in.
 */
export type AuthMode = "open" | "token" | "accounts";

export interface AuthUser {
  id: string;
  email: string;
  name: string;
  role: "admin" | "member";
  created_at: string;
  last_login_at: string | null;
  disabled_at: string | null;
}

export interface AuthStatus {
  mode: AuthMode;
  /** False on an install nobody has claimed — the case that sends you to /setup. */
  claimed: boolean;
  user: AuthUser | null;
  min_password_len: number;
}

export function useAuthStatus() {
  return useQuery<AuthStatus, Error>({
    queryKey: ["auth", "status"],
    queryFn: ({ signal }) =>
      apiJson<AuthStatus>("/api/auth/status", { signal }),
    retry: false,
    // Polling this would ask the server who you are every few seconds for no
    // reason; a sign-in or sign-out invalidates it explicitly.
    refetchInterval: false,
    staleTime: 30_000,
  });
}

/** Everything a signed-in session changes, so nothing keeps stale data. */
function invalidateEverything(qc: ReturnType<typeof useQueryClient>) {
  qc.invalidateQueries();
}

export function useLogin() {
  const qc = useQueryClient();
  return useMutation<
    { user: AuthUser },
    Error,
    { email: string; password: string }
  >({
    mutationFn: (body) =>
      apiJson<{ user: AuthUser }>("/api/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: () => invalidateEverything(qc),
  });
}

/** Claim an unclaimed install: create the first admin and switch it on. */
export function useSetup() {
  const qc = useQueryClient();
  return useMutation<
    { user: AuthUser },
    Error,
    { setup_token: string; name: string; email: string; password: string }
  >({
    mutationFn: (body) =>
      apiJson<{ user: AuthUser }>("/api/auth/setup", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: () => invalidateEverything(qc),
  });
}

export function useLogout() {
  const qc = useQueryClient();
  return useMutation<{ ok: boolean }, Error, void>({
    mutationFn: () =>
      apiJson<{ ok: boolean }>("/api/auth/logout", { method: "POST" }),
    onSuccess: () => invalidateEverything(qc),
  });
}

/**
 * Whether this session may see and change installation-wide settings.
 *
 * Before an install has accounts there are no roles, so whoever got in is the
 * operator by definition. A status that has not arrived yet reads as admin,
 * which is what keeps the settings nav from flashing items away on every page
 * load; anything that *acts* on the answer rather than laying out a page should
 * wait for the query to settle first, because a guess in that direction is
 * wrong for a member.
 */
export function isAdminOf(status: AuthStatus | undefined): boolean {
  return status?.mode !== "accounts" || status.user?.role === "admin";
}
