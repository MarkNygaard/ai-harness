/**
 * The workflow library — browsing the registry and installing from it.
 *
 * Everything goes through the harness rather than the registry directly: the
 * listing is only useful once it says what is already installed here, and only
 * the server knows that.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";

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

/**
 * Who this harness publishes as.
 *
 * `configured: false` is the normal state of an install that has never
 * published — not an error, and the UI shows a different thing for it. The name
 * is resolved by the registry from the token, never claimed by the browser: the
 * token itself stays server-side, so this is the only way the page can know
 * whose name an entry would carry.
 */
export interface PublisherIdentity {
  configured: boolean;
  name: string | null;
  login: string | null;
  /**
   * Whether this registry can issue a token by itself, rather than needing one
   * minted by whoever runs it. False for a private registry with no GitHub app
   * configured, where the only path is still a pasted token.
   */
  enrollment: boolean;
}

export function usePublisher(enabled: boolean) {
  return useQuery<PublisherIdentity, Error>({
    queryKey: ["library", "publisher"],
    enabled,
    queryFn: ({ signal }) =>
      apiJson<PublisherIdentity>("/api/library/publisher", { signal }),
    retry: false,
    staleTime: 60_000,
  });
}

/** What a publish sends. Everything but `name` is optional after the first. */
export interface PublishRequest {
  /** The local workflow, by file stem. */
  name: string;
  title?: string;
  description?: string;
  tags?: string[];
  changelog?: string;
  /** Rename the publisher first, so the entry carries the expected name. */
  publish_as?: string;
}

export interface PublishResult {
  slug: string;
  version: number;
  publisher: string;
  /** This was the workflow's first appearance in the library. */
  created: boolean;
}

/**
 * Publish a workflow, or add a version to one this harness already published.
 *
 * One call for both, and the server decides which from its own record. Asking
 * the caller would invite answering wrong, and both wrong answers are bad: a
 * duplicate entry, or a version aimed at somebody else's workflow.
 */
export function usePublishWorkflow() {
  const qc = useQueryClient();
  return useMutation<PublishResult, Error, PublishRequest>({
    mutationFn: (req) =>
      apiJson("/api/library/publish", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(req),
      }),
    onSuccess: () => {
      // The library gains an entry or a version, the workflow gains its
      // published marker, and the publisher's display name may have changed.
      void qc.invalidateQueries({ queryKey: ["library"] });
      void qc.invalidateQueries({ queryKey: ["authoring", "workflows"] });
    },
  });
}

/**
 * Amend what a published entry says about itself, without cutting a version.
 *
 * Separate from publishing because the two are different acts: a typo in a
 * description is not a new release, and fixing it by publishing again would
 * tell everyone holding the workflow that something changed when nothing did.
 */
