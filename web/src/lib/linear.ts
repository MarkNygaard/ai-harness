/**
 * React Query hooks for the Linear trigger binding API.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";
import type {
  CreatedLinearIssue,
  CreateLinearIssueInput,
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

export function useLinearSources(project: string | null) {
  return useQuery<LinearSource[], Error>({
    queryKey: ["linear", "sources", project],
    enabled: !!project,
    queryFn: ({ signal }) =>
      apiJson<LinearSource[]>(
        `/api/projects/${encodeURIComponent(project!)}/linear-sources`,
        { signal },
      ),
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

/** Create a Linear issue from a task/finding against the project's binding. */
export function useCreateLinearIssue(project: string | null) {
  return useMutation<CreatedLinearIssue, Error, CreateLinearIssueInput>({
    mutationFn: (body) =>
      apiJson<CreatedLinearIssue>(
        `/api/projects/${encodeURIComponent(project!)}/linear-issues`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        },
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
