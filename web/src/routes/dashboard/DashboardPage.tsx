import { RUN_STATUS_VARIANT } from "@/components/runflow/format";
import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { AppShell } from "@/components/AppShell";
import { MetricsPanel } from "@/components/dashboard/MetricsPanel";
import { SubscriptionsRow } from "@/components/dashboard/SubscriptionsRow";
import { PersonBadge } from "@/components/PersonBadge";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import { PERIODS, type Period } from "@/lib/dashboard-metrics";
import { useRuns } from "@/lib/runs";
import { useProjects } from "@/lib/projects";
import type { RunSummary } from "@/types/run";

/** Sentinel filter value meaning "all projects". */
const ALL = "__all__";

const startOfDay = (x: Date) =>
  new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();

/** "Today" / "Yesterday" / a locale date, relative to `now` (local time). */
function dayLabel(iso: string, now: Date): string {
  const d = new Date(iso);
  const diffDays = Math.round((startOfDay(now) - startOfDay(d)) / 86_400_000);
  if (diffDays <= 0) return "Today";
  if (diffDays === 1) return "Yesterday";
  return d.toLocaleDateString(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
    year: d.getFullYear() === now.getFullYear() ? undefined : "numeric",
  });
}

/**
 * A task = all runs sharing a title — i.e. the same Linear issue moving through
 * its workflows (`task` builds it, then `task-merge` merges it, etc.). We
 * collapse them to one row so a task's whole journey is a single line.
 */
interface Task {
  key: string;
  title: string;
  project: string | null;
  /** Chronological: oldest (build) → newest (merge). */
  runs: RunSummary[];
  /** Latest run's `recorded_at` — when the task last finished. */
  finishedAt: string;
}

function groupIntoTasks(runs: RunSummary[]): Task[] {
  // `runs` arrive newest-first. Group by title, preserving order.
  const byKey = new Map<string, RunSummary[]>();
  for (const r of runs) {
    const key = r.title || r.id;
    let arr = byKey.get(key);
    if (!arr) {
      arr = [];
      byKey.set(key, arr);
    }
    arr.push(r);
  }
  const tasks = Array.from(byKey.entries()).map(([key, group]) => ({
    key,
    title: group[0].title || group[0].workflow_name,
    project: group[0].project,
    runs: [...group].reverse(), // chronological
    finishedAt: group[0].recorded_at, // newest
  }));
  tasks.sort((a, b) => Date.parse(b.finishedAt) - Date.parse(a.finishedAt));
  return tasks;
}

interface DayGroup {
  label: string;
  tasks: Task[];
}

function groupByDay(tasks: Task[], now: Date): DayGroup[] {
  const groups: DayGroup[] = [];
  let current: DayGroup | null = null;
  for (const t of tasks) {
    const label = dayLabel(t.finishedAt, now);
    if (!current || current.label !== label) {
      current = { label, tasks: [] };
      groups.push(current);
    }
    current.tasks.push(t);
  }
  return groups;
}

