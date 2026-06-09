/**
 * Data layer for the workflow authoring API (`/api/authoring/*`) — the editor's
 * catalog, workflow list/detail, validation, and save.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";
import type {
  Catalog,
  ValidationResult,
  WorkflowSource,
  WorkflowSummary,
} from "@/types/authoring";

export function useCatalog() {
  return useQuery<Catalog, Error>({
    queryKey: ["authoring", "catalog"],
    queryFn: ({ signal }) =>
      apiJson<Catalog>("/api/authoring/catalog", { signal }),
    staleTime: 60_000,
  });
}

export function useWorkflowList() {
  return useQuery<WorkflowSummary[], Error>({
    queryKey: ["authoring", "workflows"],
    queryFn: ({ signal }) =>
      apiJson<WorkflowSummary[]>("/api/authoring/workflows", { signal }),
  });
}

export function useProjectWorkflows(project: string | null) {
  return useQuery<WorkflowSummary[], Error>({
    queryKey: ["authoring", "workflows", project],
    enabled: !!project,
    queryFn: ({ signal }) =>
      apiJson<WorkflowSummary[]>(
        `/api/projects/${encodeURIComponent(project!)}/authoring/workflows`,
        { signal },
      ),
  });
}

export function useWorkflowSource(name: string | null) {
  return useQuery<WorkflowSource, Error>({
    queryKey: ["authoring", "workflow", name],
    queryFn: ({ signal }) =>
      apiJson<WorkflowSource>(
        `/api/authoring/workflows/${encodeURIComponent(name!)}`,
        { signal },
      ),
    enabled: !!name,
  });
}

/** Validate a candidate YAML server-side (`parse_workflow` + cycle check). */
export function useValidateWorkflow() {
  return useMutation<ValidationResult, Error, string>({
    mutationFn: (yaml) =>
      apiJson<ValidationResult>("/api/authoring/validate", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ yaml }),
      }),
  });
}

/** Validate-then-save a workflow to the project's `.harness/workflows/`. */
export function useSaveWorkflow() {
  const qc = useQueryClient();
  return useMutation<
    { saved: boolean; name: string },
    Error,
    { name: string; yaml: string }
  >({
    mutationFn: (body) =>
      apiJson<{ saved: boolean; name: string }>("/api/authoring/workflows", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["authoring", "workflows"] });
      qc.invalidateQueries({ queryKey: ["runs"] });
    },
  });
}

/** Reset a workflow to its bundled default by deleting the project override. */
export function useResetWorkflow() {
  const qc = useQueryClient();
  return useMutation<{ reset: boolean; name: string }, Error, string>({
    mutationFn: (name) =>
      apiJson<{ reset: boolean; name: string }>(
        `/api/authoring/workflows/${encodeURIComponent(name)}`,
        { method: "DELETE" },
      ),
    onSuccess: (_data, name) => {
      qc.invalidateQueries({ queryKey: ["authoring", "workflow", name] });
      qc.invalidateQueries({ queryKey: ["authoring", "workflows"] });
    },
  });
}
