/**
 * React Query hooks for the Linear trigger binding API.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";
import type {
  LinearDiscovery,
  LinearSource,
  LinearSourceInput,
} from "@/types/linear";

export function useLinearDiscovery(project: string | null) {
  return useQuery<LinearDiscovery, Error>({
    queryKey: ["linear", "discovery", project],
    enabled: !!project,
    queryFn: ({ signal }) =>
      apiJson<LinearDiscovery>(
        `/api/projects/${encodeURIComponent(project!)}/linear/discovery`,
        { signal },
      ),
    retry: false,
    staleTime: 60_000,
  });
}

export function useLinearSource(
  project: string | null,
  workflow: string | null,
) {
  return useQuery<LinearSource | null, Error>({
    queryKey: ["linear", "source", project, workflow],
    enabled: !!project && !!workflow,
    queryFn: ({ signal }) =>
      apiJson<LinearSource | null>(
        `/api/projects/${encodeURIComponent(project!)}/linear-source?workflow=${encodeURIComponent(workflow!)}`,
        { signal },
      ),
  });
}

export function useSaveLinearSource(project: string | null) {
  const qc = useQueryClient();
  return useMutation<LinearSource, Error, LinearSourceInput>({
    mutationFn: (body) =>
      apiJson<LinearSource>(
        `/api/projects/${encodeURIComponent(project!)}/linear-source`,
        {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        },
      ),
    onSuccess: (_data, variables) => {
      qc.invalidateQueries({
        queryKey: ["linear", "source", project, variables.workflow],
      });
      qc.invalidateQueries({ queryKey: ["linear", "sources", project] });
    },
  });
}

interface ProjectCredential {
  provider: string;
  configured: boolean;
}

/** Whether this project has a project-scoped `linear` API key configured. */
export function useProjectLinearKey(project: string | null) {
  return useQuery<boolean, Error>({
    queryKey: ["project-credentials", project, "linear"],
    enabled: !!project,
    queryFn: async ({ signal }) => {
      const creds = await apiJson<ProjectCredential[]>(
        `/api/projects/${encodeURIComponent(project!)}/credentials`,
        { signal },
      );
      return creds.some((c) => c.provider === "linear" && c.configured);
    },
  });
}

/** Set this project's Linear API key (project-scoped, overrides the global one). */
export function useSetProjectLinearKey(project: string | null) {
  const qc = useQueryClient();
  return useMutation<unknown, Error, string>({
    mutationFn: (apiKey) =>
      apiJson(
        `/api/projects/${encodeURIComponent(project!)}/credentials/linear`,
        {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ fields: { api_key: apiKey } }),
        },
      ),
    onSuccess: () => {
      qc.invalidateQueries({
        queryKey: ["project-credentials", project, "linear"],
      });
      // Discovery uses the key — refetch so the dropdowns populate.
      qc.invalidateQueries({ queryKey: ["linear", "discovery", project] });
    },
  });
}

export function useDeleteLinearSource(project: string | null) {
  const qc = useQueryClient();
  return useMutation<{ deleted: boolean; workflow: string }, Error, string>({
    mutationFn: (workflow) =>
      apiJson<{ deleted: boolean; workflow: string }>(
        `/api/projects/${encodeURIComponent(project!)}/linear-source?workflow=${encodeURIComponent(workflow)}`,
        { method: "DELETE" },
      ),
    onSuccess: (_data, workflow) => {
      qc.invalidateQueries({
        queryKey: ["linear", "source", project, workflow],
      });
      qc.invalidateQueries({ queryKey: ["linear", "sources", project] });
    },
  });
}
