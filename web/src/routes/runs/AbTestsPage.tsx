import { Link } from "react-router-dom";
import { ArrowRight, GitCompare } from "lucide-react";
import { AppShell } from "@/components/AppShell";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { useRuns } from "@/lib/runs";
import type { RunStatus, RunSummary } from "@/types/run";

const STATUS_VARIANT: Record<
  RunStatus,
  "running" | "success" | "failed" | "skipped"
> = {
  running: "running",
  completed: "success",
  failed: "failed",
  cancelled: "failed",
};

function relativeTime(iso: string): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return iso;
  const secs = Math.round((Date.now() - then) / 1000);
  if (secs < 60) return `${secs}s ago`;
  const mins = Math.round(secs / 60);
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.round(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  return new Date(then).toLocaleDateString();
}

interface PairRow {
  pairId: string;
  title: string;
  recordedAt: string;
  armA: RunSummary | null;
  armB: RunSummary | null;
}

/** Group the run list into A/B pairs (most recent first). */
function groupPairs(runs: RunSummary[]): PairRow[] {
  const byPair = new Map<string, RunSummary[]>();
  for (const r of runs) {
    if (!r.ab_pair_id) continue;
    const list = byPair.get(r.ab_pair_id) ?? [];
    list.push(r);
    byPair.set(r.ab_pair_id, list);
  }
  const rows: PairRow[] = [];
  for (const [pairId, list] of byPair) {
    const armA = list.find((r) => r.ab_arm === "a") ?? null;
    const armB = list.find((r) => r.ab_arm === "b") ?? null;
    const sample = armA ?? armB ?? list[0];
    rows.push({
      pairId,
      title: sample.title ?? sample.workflow_name,
      recordedAt: sample.recorded_at,
      armA,
      armB,
    });
  }
  rows.sort((x, y) => y.recordedAt.localeCompare(x.recordedAt));
  return rows;
}

/** Lists previous A/B test pairs, each linking straight to its comparison page. */
export function AbTestsPage() {
  const runs = useRuns({});
  const pairs = runs.data ? groupPairs(runs.data) : [];

  return (
    <AppShell title="A/B Tests">
      <div className="mx-auto flex max-w-5xl flex-col gap-6 p-6">
        <section className="flex flex-col gap-2">
          <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            A/B test pairs
          </h2>

          {runs.isLoading && (
            <p className="text-sm text-muted-foreground">Loading…</p>
          )}
          {runs.isError && (
            <p className="text-sm text-destructive">
              {runs.error?.message ?? "Failed to load runs."}
            </p>
          )}
          {runs.data && pairs.length === 0 && (
            <p className="text-sm text-muted-foreground">
              No A/B tests yet. Start one from the{" "}
              <Link to="/runs" className="underline">
                Runs
              </Link>{" "}
              page (the “A/B” trigger) to compare two provider+model arms.
            </p>
          )}

          <div className="flex flex-col gap-1.5">
            {pairs.map((p) => (
              <PairCard key={p.pairId} pair={p} relativeTime={relativeTime} />
            ))}
          </div>
        </section>
      </div>
    </AppShell>
  );
}

function ArmBadge({ arm, run }: { arm: string; run: RunSummary | null }) {
  if (!run)
    return (
      <Badge variant="outline" className="shrink-0">
        {arm}: pending
      </Badge>
    );
  return (
    <Badge
      variant={STATUS_VARIANT[run.status] ?? "default"}
      className="shrink-0"
    >
      {arm}: {run.ab_label ?? run.id}
    </Badge>
  );
}

function PairCard({
  pair,
  relativeTime,
}: {
  pair: PairRow;
  relativeTime: (iso: string) => string;
}) {
  return (
    <Link to={`/runs/pair/${pair.pairId}`} className="group block">
      <Card className="transition-colors group-hover:border-accent-orange/50">
        <CardContent className="flex items-center gap-3 py-2.5">
          <GitCompare className="h-4 w-4 shrink-0 text-muted-foreground" />
          <div className="min-w-0 flex-1">
            <span className="truncate text-sm font-medium">{pair.title}</span>
            <div className="mt-1 flex flex-wrap items-center gap-1.5">
              <ArmBadge arm="A" run={pair.armA} />
              <ArmBadge arm="B" run={pair.armB} />
            </div>
            <div className="mt-1 truncate font-mono text-[11px] text-muted-foreground">
              {pair.pairId}
            </div>
          </div>
          <div className="text-right text-[11px] text-muted-foreground">
            {relativeTime(pair.recordedAt)}
          </div>
          <ArrowRight className="h-4 w-4 text-muted-foreground transition-transform group-hover:translate-x-0.5" />
        </CardContent>
      </Card>
    </Link>
  );
}
