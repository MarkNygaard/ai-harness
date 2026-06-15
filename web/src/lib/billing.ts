/**
 * Data layer for per-lane **billing profiles** (`/api/billing-profiles`).
 * A profile maps a model lane (`claude`/`gpt`/`kimi`/`composer`) to its billing
 * mode and, for subscriptions, the monthly price + estimated monthly list-$
 * value — from which an effective-cost multiplier is derived.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";

export type BillingMode = "usage_based" | "subscription";

export interface BillingProfile {
  lane: string;
  billing_mode: BillingMode | string;
  monthly_price_usd: number;
  /** List-$ value the plan yields per month (auto-measured for calibrated lanes). */
  est_monthly_value_usd: number | null;
  updated_at: string;
}

export function useBillingProfiles() {
  return useQuery<BillingProfile[], Error>({
    queryKey: ["billing-profiles"],
    queryFn: ({ signal }) =>
      apiJson<BillingProfile[]>("/api/billing-profiles", { signal }),
  });
}

export interface SaveBillingProfile {
  lane: string;
  billing_mode: BillingMode;
  monthly_price_usd: number;
  est_monthly_value_usd: number | null;
}

export function useSaveBillingProfile() {
  const qc = useQueryClient();
  return useMutation<BillingProfile, Error, SaveBillingProfile>({
    mutationFn: ({ lane, ...body }) =>
      apiJson<BillingProfile>(
        `/api/billing-profiles/${encodeURIComponent(lane)}`,
        {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        },
      ),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["billing-profiles"] }),
  });
}

/**
 * Effective-cost multiplier for a profile — mirrors
 * `BillingProfile::effective_multiplier` in `harness-persist`. `1` for
 * usage-based or until a subscription's monthly value is known.
 */
export function effectiveMultiplier(p: BillingProfile | undefined): number {
  if (!p || p.billing_mode !== "subscription") return 1;
  const v = p.est_monthly_value_usd;
  if (v && v > 0 && p.monthly_price_usd > 0) return p.monthly_price_usd / v;
  return 1;
}

/** The lanes worth configuring, with display metadata for the editor. */
export interface LaneMeta {
  lane: string;
  label: string;
  hint: string;
  defaultMode: BillingMode;
  /** Auto-calibrated from the weekly usage gauge (kimi, gpt). */
  calibrated: boolean;
}

/** Credential provider id → billing lane (for co-locating cost on the
 *  Credentials page). `github` has no model lane. */
export const LANE_FOR_CREDENTIAL: Record<string, string> = {
  claude: "claude",
  codex: "gpt",
  kimi: "kimi",
  cursor: "composer",
};

export const KNOWN_LANES: LaneMeta[] = [
  {
    lane: "kimi",
    label: "Kimi",
    hint: "Kimi-for-Coding subscription. Estimated value is auto-measured from the weekly usage gauge.",
    defaultMode: "subscription",
    calibrated: true,
  },
  {
    lane: "gpt",
    label: "ChatGPT / Codex",
    hint: "gpt-5.5 via Codex. Estimated value is auto-measured from the weekly usage gauge.",
    defaultMode: "subscription",
    calibrated: true,
  },
  {
    lane: "claude",
    label: "Claude (Max)",
    hint: "Anthropic subscription. The usage gauge reads the live 5-hour and weekly windows, but the estimated value stays manual. Enter the price in USD (€105 ≈ $113).",
    defaultMode: "subscription",
    calibrated: false,
  },
  {
    lane: "composer",
    label: "Cursor (Composer)",
    hint: "Usage-based dollar pool — billed per token, so effective cost ≈ notional. No estimate needed.",
    defaultMode: "usage_based",
    calibrated: false,
  },
];
