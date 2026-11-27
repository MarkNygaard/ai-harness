/**
 * Several Claude subscriptions. The first is the `claude` credential; each
 * further one is `claude:<id>`, with its own Claude Code config folder. A run
 * takes the one with the most room, weighted by plan size.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./api";

export interface ClaudeAccount {
  id: string;
  label: string;
  /** The plan read from the credential, e.g. `default_claude_max_5x`. */
  tier: string | null;
  /** The plan's size relative to Pro: 1, 5, 20. */
  weight: number;
  /** `override` (set here), `credential` (read from it) or `default`. */
  weight_source: "override" | "credential" | "default";
  connected: boolean;
}

export interface ClaudeAccountRow {
  account: ClaudeAccount;
  /** Runs using it right now. */
  running: number;
}

const KEY = ["claude-accounts"];

export function useClaudeAccounts() {
  return useQuery<ClaudeAccountRow[], Error>({
    queryKey: KEY,
    queryFn: async ({ signal }) =>
      (
        await apiJson<{ accounts: ClaudeAccountRow[] }>(
          "/api/claude-accounts",
          { signal },
        )
      ).accounts,
    retry: false,
    refetchInterval: 30_000,
  });
}

export function useAddClaudeAccount() {
  const qc = useQueryClient();
  return useMutation<{ id: string; provider: string }, Error, string>({
    mutationFn: (label) =>
      apiJson("/api/claude-accounts", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ label }),
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: KEY });
      qc.invalidateQueries({ queryKey: ["credentials"] });
    },
  });
}

export function useUpdateClaudeAccount() {
  const qc = useQueryClient();
  return useMutation<
    unknown,
    Error,
    { id: string; label?: string; plan_weight?: number }
  >({
    mutationFn: ({ id, ...body }) =>
      apiJson(`/api/claude-accounts/${encodeURIComponent(id)}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: KEY }),
  });
}

export function useRemoveClaudeAccount() {
  const qc = useQueryClient();
  return useMutation<void, Error, string>({
    // 204 with no body, so not `apiJson`.
    mutationFn: async (id) => {
      await apiFetch(`/api/claude-accounts/${encodeURIComponent(id)}`, {
        method: "DELETE",
      });
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: KEY });
      qc.invalidateQueries({ queryKey: ["usage"] });
    },
  });
}

/** "Max 5x", "Max 20x", "Pro", or the raw tier when it is something else. */
export function describePlan(a: ClaudeAccount): string {
  const t = (a.tier ?? "").toLowerCase();
  const max = /max_(\d+)x/.exec(t);
  const plan = max ? `Max ${max[1]}x` : t.includes("pro") ? "Pro" : a.tier;
  if (a.weight_source === "override") return `size ${a.weight} (set here)`;
  if (!plan) return "plan unknown, counted as Pro";
  return plan;
}
