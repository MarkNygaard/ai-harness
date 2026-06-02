import type { NodeStatus, Usage } from "@/types/run";

/** Total billable tokens reported for a node (input + output; nulls = 0). */
export function totalTokens(usage: Usage): number {
  return (usage.input ?? 0) + (usage.output ?? 0);
}

/** Sum every counter across many usages, preserving null when none reported. */
export function sumUsage(usages: Usage[]): Usage {
  const acc: Usage = { input: null, output: null, cache_read: null, cache_write: null };
  for (const u of usages) {
    for (const k of ["input", "output", "cache_read", "cache_write"] as const) {
      if (u[k] != null) acc[k] = (acc[k] ?? 0) + u[k]!;
    }
  }
  return acc;
}

/** Elapsed milliseconds between two ISO timestamps; `end` defaults to `now`. */
export function elapsedMs(
  startedAt: string | null,
  endedAt: string | null,
  now: number,
): number | null {
  if (!startedAt) return null;
  const start = Date.parse(startedAt);
  if (Number.isNaN(start)) return null;
  const end = endedAt ? Date.parse(endedAt) : now;
  return Math.max(0, end - start);
}

/** Human duration: "420ms", "3.4s", "2m 05s", "1h 03m". */
export function formatDuration(ms: number | null): string {
  if (ms == null) return "—";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)}s`;
  const m = Math.floor(s / 60);
  const rem = Math.floor(s % 60);
  if (m < 60) return `${m}m ${String(rem).padStart(2, "0")}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${String(m % 60).padStart(2, "0")}m`;
}

/** Compact token count: 1234 → "1.2k". */
export function formatTokens(n: number | null): string {
  if (n == null) return "—";
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

/** CSS color token for a node/run status. */
export function statusColor(status: NodeStatus): string {
  switch (status) {
    case "running":
      return "var(--status-running)";
    case "success":
      return "var(--status-success)";
    case "failed":
    case "cancelled":
      return "var(--status-failed)";
    default:
      return "var(--status-skipped)";
  }
}

export function statusLabel(status: NodeStatus): string {
  return status.charAt(0).toUpperCase() + status.slice(1);
}
