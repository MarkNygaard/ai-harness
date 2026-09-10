/**
 * The workflow library — browsing the registry and installing from it.
 *
 * Everything goes through the harness rather than the registry directly: the
 * listing is only useful once it says what is already installed here, and only
 * the server knows that.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { apiJson } from "./api";
import type { LibraryEntry } from "@/types/library";

/** What the server says when the name an install wants is already in use. */
export interface NameConflict {
  /** The name that is taken. */
  conflict: string;
  /** A free name near it, or null when even the suffixed ones are taken. */
  suggested_name: string | null;
}

/**
 * A `409` from the install route, carrying the taken name and a suggestion.
 *
 * Thrown rather than returned so the mutation's error path handles it like any
 * other failure, and carried as a typed field so the dialog can tell "this name
 * is taken" apart from "the registry is down" — the first is a question for the
 * person, the second is not.
 */
export class InstallNameConflict extends Error {
  readonly conflict: NameConflict;
  constructor(conflict: NameConflict) {
    super(`a workflow called ${conflict.conflict} is already installed here`);
    this.name = "InstallNameConflict";
    this.conflict = conflict;
  }
}

export function useLibrary(enabled: boolean) {
  return useQuery<LibraryEntry[], Error>({
    queryKey: ["library"],
    enabled,
    queryFn: ({ signal }) =>
      apiJson<LibraryEntry[]>("/api/library", { signal }),
    // The registry is a remote service and a browse is not worth retrying into:
    // when it is down, saying so immediately beats three slow attempts.
    retry: false,
    staleTime: 30_000,
  });
}

/**
 * Install a workflow, or move an installed one to the latest version — the same
 * call, because they are the same operation.
 *
 * `name` answers a previous conflict. Omitted on the first attempt, so the
 * ordinary case is one request with no body.
 */
/**
 * How to ask for an install.
 *
 * **A body is sent only when there is something to say.** The route takes an
 * optional JSON body, so the ordinary install is a bare `POST` — and a body
 * without `Content-Type: application/json` is refused with a `415`, which is
 * exactly what an empty `{}` sent without the header earned. Separated from the
 * hook so the shape is testable: the bug was invisible in review and obvious in
 * a browser.
 */
export function installRequestInit(name?: string): RequestInit {
  if (!name) return { method: "POST" };
  return {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name }),
  };
}

export function useInstallWorkflow() {
  const qc = useQueryClient();
  return useMutation<
    { installed_as: string; version: number; withdrawn: boolean },
    Error,
    { slug: string; name?: string }
  >({
    mutationFn: async ({ slug, name }) => {
      try {
        return await apiJson(
          `/api/library/${encodeURIComponent(slug)}/install`,
          installRequestInit(name),
        );
      } catch (e) {
        throw asConflict(e);
      }
    },
    onSuccess: () => {
      // Both lists move: the library gains an "installed" marker and the
      // workflows page gains a workflow.
      void qc.invalidateQueries({ queryKey: ["library"] });
      void qc.invalidateQueries({ queryKey: ["authoring", "workflows"] });
    },
  });
}

export function useUninstallWorkflow() {
  const qc = useQueryClient();
  return useMutation<{ uninstalled: string }, Error, string>({
    mutationFn: (slug) =>
      apiJson(`/api/library/${encodeURIComponent(slug)}`, { method: "DELETE" }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["library"] });
      void qc.invalidateQueries({ queryKey: ["authoring", "workflows"] });
    },
  });
}

/**
 * Recognise a name conflict inside whatever `apiJson` threw.
 *
 * The shape is read defensively rather than assumed: an error path that throws
 * its own `TypeError` while inspecting an error would replace a message the
 * person could act on with one nobody can.
 */
export function asConflict(e: unknown): Error {
  const body = (e as { body?: unknown })?.body;
  if (body && typeof body === "object" && "conflict" in body) {
    const conflict = (body as { conflict?: unknown }).conflict;
    const suggested = (body as { suggested_name?: unknown }).suggested_name;
    if (typeof conflict === "string") {
      return new InstallNameConflict({
        conflict,
        suggested_name: typeof suggested === "string" ? suggested : null,
      });
    }
  }
  return e instanceof Error ? e : new Error(String(e));
}
