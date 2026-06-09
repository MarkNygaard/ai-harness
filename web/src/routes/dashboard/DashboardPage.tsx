import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { AppShell } from "@/components/AppShell";
import { SubscriptionsRow } from "@/components/dashboard/SubscriptionsRow";
import { Badge } from "@/components/ui/badge";
import { useRuns } from "@/lib/runs";
import { useProjects } from "@/lib/projects";
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
 * its workflows (`idea-to-pr` builds it, then `merge-pr` merges it, etc.). We
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
        <SubscriptionsRow />
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
        {!runs.isLoading && !runs.isError && days.length === 0 && (
          <p className="text-sm text-muted-foreground">
            {filter === ALL ? "No runs yet. " : "No runs for this project. "}
            <Link to="/runs" className="underline">
              Start one
            </Link>
            .
          </p>
        )}

        {days.map((day) => (
          <section key={day.label} className="flex flex-col gap-1">
            <h2 className="px-1 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
              {day.label}
            </h2>
            <div className="overflow-hidden rounded-md border border-border">
              {day.tasks.map((t, i) => (
                <TaskRow key={t.key} task={t} divider={i > 0} />
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

/** One dense line per task: title · project · workflow journey · finish time. */
function TaskRow({ task, divider }: { task: Task; divider: boolean }) {
  // Title links to the build run (full pipeline graph) when present.
  const primary =
    task.runs.find((r) => r.workflow_name.includes("idea-to-pr")) ??
    task.runs[task.runs.length - 1];
  const finishedTime = new Date(task.finishedAt).toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
  });
  return (
    <div
      className={`flex items-center gap-2 px-3 py-1.5 text-sm hover:bg-muted/40 ${
        divider ? "border-t border-border" : ""
      }`}
    >
      <Link
        to={`/runs/${primary.id}`}
        className="min-w-0 flex-1 truncate font-medium hover:underline"
      >
        {task.title}
      </Link>
      {task.project && (
        <Badge variant="outline" className="shrink-0 text-[10px]">
          {task.project}
        </Badge>
      )}
      <div className="flex shrink-0 items-center gap-1">
        {task.runs.map((r) => (
          <Link
            key={r.id}
            to={`/runs/${r.id}`}
            title={`${r.workflow_name} — ${r.status}`}
          >
            <Badge
              variant={STATUS_VARIANT[r.status] ?? "default"}
              className="font-mono text-[10px] hover:opacity-80"
            >
              {r.workflow_name}
            </Badge>
          </Link>
        ))}
      </div>
      <span className="w-12 shrink-0 text-right font-mono text-[11px] text-muted-foreground">
        {finishedTime}
      </span>
    </div>
  );
}
