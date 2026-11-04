/**
 * A person's Linear account, linked to their harness account.
 *
 * People sign in here with GitHub and use a work address in Linear, so a
 * mention or a delegation from Linear cannot be matched to them by address.
 * Linking is signing in to Linear once: the server records who that Linear
 * user is and throws the token away.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";

export interface LinkedIdentity {
  id: string;
  user_id: string;
  /** `linear`. */
  provider: string;
  /** The Linear connection (workspace) it was linked through. */
  connection: string;
  provider_user_id: string;
  email: string | null;
  name: string | null;
  linked_at: string;
}

/** A Linear connection someone could link through. */
export interface LinkableConnection {
  id: string;
  label: string;
}

export interface MyIdentities {
  identities: LinkedIdentity[];
  connections: LinkableConnection[];
}

export function useMyIdentities(enabled: boolean) {
  return useQuery<MyIdentities, Error>({
    queryKey: ["identities", "me"],
    enabled,
    queryFn: ({ signal }) =>
      apiJson<MyIdentities>("/api/me/identities", { signal }),
    retry: false,
  });
}

/**
 * Send the browser to Linear to sign in. The URL comes back as JSON because
 * this request is authenticated and a plain navigation would not be.
 */
export function useLinkLinear() {
  return useMutation<{ url: string }, Error, { connection: string }>({
    mutationFn: ({ connection }) =>
      apiJson<{ url: string }>(
        `/api/me/identities/linear/start?connection=${encodeURIComponent(connection)}`,
        { method: "POST" },
      ),
    onSuccess: ({ url }) => {
      window.location.assign(url);
    },
  });
}

export function useUnlinkMine() {
  const qc = useQueryClient();
  return useMutation<unknown, Error, { id: string }>({
    mutationFn: ({ id }) =>
      apiJson(`/api/me/identities/${encodeURIComponent(id)}`, {
        method: "DELETE",
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["identities"] }),
  });
}

/** An account's links, for an administrator. */
export function useUserIdentities(userId: string, enabled: boolean) {
  return useQuery<{ identities: LinkedIdentity[] }, Error>({
    queryKey: ["identities", "user", userId],
    enabled,
    queryFn: ({ signal }) =>
      apiJson<{ identities: LinkedIdentity[] }>(
        `/api/users/${encodeURIComponent(userId)}/identities`,
        { signal },
      ),
    retry: false,
  });
}

/** Remove a link. An administrator can do this, never create one. */
export function useUnlinkForUser() {
  const qc = useQueryClient();
  return useMutation<unknown, Error, { userId: string; id: string }>({
    mutationFn: ({ userId, id }) =>
      apiJson(
        `/api/users/${encodeURIComponent(userId)}/identities/${encodeURIComponent(id)}`,
        { method: "DELETE" },
      ),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["identities"] }),
  });
}

/** How a linked identity reads: the Linear address, else the name. */
export function identityLabel(identity: LinkedIdentity): string {
  if (identity.name && identity.email)
    return `${identity.name} <${identity.email}>`;
  return identity.email ?? identity.name ?? identity.provider_user_id;
}
