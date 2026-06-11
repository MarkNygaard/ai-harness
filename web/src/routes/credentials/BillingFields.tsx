import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import {
  KNOWN_LANES,
  effectiveMultiplier,
  useBillingProfiles,
  useSaveBillingProfile,
  type BillingMode,
  type BillingProfile,
  type LaneMeta,
} from "@/lib/billing";

const inputCls =
  "h-8 w-28 rounded-md border border-input bg-transparent px-2.5 text-[12px] outline-none focus:ring-2 focus:ring-ring tabular-nums";

/**
 * Compact subscription-cost editor for one model lane, embedded in that
 * provider's credential card. Sets billing mode + monthly price (+ estimated
 * monthly value for non-calibrated subscriptions) so cost can be shown as
 * *effective* (what you actually pay), not just *notional* (API list price).
 */
export function BillingFields({ lane }: { lane: string }) {
  const meta = KNOWN_LANES.find((l) => l.lane === lane);
  const profiles = useBillingProfiles();
  if (!meta) return null;
  if (!profiles.data) return null; // wait so the form seeds from stored values
  const profile = profiles.data.find((p) => p.lane === lane);
  return (
    <BillingForm
      key={`${lane}:${profile?.updated_at ?? "new"}`}
      meta={meta}
      profile={profile}
    />
  );
}

function BillingForm({
  meta,
  profile,
}: {
  meta: LaneMeta;
  profile: BillingProfile | undefined;
}) {
  const save = useSaveBillingProfile();
  const [mode, setMode] = useState<BillingMode>(
    (profile?.billing_mode as BillingMode) ?? meta.defaultMode,
  );
  const [price, setPrice] = useState(
    profile ? String(profile.monthly_price_usd) : "",
  );
  const [estValue, setEstValue] = useState(
    profile?.est_monthly_value_usd != null
      ? String(profile.est_monthly_value_usd)
      : "",
  );

  const priceNum = parseFloat(price) || 0;
  const estNum = estValue.trim() === "" ? null : parseFloat(estValue) || 0;
  const isSub = mode === "subscription";
  const preview = effectiveMultiplier({
    lane: meta.lane,
    billing_mode: mode,
    monthly_price_usd: priceNum,
    est_monthly_value_usd: estNum,
    updated_at: "",
  });

  const onSave = () =>
    save.mutate({
      lane: meta.lane,
      billing_mode: mode,
      monthly_price_usd: priceNum,
      est_monthly_value_usd: isSub ? estNum : null,
    });

  return (
    <div className="mt-3 flex flex-col gap-2 border-t border-border pt-3">
      <div className="flex items-center gap-2">
        <span className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
          Subscription cost
        </span>
        {meta.calibrated && (
          <Badge variant="secondary" className="text-[10px]">
            value auto-measured
          </Badge>
        )}
      </div>
      <p className="text-[11px] text-muted-foreground">{meta.hint}</p>

      <div className="flex flex-wrap items-end gap-3">
        <label className="flex flex-col gap-1 text-[11px] text-muted-foreground">
          Mode
          <select
            value={mode}
            onChange={(e) => setMode(e.target.value as BillingMode)}
            className={`${inputCls} w-36`}
          >
            <option value="subscription">subscription</option>
            <option value="usage_based">usage-based</option>
          </select>
        </label>

        <label className="flex flex-col gap-1 text-[11px] text-muted-foreground">
          Monthly price (USD)
          <input
            type="number"
            min="0"
            step="0.01"
            value={price}
            onChange={(e) => setPrice(e.target.value)}
            placeholder="0.00"
            className={inputCls}
          />
        </label>

        <label className="flex flex-col gap-1 text-[11px] text-muted-foreground">
          Est. monthly value (USD)
          <input
            type="number"
            min="0"
            step="1"
            value={estValue}
            onChange={(e) => setEstValue(e.target.value)}
            placeholder={!isSub ? "n/a" : meta.calibrated ? "auto" : "e.g. 400"}
            disabled={!isSub}
            className={`${inputCls} disabled:opacity-40`}
          />
        </label>

        <div className="flex flex-col gap-1 text-[11px] text-muted-foreground">
          Effective
          <span className="flex h-8 items-center font-mono text-[12px] text-foreground">
            {preview.toFixed(3)}× notional
          </span>
        </div>

        <Button
          type="button"
          onClick={onSave}
          disabled={save.isPending}
          className="ml-auto"
        >
          {save.isPending ? "Saving…" : "Save"}
        </Button>
      </div>

      {save.isError && (
        <p className="text-[11px] text-destructive">{save.error.message}</p>
      )}
    </div>
  );
}
