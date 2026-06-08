import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { ArrowRight } from "lucide-react";
import { AppShell } from "@/components/AppShell";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { useRuns } from "@/lib/runs";
import { useProjects } from "@/lib/projects";
import { elapsedMs, formatDuration } from "@/components/runflow/format";
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

/** Sentinel filter value meaning "every project (and unassigned) runs". */
const ALL = "__all__";

/**
 * Local-time day bucket label for an ISO timestamp, relative to `now`:
 * "Today" / "Yesterday" / a locale date for anything older.
 */
function dayLabel(iso: string, now: Date): string {
  const d = new Date(iso);
  const startOf = (x: Date) =>
    new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const dayMs = 86_400_000;
  const diffDays = Math.round((startOf(now) - startOf(d)) / dayMs);
  if (diffDays <= 0) return "Today";
  if (diffDays === 1) return "Yesterday";
  return d.toLocaleDateString(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
    year: d.getFullYear() === now.getFullYear() ? undefined : "numeric",
  });
}

interface DayGroup {
  label: string;
  runs: RunSummary[];
}

/** Group runs (already newest-first) into ordered local-day sections. */
function groupByDay(runs: RunSummary[], now: Date): DayGroup[] {
  const groups: DayGroup[] = [];
  let current: DayGroup | null = null;
  for (const run of runs) {
    const label = dayLabel(run.recorded_at, now);
    if (!current || current.label !== label) {
      current = { label, runs: [] };
      groups.push(current);
    }
    current.runs.push(run);
  }
  return groups;
}

export function DashboardPage() {
  const runs = useRuns();
  const projects = useProjects();
  const [filter, setFilter] = useState<string>(ALL);
  const now = new Date();

  const filtered = useMemo(() => {
    const all = runs.data ?? [];
    const sorted = [...all].sort(
      (a, b) => Date.parse(b.recorded_at) - Date.parse(a.recorded_at),
    );
    if (filter === ALL) return sorted;
    return sorted.filter((r) => (r.project ?? "") === filter);
  }, [runs.data, filter]);

  const groups = useMemo(() => groupByDay(filtered, now), [filtered, now]);

  return (
    <AppShell title="Dashboard">
      <div className="flex w-full flex-col gap-6 p-6">
        <div className="flex flex-wrap items-center gap-2">
          <FilterButton active={filter === ALL} onClick={() => setFilter(ALL)}>
            All
          </FilterButton>
          {projects.data?.map((p) => (
            <FilterButton
              key={p.name}
              active={filter === p.name}
              onClick={() => setFilter(p.name)}
            >
              {p.name}
            </FilterButton>
          ))}
        </div>

        {runs.isLoading && (
          <p className="text-sm text-muted-foreground">Loading…</p>
        )}
        {runs.isError && (
          <p className="text-sm text-destructive">
            Failed to load runs: {runs.error.message}
          </p>
        )}
        {!runs.isLoading && !runs.isError && filtered.length === 0 && (
          <p className="text-sm text-muted-foreground">
            {filter === ALL
              ? "No runs yet. "
              : "No runs for this project. "}
            <Link to="/runs" className="underline">
              Start one
            </Link>
            .
          </p>
        )}

        {groups.map((group) => (
          <section key={group.label} className="flex flex-col gap-2">
            <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
              {group.label}
            </h2>
            <div className="flex flex-col gap-1.5">
              {group.runs.map((run) => (
                <RunRow key={run.id} run={run} now={now} />
              ))}
            </div>
          </section>
        ))}
      </div>
    </AppShell>
  );
}

function FilterButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`h-7 rounded-full border px-3 text-xs font-medium transition-colors ${
        active
          ? "border-accent-orange/50 bg-accent-orange/10 text-foreground"
          : "border-border text-muted-foreground hover:bg-muted"
      }`}
    >
      {children}
    </button>
  );
}

function RunRow({ run, now }: { run: RunSummary; now: Date }) {
  const duration = formatDuration(
    elapsedMs(run.started_at, run.ended_at, now.getTime()),
  );
  return (
    <Link to={`/runs/${run.id}`} className="group block">
      <Card className="transition-colors group-hover:border-accent-orange/50">
        <CardContent className="flex items-center gap-3 py-2.5">
          <Badge variant={STATUS_VARIANT[run.status] ?? "default"}>
            {run.status}
          </Badge>
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="truncate text-sm font-medium">
                {run.title || run.workflow_name}
              </span>
              {run.project && (
                <Badge variant="outline" className="shrink-0">
                  {run.project}
                </Badge>
              )}
            </div>
            <div className="truncate font-mono text-[11px] text-muted-foreground">
              {run.title ? `${run.workflow_name} · ` : ""}
              {run.id}
            </div>
          </div>
          <div className="text-right text-[11px] text-muted-foreground">
            <div>{duration}</div>
            <div>{run.node_count} steps</div>
          </div>
          <ArrowRight className="h-4 w-4 text-muted-foreground transition-transform group-hover:translate-x-0.5" />
        </CardContent>
      </Card>
    </Link>
  );
}
