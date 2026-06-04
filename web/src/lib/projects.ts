/**
 * Data layer for the project registry (`/api/projects`).
 *
 * - `useProjects()`        — `GET /api/projects` (list).
 * - `useRegisterProject()` — `POST /api/projects` (register/update; clones the repo).
 * - `useDeleteProject()`   — `DELETE /api/projects/{name}` (deregister + remove checkout).
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";
import type { Project, RegisterProjectRequest } from "@/types/project";

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
      const res = await apiJson<Project | { project: Project; warning?: string }>(
        "/api/projects",
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        },
      );
      return "project" in res ? res : { project: res };
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ["projects"] }),
  });
}

export function useDeleteProject() {
  const qc = useQueryClient();
  return useMutation<{ deleted: boolean; project: string }, Error, string>({
    mutationFn: (name) =>
      apiJson(`/api/projects/${encodeURIComponent(name)}`, { method: "DELETE" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["projects"] }),
  });
}
