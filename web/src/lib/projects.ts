/**
 * Data layer for the project registry (`/api/projects`).
 *
 * - `useProjects()`        — `GET /api/projects` (list).
 * - `useRegisterProject()` — `POST /api/projects` (register/update; clones the repo).
 * - `useDeleteProject()`   — `DELETE /api/projects/{name}` (deregister + remove checkout).
 * - `useProjectCacheSize(name)` — `GET /api/projects/{name}/cache-size`.
 * - `useSetProjectCacheCap()`   — `PUT /api/projects/{name}/cache-cap`.
 * - `useClearProjectCache()`    — `POST /api/projects/{name}/cache/clear`.
 * - `useSweepProjectCache()`    — `POST /api/projects/{name}/cache/sweep`.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";
import type {
  CacheSize,
  Project,
  RegisterProjectRequest,
} from "@/types/project";

export function useProjects() {
  return useQuery<Project[], Error>({
    queryKey: ["projects"],
    queryFn: ({ signal }) => apiJson<Project[]>("/api/projects", { signal }),
  });
}

/**
 * Register/update a project. The server may return the project directly, or — if
 * the row saved but the git clone failed — `{ project, warning }`. We normalize
 * to `{ project, warning? }` so the form can surface a non-fatal repo warning.
 */
export function useRegisterProject() {
  const qc = useQueryClient();
  return useMutation<
    { project: Project; warning?: string },
    Error,
    RegisterProjectRequest
  >({
    mutationFn: async (body) => {
      const res = await apiJson<
        Project | { project: Project; warning?: string }
      >("/api/projects", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
      return "project" in res ? res : { project: res };
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ["projects"] }),
  });
}

export function useDeleteProject() {
  const qc = useQueryClient();
  return useMutation<{ deleted: boolean; project: string }, Error, string>({
    mutationFn: (name) =>
      apiJson(`/api/projects/${encodeURIComponent(name)}`, {
        method: "DELETE",
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["projects"] }),
  });
}

export function useProjectCacheSize(name: string, enabled = true) {
  return useQuery<CacheSize, Error>({
    queryKey: ["project-cache-size", name],
    queryFn: ({ signal }) =>
      apiJson<CacheSize>(
        `/api/projects/${encodeURIComponent(name)}/cache-size`,
        {
          signal,
        },
      ),
    staleTime: 30_000,
    enabled,
  });
}

export function useSetProjectCacheCap() {
  const qc = useQueryClient();
  return useMutation<Project, Error, { name: string; cap_gb: number | null }>({
    mutationFn: ({ name, cap_gb }) =>
      apiJson<Project>(`/api/projects/${encodeURIComponent(name)}/cache-cap`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ cap_gb }),
      }),
    onSuccess: (_, { name }) => {
      qc.invalidateQueries({ queryKey: ["projects"] });
      qc.invalidateQueries({ queryKey: ["project-cache-size", name] });
    },
  });
}

export function useClearProjectCache() {
  const qc = useQueryClient();
  return useMutation<{ cleared: boolean; bytes_freed: number }, Error, string>({
    mutationFn: (name) =>
      apiJson<{ cleared: boolean; bytes_freed: number }>(
        `/api/projects/${encodeURIComponent(name)}/cache/clear`,
        {
          method: "POST",
        },
      ),
    onSuccess: (_, name) =>
      qc.invalidateQueries({ queryKey: ["project-cache-size", name] }),
  });
}

export function useSweepProjectCache() {
  const qc = useQueryClient();
  return useMutation<
    { swept: boolean; before?: number; after?: number },
    Error,
    string
  >({
    mutationFn: (name) =>
      apiJson<{ swept: boolean; before?: number; after?: number }>(
        `/api/projects/${encodeURIComponent(name)}/cache/sweep`,
        { method: "POST" },
      ),
    onSuccess: (_, name) =>
      qc.invalidateQueries({ queryKey: ["project-cache-size", name] }),
  });
}
