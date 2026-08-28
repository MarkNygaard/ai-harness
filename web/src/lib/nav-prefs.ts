/**
 * Which sidebar entries this browser hides.
 *
 * The set lived inside `AppSidebar` while the only way to edit it was a mode
 * toggle in that same sidebar. Now that Settings → Preferences edits it too,
 * both have to see the same value — so it moves to a module-level store, the
 * same shape as [`theme`](./theme.ts), for the same reason: two unrelated
 * components, one piece of state, no shared ancestor to hang a provider on.
 *
 * Per browser, not per user: this is a display preference, and it stays in
 * `localStorage` rather than becoming server state.
 */
import { useSyncExternalStore } from "react";

const STORAGE_KEY = "harness.nav.hidden";

const listeners = new Set<() => void>();

function read(): string[] {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    const parsed = raw ? (JSON.parse(raw) as unknown) : [];
    // Anything could be in storage — a hand edit, or an older shape.
    return Array.isArray(parsed)
      ? parsed.filter((v) => typeof v === "string")
      : [];
  } catch {
    return [];
  }
}

// Cached: `getSnapshot` must return a stable reference or React re-renders
// forever. Sorted so the same set always produces the same array.
let snapshot: string[] = read().sort();

function write(next: string[]): void {
  const sorted = [...next].sort();
  if (
    sorted.length === snapshot.length &&
    sorted.every((h, i) => h === snapshot[i])
  ) {
    return;
  }
  snapshot = sorted;
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(sorted));
  } catch {
    // Unwritable storage means the choice won't survive a reload — but it
    // should still take effect for this visit.
  }
  for (const listener of listeners) listener();
}

function subscribe(onChange: () => void): () => void {
  listeners.add(onChange);
  return () => {
    listeners.delete(onChange);
  };
}

function getSnapshot(): string[] {
  return snapshot;
}

/** Show or hide one nav entry, keyed by its href. */
export function toggleNavHidden(href: string): void {
  write(
    snapshot.includes(href)
      ? snapshot.filter((h) => h !== href)
      : [...snapshot, href],
  );
}

/** Show every entry again. */
export function resetNavHidden(): void {
  write([]);
}

/** The hrefs hidden from the sidebar, and whether a given one is hidden. */
export function useHiddenNav(): {
  hidden: string[];
  isHidden: (href: string) => boolean;
} {
  const hidden = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  return { hidden, isHidden: (href) => hidden.includes(href) };
}
