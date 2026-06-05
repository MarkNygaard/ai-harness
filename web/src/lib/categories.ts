/**
 * Data layer for the workflow step **category** registry (`/api/categories`).
 * Categories group steps for the run overview's time-by-category breakdown and
 * bar colouring; the workflow editor picks one per node.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";

export interface Category {
  id: string;
  label: string;
  /** CSS colour string (e.g. an `oklch(...)` value). */
  color: string;
  ordinal: number;
  created_at: string;
  updated_at: string;
}

export function useCategories() {
  return useQuery<Category[], Error>({
    queryKey: ["categories"],
    queryFn: ({ signal }) => apiJson<Category[]>("/api/categories", { signal }),
  });
}

export interface SaveCategory {
  id: string;
  label: string;
  color: string;
  ordinal?: number;
}

export function useSaveCategory() {
  const qc = useQueryClient();
  return useMutation<Category, Error, SaveCategory>({
    mutationFn: ({ id, label, color, ordinal }) =>
      apiJson<Category>(`/api/categories/${encodeURIComponent(id)}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ label, color, ordinal: ordinal ?? 0 }),
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["categories"] }),
  });
}

export function useDeleteCategory() {
  const qc = useQueryClient();
  return useMutation<{ deleted: boolean; id: string }, Error, string>({
    mutationFn: (id) =>
      apiJson(`/api/categories/${encodeURIComponent(id)}`, {
        method: "DELETE",
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["categories"] }),
  });
}

/** Build an id → colour map for resolving a node's category colour. */
export function categoryColorMap(
  cats: Category[] | undefined,
): Map<string, string> {
  return new Map((cats ?? []).map((c) => [c.id, c.color]));
}
