import { Badge } from "@/components/ui/badge";
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
      <CardContent className="flex flex-col gap-3">
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

/** "Max 20x", "Max 5x", "Pro", or the size for anything else. */
function planName(weight: number | undefined): string {
  if (weight === 1) return "Pro";
  if (weight != null && Number.isInteger(weight) && weight > 1)
    return `Max ${weight}x`;
  return weight != null ? `size ${weight}` : "";
}

/** "5h" for the 5-hour window, "Week" for the weekly one. */
function shortWindow(label: string): string {
  return windowRank(label) === 0 ? "Week" : "5h";
}

/**
 * Each window's usage across several accounts, weighted by plan size: what is
 * used of everything they hold together. A Max 20x counts twenty times a Pro.
 */
export function pooledWindows(subs: SubscriptionUsage[]): UsageWindow[] {
  const sums = new Map<string, { used: number; weight: number }>();
  for (const s of subs) {
    if (!s.available) continue;
    const weight = s.weight ?? 1;
    for (const w of s.windows) {
      const acc = sums.get(w.label) ?? { used: 0, weight: 0 };
      acc.used += w.usedPct * weight;
      acc.weight += weight;
      sums.set(w.label, acc);
    }
  }
  return [...sums.entries()]
    .map(([label, { used, weight }]) => ({
      label,
      usedPct: weight > 0 ? used / weight : 0,
      // Each account resets on its own clock: shown per account below.
      resetsAt: null,
    }))
    .sort((a, b) => windowRank(a.label) - windowRank(b.label));
}

function AccountLine({ sub }: { sub: SubscriptionUsage }) {
  const windows = [...sub.windows].sort(
    (a, b) => windowRank(a.label) - windowRank(b.label),
  );
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-baseline justify-between gap-2">
        <span className="flex min-w-0 items-baseline gap-1.5">
          <span className="truncate text-xs font-medium">
            {sub.accountLabel ?? sub.label}
          </span>
          <span className="shrink-0 text-[11px] text-muted-foreground">
            {planName(sub.weight)}
          </span>
        </span>
        {sub.next && (
          <Badge variant="secondary" title="A run starting now takes this one">
            next
          </Badge>
        )}
      </div>
      {sub.available && windows.length > 0 ? (
        <div className="grid grid-cols-2 gap-3">
          {windows.map((w) => (
            <div
              key={w.label}
              className="flex items-center gap-1.5"
              title={`${w.label}: ${Math.round(w.usedPct)}% used. ${resetsLabel(w.resetsAt)}`}
            >
              <span className="w-8 shrink-0 text-[11px] text-muted-foreground">
                {shortWindow(w.label)}
              </span>
              <UsageBar pct={w.usedPct} />
              <span className="w-8 shrink-0 text-right text-[11px] tabular-nums">
                {Math.round(w.usedPct)}%
              </span>
            </div>
          ))}
        </div>
      ) : (
        <p className="text-[11px] text-muted-foreground">
          {sub.error ?? "Usage unavailable."}
        </p>
      )}
    </div>
  );
}

/** One group per provider, in the order they arrive; accounts stay together. */
export function groupByProvider(
  subs: SubscriptionUsage[],
): SubscriptionUsage[][] {
  const groups = new Map<string, SubscriptionUsage[]>();
  for (const s of subs) {
    const g = groups.get(s.cli);
    if (g) g.push(s);
    else groups.set(s.cli, [s]);
  }
  return [...groups.values()];
}

/**
 * Several accounts of one provider as one card: the pool's usage together,
 * then a line per account.
 */
function PooledCard({ subs }: { subs: SubscriptionUsage[] }) {
  const pooled = pooledWindows(subs);
  const title = subs[0].label.split(" · ")[0];
  return (
    <Card>
      <CardContent className="flex flex-col gap-3">
        <div className="flex items-baseline justify-between gap-2">
          <span className="text-sm font-medium">{title}</span>
          <span className="text-[11px] text-muted-foreground">
            {subs.length} accounts
          </span>
        </div>
        {pooled.length > 0 && (
          <div className="flex flex-col gap-3">
            {pooled.map((w) => (
              <WindowRow key={w.label} w={w} />
            ))}
            <p className="-mt-1 text-[11px] text-muted-foreground">
              All accounts together, weighted by plan.
            </p>
          </div>
        )}
        <div className="flex flex-col gap-2.5 border-t border-border pt-3">
          {subs.map((s) => (
            <AccountLine key={s.account} sub={s} />
          ))}
        </div>
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
        {groupByProvider(subs).map((group) =>
          group.length > 1 ? (
            <PooledCard key={group[0].cli} subs={group} />
          ) : (
            <SubscriptionCard key={group[0].cli} sub={group[0]} />
          ),
        )}
      </div>
    </section>
  );
}
