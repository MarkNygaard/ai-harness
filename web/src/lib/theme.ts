/**
 * Light / dark theme preference.
 *
 * The palette itself already lives in `styles/globals.css`: `:root` holds the
 * light tokens and `.dark` the dark ones. All this does is decide whether
 * `.dark` is on `<html>`.
 *
 * Three states, not two. **system** follows the OS and keeps following it as the
 * OS flips (many desktops switch at sunset); **light** and **dark** pin it.
 *
 * The stored choice is also read by a small inline script in `index.html` that
 * runs before this module loads — otherwise the page paints in the wrong theme
 * and visibly repaints once React mounts. That script duplicates
 * [`THEME_STORAGE_KEY`] and the resolution rule below; they have to stay in step.
 */
import { useSyncExternalStore } from "react";

export type ThemePreference = "light" | "dark" | "system";
export type ResolvedTheme = "light" | "dark";

/** Also hardcoded in the `index.html` pre-paint script. */
export const THEME_STORAGE_KEY = "harness.theme";

const DARK_QUERY = "(prefers-color-scheme: dark)";

function isPreference(value: unknown): value is ThemePreference {
  return value === "light" || value === "dark" || value === "system";
}

/**
 * The stored choice, or `system` when there isn't one.
 *
 * Storage can throw outright, not just come back empty — a browser set to block
 * site data, or a private window — so this never assumes it is readable.
 */
export function readThemePreference(): ThemePreference {
  try {
    const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
    return isPreference(stored) ? stored : "system";
  } catch {
    return "system";
  }
}

/** Whether the OS currently asks for dark. `false` where it can't be asked. */
export function systemPrefersDark(): boolean {
  if (
    typeof window === "undefined" ||
    typeof window.matchMedia !== "function"
  ) {
    return false;
  }
  return window.matchMedia(DARK_QUERY).matches;
}

/** Which palette a preference actually means right now. */
export function resolveTheme(preference: ThemePreference): ResolvedTheme {
  if (preference === "system") return systemPrefersDark() ? "dark" : "light";
  return preference;
}

/** Put the resolved theme on `<html>`, where the CSS variables key off it. */
export function applyTheme(resolved: ResolvedTheme): void {
  if (typeof document === "undefined") return;
  document.documentElement.classList.toggle("dark", resolved === "dark");
}

// ── The store ────────────────────────────────────────────────────────────────
//
// A module-level store rather than a context provider: the toggle sets the
// theme and unrelated components (the run graph, the workflow editor) read it,
// with no shared ancestor to hang a provider on. `useSyncExternalStore` keeps
// every reader in step without one.

type Snapshot = { preference: ThemePreference; resolved: ResolvedTheme };

const listeners = new Set<() => void>();

function initial(): Snapshot {
  const preference =
    typeof window === "undefined" ? "system" : readThemePreference();
  return { preference, resolved: resolveTheme(preference) };
}

// Cached, and only replaced when something actually changed: `getSnapshot` must
// return a stable reference or React re-renders forever.
let snapshot: Snapshot = initial();

function update(preference: ThemePreference): void {
  const resolved = resolveTheme(preference);
  if (snapshot.preference === preference && snapshot.resolved === resolved) {
    return;
  }
  snapshot = { preference, resolved };
  applyTheme(resolved);
  for (const listener of listeners) listener();
}

// Follow the OS while the preference is `system`. Re-resolving under any
// preference is harmless: `update` is a no-op when nothing changed.
if (typeof window !== "undefined" && typeof window.matchMedia === "function") {
  window
    .matchMedia(DARK_QUERY)
    .addEventListener("change", () => update(snapshot.preference));
}

function subscribe(onChange: () => void): () => void {
  listeners.add(onChange);
  return () => {
    listeners.delete(onChange);
  };
}

function getSnapshot(): Snapshot {
  return snapshot;
}

/** Choose a theme, and remember it for next time. */
export function setThemePreference(preference: ThemePreference): void {
  try {
    window.localStorage.setItem(THEME_STORAGE_KEY, preference);
  } catch {
    // Unwritable storage means the choice won't survive a reload — but it
    // should still take effect for this visit.
  }
  update(preference);
}

/** The current preference, what it resolves to, and how to change it. */
export function useTheme(): Snapshot & {
  setPreference: (preference: ThemePreference) => void;
} {
  const current = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  return { ...current, setPreference: setThemePreference };
}