export function DashboardPage() {
  const runs = useRuns();
  const projects = useProjects();
  const [filter, setFilter] = useState<string>(ALL);
  const [period, setPeriod] = useState<Period>(7);
  const now = new Date();

  const days = useMemo(() => {
    const all = runs.data ?? [];
    const scoped =
      filter === ALL ? all : all.filter((r) => (r.project ?? "") === filter);
    return groupByDay(groupIntoTasks(scoped), now);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [runs.data, filter]);

  return (
    <AppShell title="Dashboard">
      <div className="flex w-full flex-col gap-4 p-6">
        {/* One row of filters, scoping everything below it. */}
        <div className="flex flex-wrap items-center gap-2">
          <FilterButton active={filter === ALL} onClick={() => setFilter(ALL)}>
            All projects
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
          <div
            className="ml-auto flex rounded-md border border-input p-0.5"
            role="group"
            aria-label="Period"
          >
            {PERIODS.map((d) => (
              <button
                key={d}
                type="button"
                onClick={() => setPeriod(d)}
                aria-pressed={period === d}
                className={
                  period === d
                    ? "rounded-sm bg-secondary px-3 py-1 text-[12px] font-medium"
                    : "rounded-sm px-3 py-1 text-[12px] text-muted-foreground hover:text-foreground"
                }
              >
                {d} days
              </button>
            ))}
          </div>
        </div>

        {/* The latest tasks beside the subscriptions, the list only as tall as
            the cards next to it and scrolling within that — most people want
            the last few, and the history is a scroll away. */}
        <div className="grid gap-3 lg:grid-cols-3">
          <section className="relative flex min-h-[24rem] flex-col gap-2 lg:col-span-2">
            <div className="flex flex-col gap-2 lg:absolute lg:inset-0">
              <h2 className="px-1 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
                Recent tasks
              </h2>
              <Card className="min-h-0 flex-1 py-0">
                <div className="max-h-[28rem] overflow-y-auto px-3 py-2 lg:max-h-none lg:h-full">
                  {runs.isLoading && (
                    <p className="text-sm text-muted-foreground">Loading…</p>
                  )}
                  {runs.isError && (
                    <p className="text-sm text-destructive">
                      Failed to load runs: {runs.error.message}
                    </p>
                  )}
                  {!runs.isLoading && !runs.isError && days.length === 0 && (
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
                  <div className="flex flex-col gap-3">
                    {days.map((day) => (
                      <section
                        key={day.label}
                        className="flex flex-col gap-0.5"
                      >
                        <h3 className="flex items-center gap-2 px-1 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
                          <span
                            className="h-3 w-0.5 bg-status-running"
                            aria-hidden
                          />
                          {day.label}
                        </h3>
                        <div className="flex flex-col">
                          {day.tasks.map((t, i) => (
                            <TaskRow key={t.key} task={t} divider={i > 0} />
                          ))}
                        </div>
                      </section>
                    ))}
                  </div>
                </div>
              </Card>
            </div>
          </section>

          <div className="lg:col-span-1">
            <SubscriptionsRow vertical />
          </div>
        </div>

        <section className="flex flex-col gap-2">
          <h2 className="px-1 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
            How it&rsquo;s going — last {period} days
            {filter !== ALL ? ` · ${filter}` : ""}
          </h2>
          <MetricsPanel
            days={period}
            project={filter === ALL ? null : filter}
          />
        </section>
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

/**
 * One line per task: finish time (left) · title (plain text) · project ·
 * workflow labels. Only the workflow labels navigate — to their run.
 *
 * Each workflow label carries who started that run, as initials tucked over
 * its left edge. A task's runs can come from different people (one builds,
 * another asks for changes), and initials gathered in one group could not say
 * which run was whose.
 */
function TaskRow({ task, divider }: { task: Task; divider: boolean }) {
  const finishedTime = new Date(task.finishedAt).toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
  });
  return (
    <div
      className={`flex items-center gap-3 px-1 py-1.5 text-sm hover:bg-muted/30 ${
        divider ? "border-t border-border/50" : ""
      }`}
    >
      <span className="w-16 shrink-0 font-mono text-[11px] text-muted-foreground">
        {finishedTime}
      </span>
      <span className="min-w-0 flex-1 truncate">{task.title}</span>
      {task.project && (
        <Badge variant="outline" className="shrink-0 text-[10px]">
          {task.project}
        </Badge>
      )}
      <div className="flex shrink-0 items-center gap-1.5">
        {task.runs.map((r) => (
          <Link
            key={r.id}
            to={`/runs/${r.id}`}
            title={`${r.workflow_name} — ${r.status}`}
            className="flex items-center"
          >
            {/* Over the label's left edge, above it, ringed so the overlap
                reads as one person on one run rather than a clipped circle. */}
            <PersonBadge
              actor={r.trigger_actor}
              source={r.trigger_source}
              size="sm"
              className="relative z-10 -mr-1.5 ring-2 ring-background"
            />
            <Badge
              variant={RUN_STATUS_VARIANT[r.status] ?? "default"}
              className="font-mono text-[10px] hover:opacity-80"
            >
              {r.workflow_name}
            </Badge>
          </Link>
        ))}
      </div>
    </div>
  );
}
