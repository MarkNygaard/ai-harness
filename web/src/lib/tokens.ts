/**
 * Your personal access tokens — what a program authenticates with once there
 * is a login in front of the UI.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";

export interface AccessToken {
  id: string;
  user_id: string;
  name: string;
  created_at: string;
  /** `null` until something authenticates with it. */
  last_used_at: string | null;
}

/** The value is returned exactly once, at creation. */
export interface CreatedToken {
  token: AccessToken;
  secret: string;
}

export function useTokens(enabled: boolean) {
  return useQuery<AccessToken[], Error>({
    queryKey: ["tokens"],
    enabled,
    queryFn: ({ signal }) => apiJson<AccessToken[]>("/api/tokens", { signal }),
    retry: false,
    refetchInterval: false,
  });
}

export function useCreateToken() {
  const qc = useQueryClient();
  return useMutation<CreatedToken, Error, { name: string }>({
    mutationFn: (body) =>
      apiJson<CreatedToken>("/api/tokens", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["tokens"] }),
  });
}

export function useRevokeToken() {
  const qc = useQueryClient();
  return useMutation<{ revoked: boolean; id: string }, Error, string>({
    mutationFn: (id) =>
      apiJson<{ revoked: boolean; id: string }>(
        `/api/tokens/${encodeURIComponent(id)}`,
        { method: "DELETE" },
      ),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["tokens"] }),
  });
}
