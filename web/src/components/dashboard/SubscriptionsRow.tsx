import { Card, CardContent } from "@/components/ui/card";
import {
  useUsage,
  type SubscriptionUsage,
  type UsageWindow,
} from "@/lib/usage";

/** "Resets in 12d" / "Resets in 22h" / "Resets in 14m" from an absolute time. */
function resetsLabel(iso: string | null): string {
  if (!iso) return "";
  const ms = Date.parse(iso) - Date.now();
  if (Number.isNaN(ms) || ms <= 0) return "resets soon";
  const hours = Math.round(ms / 3_600_000);
  if (hours >= 48) return `Resets in ${Math.round(hours / 24)}d`;
  if (hours >= 1) return `Resets in ${hours}h`;
  return `Resets in ${Math.max(1, Math.round(ms / 60_000))}m`;
}

function UsageBar({ pct }: { pct: number }) {
  const clamped = Math.max(0, Math.min(100, pct));
  return (
    <div className="h-1.5 w-full overflow-hidden rounded-full bg-muted">
      <div
        className="h-full rounded-full bg-foreground"
        style={{ width: `${clamped}%` }}
      />
    </div>
  );
}

function WindowRow({ w }: { w: UsageWindow }) {
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-baseline justify-between gap-2">
        <span className="text-xs text-muted-foreground">{w.label}</span>
        <span className="text-[11px] text-muted-foreground">
          {resetsLabel(w.resetsAt)}
        </span>
      </div>
      {w.amount != null ? (
        // No readable quota — show the absolute figure, not a percent bar.
        <div className="flex items-baseline gap-2">
          <span className="text-sm font-semibold tabular-nums">{w.amount}</span>
          {w.caption && (
            <span className="text-[11px] text-muted-foreground">
              {w.caption}
            </span>
          )}
        </div>
      ) : (
        <div className="flex items-center gap-2">
          <span className="w-9 shrink-0 text-sm font-semibold tabular-nums">
            {Math.round(w.usedPct)}%
          </span>
          <UsageBar pct={w.usedPct} />
        </div>
      )}
    </div>
  );
}

/** Longer-period windows first (7-day/weekly above the 5-hour). */
function windowRank(label: string): number {
  const l = label.toLowerCase();
  return l.includes("week") || l.includes("day") || l.includes("7") ? 0 : 1;
}

function SubscriptionCard({ sub }: { sub: SubscriptionUsage }) {
  const windows = [...sub.windows].sort(
    (a, b) => windowRank(a.label) - windowRank(b.label),
  );
  return (
    <Card>
      <CardContent className="flex flex-col gap-3 p-4">
        <div className="text-sm font-medium">{sub.label}</div>
        {sub.available && windows.length > 0 ? (
          <div className="flex flex-col gap-3">
            {windows.map((w) => (
              <WindowRow key={w.label} w={w} />
            ))}
          </div>
        ) : (
          <p className="text-xs text-muted-foreground">
            {sub.error ?? "Usage unavailable."}
          </p>
        )}
      </CardContent>
    </Card>
  );
}

/**
 * "Subscriptions": remaining usage per connected CLI. Hidden entirely when
 * nothing is connected (or the endpoint is unavailable). `vertical` stacks the
 * cards in one column — for the dashboard's narrow right rail.
 */
export function SubscriptionsRow({ vertical = false }: { vertical?: boolean }) {
  const usage = useUsage();
  const subs = usage.data?.subscriptions ?? [];
  if (usage.isLoading || subs.length === 0) return null;
  return (
    <section className="flex flex-col gap-2">
      <h2 className="px-1 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
        Subscriptions
      </h2>
      <div
        className={
          vertical
            ? "flex flex-col gap-3"
            : "grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3"
        }
      >
        {subs.map((s) => (
          <SubscriptionCard key={s.cli} sub={s} />
        ))}
      </div>
    </section>
  );
}
