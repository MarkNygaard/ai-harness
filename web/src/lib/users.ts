/**
 * Who has an account on this harness. Administrator-only, at the route.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";
import type { AuthUser } from "./auth";

export function useUsers(enabled: boolean) {
  return useQuery<AuthUser[], Error>({
    queryKey: ["users"],
    enabled,
    queryFn: ({ signal }) => apiJson<AuthUser[]>("/api/users", { signal }),
    retry: false,
    refetchInterval: false,
  });
}

function useUserMutation<TBody>(
  path: (id: string) => string,
  method: "PUT" | "DELETE",
) {
  const qc = useQueryClient();
  return useMutation<AuthUser, Error, { id: string } & TBody>({
    mutationFn: ({ id, ...body }) =>
      apiJson<AuthUser>(path(id), {
        method,
        ...(method === "PUT"
          ? {
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify(body),
            }
          : {}),
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["users"] });
      // Demoting or suspending yourself changes what you can see.
      qc.invalidateQueries({ queryKey: ["auth", "status"] });
    },
  });
}

export function useSetUserRole() {
  return useUserMutation<{ role: "admin" | "member" }>(
    (id) => `/api/users/${encodeURIComponent(id)}/role`,
    "PUT",
  );
}

export function useSetUserDisabled() {
  return useUserMutation<{ disabled: boolean }>(
    (id) => `/api/users/${encodeURIComponent(id)}/disabled`,
    "PUT",
  );
}

export function useDeleteUser() {
  const qc = useQueryClient();
  return useMutation<{ deleted: boolean; id: string }, Error, { id: string }>({
    mutationFn: ({ id }) =>
      apiJson<{ deleted: boolean; id: string }>(
        `/api/users/${encodeURIComponent(id)}`,
        { method: "DELETE" },
      ),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["users"] });
      qc.invalidateQueries({ queryKey: ["auth", "status"] });
    },
  });
}
