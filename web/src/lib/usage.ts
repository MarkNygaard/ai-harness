/**
 * Data layer for subscription usage (`GET /api/usage`).
 *
 * One card per connected CLI: Claude (first-party Claude Code limits), and
 * ChatGPT/Codex + Kimi (via the omp auth-broker). The server caches upstream
 * for ~3min, so a modest client refetch interval is safe.
 */
import { useQuery } from "@tanstack/react-query";
import { apiJson } from "./api";

export interface UsageWindow {
  label: string;
  /** Percent of the window consumed (0–100). Ignored when `amount` is set. */
  usedPct: number;
  /** Absolute reset time (RFC3339), when known. */
  resetsAt: string | null;
  /**
   * Preformatted absolute figure (e.g. "$1.86") shown instead of a percent bar,
   * where a percentage would mislead (Cursor — quota isn't readable, so we show
   * a notional cost estimate).
   */
  amount?: string | null;
  /** Short qualifier under the amount (e.g. "notional · API list rates"). */
  caption?: string | null;
}

export interface SubscriptionUsage {
  /** Stable key (`claude` | `codex` | `kimi`). */
  cli: string;
  label: string;
  available: boolean;
  error: string | null;
  windows: UsageWindow[];
}

export interface UsageResponse {
  subscriptions: SubscriptionUsage[];
}

export function useUsage() {
  return useQuery<UsageResponse, Error>({
    queryKey: ["usage"],
    queryFn: ({ signal }) => apiJson<UsageResponse>("/api/usage", { signal }),
    staleTime: 60_000,
    refetchInterval: 120_000,
  });
}
