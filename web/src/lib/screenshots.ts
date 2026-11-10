/**
 * Screenshots a run's agents took of the UI it changed. The server keeps them
 * for a number of days after the run (`HARNESS_SCREENSHOT_TTL_DAYS`, 7 by
 * default), then deletes them and says how many there were.
 */
import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./api";

export interface Screenshot {
  file: string;
  page?: string;
  viewport?: string;
  note?: string;
}

export interface RunScreenshots {
  screenshots: Screenshot[];
  /** How many there were, when they have been deleted; otherwise null. */
  expired: number | null;
  ttl_days: number;
}

function base(runId: string): string {
  return `/api/runs/${encodeURIComponent(runId)}/screenshots`;
}

/**
 * The run's screenshots. They are kept as the run ends, so a live run has none
 * yet; `live` is part of the key so the list is read again when it finishes.
 */
export function useRunScreenshots(runId: string | null, live: boolean) {
  return useQuery<RunScreenshots, Error>({
    queryKey: ["runs", runId, "screenshots", live],
    enabled: !!runId,
    queryFn: ({ signal }) => apiJson<RunScreenshots>(base(runId!), { signal }),
  });
}

/**
 * An object URL for one screenshot. The API wants a bearer token, which an
 * `<img src>` cannot send, so the image is fetched and handed over as a blob.
 */
export function useScreenshotUrl(runId: string, file: string): string | null {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let revoked = false;
    let made: string | null = null;
    const ctrl = new AbortController();
    apiFetch(`${base(runId)}/${encodeURIComponent(file)}`, {
      signal: ctrl.signal,
      headers: { Accept: "image/*" },
    })
      .then((r) => r.blob())
      .then((blob) => {
        if (revoked) return;
        made = URL.createObjectURL(blob);
        setUrl(made);
      })
      .catch(() => {});
    return () => {
      revoked = true;
      ctrl.abort();
      if (made) URL.revokeObjectURL(made);
    };
  }, [runId, file]);
  return url;
}

/** Screenshots grouped by page, in the order the agents listed them. */
export function groupByPage(shots: Screenshot[]): [string, Screenshot[]][] {
  const groups = new Map<string, Screenshot[]>();
  for (const s of shots) {
    const key = s.page?.trim() || s.file;
    groups.set(key, [...(groups.get(key) ?? []), s]);
  }
  return [...groups.entries()];
}
