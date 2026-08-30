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

/**
 * What `PUT /api/users/{id}` answers. `sessions_closed` is true when the
 * address changed: an address is what sign-in matches on, so every session
 * the account held ended with it and the member has to sign in again.
 */
export interface ProfileUpdate {
  user: AuthUser;
  sessions_closed: boolean;
}

function useUserMutation<TBody, TResult = AuthUser>(
  path: (id: string) => string,
  method: "PUT" | "DELETE",
) {
  const qc = useQueryClient();
  return useMutation<TResult, Error, { id: string } & TBody>({
    mutationFn: ({ id, ...body }) =>
      apiJson<TResult>(path(id), {
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

/**
 * The address is what sign-in matches on, so the server holds it unique and
 * answers 409 when it is not. The response reports whether the member was
 * signed out so the UI can say so after the dialog closes.
 */
export function useSetUserProfile() {
  return useUserMutation<{ name: string; email: string }, ProfileUpdate>(
    (id) => `/api/users/${encodeURIComponent(id)}`,
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
