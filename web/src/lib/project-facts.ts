/**
 * Learned project facts (`/api/projects/{name}/facts`): what a project's runs
 * kept failing on, written down and told to every later run there.
 *
 * - `useProjectFacts(name)`   — `GET` every fact, with how many runs back it.
 * - `useAddProjectFact()`     — `POST` a fact written by hand (live at once).
 * - `useUpdateProjectFact()`  — `PUT {id}` reword one.
 * - `useRemoveProjectFact()`  — `DELETE {id}` retire one; it isn't learned again.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./api";

export type FactStatus = "forming" | "active" | "retired";

export interface ProjectFact {
  id: string;
  project: string;
  text: string;
  status: FactStatus;
  /** `learned` from failures, or `manual`. */
  source: "learned" | "manual";
  fingerprints: string[];
  run_ids: string[];
  retire_reason: string | null;
  created_at: string;
  activated_at: string | null;
  retired_at: string | null;
  /** Runs in the last 30 days that hit its failures. */
  runs: number;
  /** Runs a learned fact needs before it goes live. */
  needed: number;
}

const base = (project: string) =>
  `/api/projects/${encodeURIComponent(project)}/facts`;

const key = (project: string) => ["project-facts", project];

export function useProjectFacts(project: string) {
  return useQuery<ProjectFact[], Error>({
    queryKey: key(project),
    queryFn: async ({ signal }) =>
      (await apiJson<{ facts: ProjectFact[] }>(base(project), { signal }))
        .facts,
    staleTime: 30_000,
  });
}

export function useAddProjectFact(project: string) {
  const qc = useQueryClient();
  return useMutation<ProjectFact, Error, string>({
    mutationFn: (text) =>
      apiJson<ProjectFact>(base(project), {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ text }),
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: key(project) }),
  });
}

export function useUpdateProjectFact(project: string) {
  const qc = useQueryClient();
  return useMutation<void, Error, { id: string; text: string }>({
    mutationFn: async ({ id, text }) => {
      await apiFetch(`${base(project)}/${encodeURIComponent(id)}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ text }),
      });
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: key(project) }),
  });
}

export function useRemoveProjectFact(project: string) {
  const qc = useQueryClient();
  return useMutation<void, Error, string>({
    mutationFn: async (id) => {
      await apiFetch(`${base(project)}/${encodeURIComponent(id)}`, {
        method: "DELETE",
      });
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: key(project) }),
  });
}
