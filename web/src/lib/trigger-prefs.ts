/**
 * Where this browser manages Linear triggers: the Triggers page, or each
 * project's own Linear tab.
 *
 * A transition switch, not a permanent mode. Linear triggers are moving from
 * the project page to the Triggers page; until that move is finished, both
 * homes work, and this decides which one the links lead to. Same shape as
 * [`nav-prefs`](./nav-prefs.ts) — a display preference, per browser, in
 * `localStorage` — for the same reason: several unrelated components read it.
 */
import { useSyncExternalStore } from "react";

export type LinearTriggerHome = "triggers" | "project";

const STORAGE_KEY = "harness.triggers.linear-home";
const DEFAULT: LinearTriggerHome = "triggers";

const listeners = new Set<() => void>();

function read(): LinearTriggerHome {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    return raw === "project" || raw === "triggers" ? raw : DEFAULT;
  } catch {
    return DEFAULT;
  }
}

let snapshot: LinearTriggerHome = read();

export function setLinearTriggerHome(next: LinearTriggerHome): void {
  if (next === snapshot) return;
  snapshot = next;
  try {
    window.localStorage.setItem(STORAGE_KEY, next);
  } catch {
    // Unwritable storage: the choice holds for this visit only.
  }
  for (const listener of listeners) listener();
}

function subscribe(onChange: () => void): () => void {
  listeners.add(onChange);
  return () => {
    listeners.delete(onChange);
  };
}

const getSnapshot = () => snapshot;

export function useLinearTriggerHome(): LinearTriggerHome {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