export function useAmendPublished() {
  const qc = useQueryClient();
  return useMutation<
    { amended: string },
    Error,
    { name: string; title?: string; description?: string; tags?: string[] }
  >({
    mutationFn: ({ name, ...body }) =>
      apiJson(`/api/library/published/${encodeURIComponent(name)}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["library"] });
      void qc.invalidateQueries({ queryKey: ["authoring", "workflows"] });
    },
  });
}

/**
 * Take a published workflow out of the library.
 *
 * The local file is untouched — it is yours, and withdrawing an entry says
 * something about the library rather than about this harness. Anyone who
 * already installed it keeps their copy, which goes on working.
 */
export function useUnpublishWorkflow() {
  const qc = useQueryClient();
  return useMutation<{ unpublished: string }, Error, string>({
    mutationFn: (name) =>
      apiJson(`/api/library/published/${encodeURIComponent(name)}`, {
        method: "DELETE",
      }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["library"] });
      void qc.invalidateQueries({ queryKey: ["authoring", "workflows"] });
    },
  });
}

/** What an amendment would change. Empty means there is nothing to send. */
export interface Amendments {
  title?: string;
  description?: string;
}

/**
 * Work out what an entry's metadata amendment should carry.
 *
 * Only the fields that actually changed, so publishing an update with the form
 * untouched stays one request rather than two. Separated from the dialog
 * because getting it wrong is invisible in review: sending everything every
 * time still works, and sending nothing when something changed silently drops
 * the edit.
 *
 * A title cleared to nothing is not an amendment. The registry has no concept
 * of an entry without a title, so a blank field means "leave it", not "erase
 * it". A description is allowed to be cleared, because an entry with no
 * description is a real state.
 */
export function amendmentsFor(
  current: { title: string; description: string },
  published: { title: string; description: string },
): Amendments {
  const out: Amendments = {};
  const title = current.title.trim();
  if (title && title !== published.title.trim()) out.title = title;

  const description = current.description.trim();
  if (description !== published.description.trim()) {
    out.description = description;
  }
  return out;
}

/** A device flow in progress, as much of it as the page shows. */
export interface DeviceFlow {
  device_code: string;
  user_code: string;
  verification_uri: string;
  expires_in: number;
  interval: number;
}

interface PollResult {
  status: "pending" | "slow_down" | "complete";
  github_login: string | null;
}

/**
 * How long to wait after a `slow_down`, on top of the interval GitHub gave.
 * GitHub's own guidance for the device flow, and ignoring it gets the flow
 * throttled rather than merely slowed.
 */
const SLOW_DOWN_PENALTY_SECS = 5;

/**
 * Run the whole enrollment flow: start it, show the code, poll until the
 * person has authorized.
 *
 * Polling lives here rather than in a query because the cadence is the
 * registry's to set — it forwards GitHub's `interval`, and raises it on a
 * `slow_down` — and a refetch interval fixed at mount cannot follow that.
 *
 * The token never arrives here. The server writes it to the credential store
 * and answers with the login it belongs to, because a token that reached the
 * page could publish under its owner's name from anything that could read it.
 */
export function useEnrollment() {
  const qc = useQueryClient();
  const [flow, setFlow] = useState<DeviceFlow | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const cancelled = useRef(false);

  // A flow abandoned by closing the dialog must stop polling. Without this the
  // timer outlives the component and writes a credential nobody is waiting for.
  useEffect(() => {
    cancelled.current = false;
    return () => {
      cancelled.current = true;
    };
  }, []);

  const reset = useCallback(() => {
    setFlow(null);
    setError(null);
    setBusy(false);
  }, []);

  const start = useCallback(async () => {
    setError(null);
    setBusy(true);
    try {
      const started = await apiJson<DeviceFlow>("/api/library/enroll", {
        method: "POST",
      });
      setFlow(started);
      await poll(started, started.interval);
    } catch (e) {
      if (!cancelled.current) setError(errorText(e));
      setBusy(false);
    }

    async function poll(started: DeviceFlow, waitSecs: number) {
      const deadline = Date.now() + started.expires_in * 1000;
      let wait = Math.max(1, waitSecs);

      for (;;) {
        if (cancelled.current) return;
        if (Date.now() > deadline) {
          setError("The code expired before it was entered. Try again.");
          setBusy(false);
          return;
        }
        await sleep(wait * 1000);
        if (cancelled.current) return;

        const result = await apiJson<PollResult>("/api/library/enroll/poll", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ device_code: started.device_code }),
        });

        if (result.status === "complete") {
          setBusy(false);
          setFlow(null);
          // The publisher identity and the credential badge both just changed.
          void qc.invalidateQueries({ queryKey: ["library", "publisher"] });
          void qc.invalidateQueries({ queryKey: ["credentials"] });
          return;
        }
        if (result.status === "slow_down") wait += SLOW_DOWN_PENALTY_SECS;
      }
    }
  }, [qc]);

  return { flow, error, busy, start, reset };
}

function sleep(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function errorText(e: unknown) {
  return e instanceof Error ? e.message : String(e);
}
