/**
 * Invitations, and the links that redeem them.
 *
 * The link is the mechanism and mail is a convenience — SMTP is configured in
 * this same UI, so requiring it to invite the first colleague would be a circle.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";

export interface Invite {
  id: string;
  email: string;
  kind: "invite" | "reset";
  role: "admin" | "member";
  created_by: string | null;
  created_at: string;
  expires_at: string;
  accepted_at: string | null;
}

export interface CreatedInvite {
  invite: Invite;
  /** `null` only when no public URL is set — then there is no link to give. */
  link: string | null;
  mailed: boolean;
  /** Why mail did not go out. The invitation is real regardless. */
  mail_error: string | null;
}

export function useInvites(enabled: boolean) {
  return useQuery<Invite[], Error>({
    queryKey: ["invites"],
    enabled,
    queryFn: ({ signal }) => apiJson<Invite[]>("/api/invites", { signal }),
    retry: false,
    refetchInterval: false,
  });
}

export function useCreateInvite() {
  const qc = useQueryClient();
  return useMutation<
    CreatedInvite,
    Error,
    { email: string; role: "admin" | "member" }
  >({
    mutationFn: (body) =>
      apiJson<CreatedInvite>("/api/invites", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["invites"] }),
  });
}

export function useRevokeInvite() {
  const qc = useQueryClient();
  return useMutation<{ revoked: boolean; id: string }, Error, string>({
    mutationFn: (id) =>
      apiJson<{ revoked: boolean; id: string }>(
        `/api/invites/${encodeURIComponent(id)}`,
        { method: "DELETE" },
      ),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["invites"] }),
  });
}

export interface InviteDetails {
  email: string;
  kind: "invite" | "reset";
  expires_at: string;
}

/** What a link is for, before redeeming it. Public. */
export function useInviteDetails(token: string | null) {
  return useQuery<InviteDetails, Error>({
    queryKey: ["invite", token],
    enabled: !!token,
    queryFn: ({ signal }) =>
      apiJson<InviteDetails>(
        `/api/invites/token/${encodeURIComponent(token!)}`,
        { signal },
      ),
    retry: false,
    refetchInterval: false,
  });
}

/** Redeem a link: create the account, or set a new password. Public. */
export function useAcceptInvite(token: string | null) {
  return useMutation<
    { accepted: boolean; email: string },
    Error,
    { name?: string; password: string }
  >({
    mutationFn: (body) =>
      apiJson<{ accepted: boolean; email: string }>(
        `/api/invites/token/${encodeURIComponent(token!)}`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        },
      ),
  });
}

/** Ask for a reset link. Answers the same whether or not the address exists. */
export function useRequestReset() {
  return useMutation<
    { ok: boolean; message: string },
    Error,
    { email: string }
  >({
    mutationFn: (body) =>
      apiJson<{ ok: boolean; message: string }>("/auth/reset-password", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
  });
}
