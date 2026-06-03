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
    queryFn: ({ signal }) => apiJson<ProviderCredential[]>("/api/credentials", { signal }),
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

export function useDeleteCredential() {
  const qc = useQueryClient();
  return useMutation<{ deleted: boolean; provider: string }, Error, string>({
    mutationFn: (provider) =>
      apiJson(`/api/credentials/${encodeURIComponent(provider)}`, { method: "DELETE" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["credentials"] }),
  });
}
