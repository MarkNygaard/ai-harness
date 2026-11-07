import { useState } from "react";
import { IconArrowDownRight, IconArrowUpRight } from "@tabler/icons-react";
import {
  Bar,
  BarChart,
  CartesianGrid,
  Line,
  LineChart,
  XAxis,
  YAxis,
} from "recharts";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  type ChartConfig,
  ChartContainer,
  ChartLegend,
  ChartLegendContent,
  ChartTooltip,
  ChartTooltipContent,
} from "@/components/ui/chart";
import {
  type Delta,
  type Metrics,
  type Period,
  completionRate,
  dayLabel,
  delta,
  formatDuration,
  formatPercent,
  formatUsd,
  useDashboardMetrics,
} from "@/lib/dashboard-metrics";

/**
 * Data colours, validated with the dataviz palette checker in both modes: the
 * series blue for runs that finished and for every single-series chart, the
 * status red for runs that didn't. Green-for-done was the obvious pairing and
 * fails red-green colour blindness (ΔE 4 against the red), so it isn't used.
 */
const SERIES = { light: "#2a78d6", dark: "#3987e5" };
const CRITICAL = { light: "#d03b3b", dark: "#d03b3b" };
/** Categorical slot 2, beside SERIES for merged against opened PRs. */
const SERIES_2 = { light: "#eb6834", dark: "#d95926" };

const outcomeConfig = {
  completed: { label: "Completed", theme: SERIES },
  failed: { label: "Failed or cancelled", theme: CRITICAL },
} satisfies ChartConfig;

const runTimeConfig = {
  median_secs: { label: "Median run time", theme: SERIES },
} satisfies ChartConfig;

const costConfig = {
  cost_usd: { label: "Notional cost", theme: SERIES },
} satisfies ChartConfig;

const prConfig = {
  prs_opened: { label: "Opened", theme: SERIES },
  prs_merged: { label: "Merged", theme: SERIES_2 },
} satisfies ChartConfig;

/** Workflows shown by name; the rest fold into one "Other" bar. */
const WORKFLOW_ROWS = 8;

/**
 * How the harness has been doing over `days`, for `project` or all of them:
 * four headline numbers against the period before, then runs, run time and
 * cost per day, and the same split per workflow.
 */
export function MetricsPanel({
  days,
  project,
}: {
  days: Period;
  project: string | null;
}) {
  const metrics = useDashboardMetrics(days, project);
  const m = metrics.data;

  if (metrics.isError) {
    return (
      <p className="text-sm text-destructive">
        Couldn&rsquo;t load the metrics: {metrics.error.message}
      </p>
    );
  }
  if (!m) {
    return <p className="text-sm text-muted-foreground">Loading metrics…</p>;
  }

  // While a new filter loads, the old picture stays, dimmed — no flash.
  return (
    <div
      className={`flex flex-col gap-6 transition-opacity ${
        metrics.isPlaceholderData ? "opacity-60" : ""
      }`}
    >
      <StatTiles m={m} />
      {m.totals.runs === 0 ? (
        <Card>
          <CardContent className="py-10 text-center text-sm text-muted-foreground">
            No finished runs in the last {m.days} days
            {project ? ` in ${project}` : ""}.
          </CardContent>
        </Card>
      ) : (
        <div className="grid gap-6 lg:grid-cols-2">
          <RunsPerDay m={m} />
          <RunTimePerDay m={m} />
          <CostPerDay m={m} />
          <ByWorkflow m={m} />
        </div>
      )}
      <PullRequests m={m} project={project} />
    </div>
  );
}

// ── Pull requests ──────────────────────────────────────────────────────────

/**
 * What became of the PRs the runs opened: how many opened and merged, how
 * long a task took from kickoff to its PR, and how long a PR waited to merge.
 */
function PullRequests({ m, project }: { m: Metrics; project: string | null }) {
  const vs = `vs previous ${m.days} days`;
  const t = m.totals;
  const p = m.previous;
  const none = t.prs_opened + t.prs_merged + p.prs_opened + p.prs_merged === 0;
  return (
    <section className="flex flex-col gap-3">
      <h2 className="text-sm font-medium">Pull requests</h2>
      {none ? (
        <Card>
          <CardContent className="py-8 text-center text-sm text-muted-foreground">
            No pull requests from runs in the last {m.days} days
            {project ? ` in ${project}` : ""}. A run whose output links a pull
            request is followed until it merges or closes.
          </CardContent>
        </Card>
      ) : (
        <div className="grid gap-6 lg:grid-cols-2">
          <div className="grid grid-cols-2 content-start gap-3">
            <StatTile
              label="PRs opened"
              value={t.prs_opened.toLocaleString()}
              delta={delta(t.prs_opened, p.prs_opened, { better: "up" })}
              caption={vs}
              help="Pull requests the runs opened in the period."
            />
            <StatTile
              label="PRs merged"
              value={t.prs_merged.toLocaleString()}
              delta={delta(t.prs_merged, p.prs_merged, { better: "up" })}
              caption={vs}
              help="Pull requests from runs that merged in the period, whenever they were opened."
            />
            <StatTile
              label="Kickoff to PR"
              value={formatDuration(t.median_to_pr_secs)}
              delta={delta(t.median_to_pr_secs, p.median_to_pr_secs, {
                better: "down",
              })}
              caption={`median · ${vs}`}
              help="From the run starting to its pull request opening. A PR someone opened before the run started is left out."
            />
            <StatTile
              label="PR to merge"
              value={formatDuration(t.median_to_merge_secs)}
              delta={delta(t.median_to_merge_secs, p.median_to_merge_secs, {
                better: "down",
              })}
              caption={`median · ${vs}`}
              help="From a pull request opening to it merging: the time it spent in review."
            />
          </div>
          <PrsPerDay m={m} />
        </div>
      )}
    </section>
  );
}

function PrsPerDay({ m }: { m: Metrics }) {
  return (
    <ChartCard
      title="Pull requests per day"
      subtitle="Opened, and merged, on each day"
      table={
        <DataTable
          head={["Day", "Opened", "Merged"]}
          rows={[...m.daily]
            .reverse()
            .map((d) => [dayLabel(d.day), d.prs_opened, d.prs_merged])}
        />
      }
    >
      <ChartContainer config={prConfig} className="aspect-auto h-56 w-full">
        <BarChart
          data={m.daily}
          barGap={2}
          margin={{ left: -16, right: 4, top: 8 }}
        >
          <CartesianGrid vertical={false} stroke="var(--border)" />
          <XAxis
            dataKey="day"
            tickFormatter={dayLabel}
            tickLine={false}
            axisLine={false}
            minTickGap={24}
            tick={axisTick}
          />
          <YAxis
            allowDecimals={false}
            tickLine={false}
            axisLine={false}
            tick={axisTick}
            width={40}
          />
          <ChartTooltip
            cursor={false}
            content={
              <ChartTooltipContent
                indicator="line"
                labelFormatter={(v) => dayLabel(String(v))}
              />
            }
          />
          <ChartLegend content={<ChartLegendContent />} />
          <Bar
            isAnimationActive={false}
            dataKey="prs_opened"
            fill="var(--color-prs_opened)"
            radius={[4, 4, 0, 0]}
            maxBarSize={12}
          />
          <Bar
            isAnimationActive={false}
            dataKey="prs_merged"
            fill="var(--color-prs_merged)"
            radius={[4, 4, 0, 0]}
            maxBarSize={12}
          />
        </BarChart>
      </ChartContainer>
    </ChartCard>
  );
}

// ── Headline numbers ───────────────────────────────────────────────────────

function StatTiles({ m }: { m: Metrics }) {
  const rate = completionRate(m.totals);
  const prevRate = completionRate(m.previous);
  const vs = `vs previous ${m.days} days`;
  return (
    <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
      <StatTile
        label="Tasks finished"
        value={m.totals.tasks.toLocaleString()}
        delta={delta(m.totals.tasks, m.previous.tasks, { better: "up" })}
        caption={vs}
        help="Distinct tasks with a completed run. Runs that share a title — an issue built, reviewed and merged — are one task."
      />
      <StatTile
        label="Completion rate"
        value={formatPercent(rate)}
        delta={delta(rate, prevRate, { better: "up", kind: "points" })}
        caption={`${m.totals.completed} of ${m.totals.runs} runs · ${vs}`}
        help="Runs that completed, out of every run that finished. Failed and cancelled runs count against it."
      />
      <StatTile
        label="Median run time"
        value={formatDuration(m.totals.median_secs)}
        delta={delta(m.totals.median_secs, m.previous.median_secs, {
          better: "down",
        })}
        caption={vs}
        help="First step started to last step ended, for runs that completed or failed."
      />
      <StatTile
        label="Notional cost"
        value={formatUsd(m.totals.cost_usd)}
        delta={delta(m.totals.cost_usd, m.previous.cost_usd, {
          better: "none",
        })}
        caption={vs}
        help="The tokens these runs used, priced at API rates. A comparison, not a bill: subscriptions pay for most of it."
      />
    </div>
  );
}

function StatTile({
  label,
  value,
  delta: d,
  caption,
  help,
}: {
  label: string;
  value: string;
  delta: Delta | null;
  caption: string;
  help: string;
}) {
  return (
    <Card size="sm" title={help}>
      <CardContent className="flex flex-col gap-1 px-4 py-3">
        <span className="text-xs text-muted-foreground">{label}</span>
        <span className="text-2xl font-semibold">{value}</span>
        <span className="flex flex-wrap items-center gap-x-2 text-[11px] text-muted-foreground">
          {d && <DeltaText d={d} />}
          <span>{caption}</span>
        </span>
      </CardContent>
    </Card>
  );
}

/** A change, with its direction as an arrow as well as a colour. */
function DeltaText({ d }: { d: Delta }) {
  const up = d.text.startsWith("+");
  const down = d.text.startsWith("−");
  const tone =
    d.good == null
      ? "text-muted-foreground"
      : d.good
        ? "text-status-success"
        : "text-destructive";
  return (
    <span className={`inline-flex items-center gap-0.5 font-medium ${tone}`}>
      {up && <IconArrowUpRight className="size-3" aria-hidden />}
      {down && <IconArrowDownRight className="size-3" aria-hidden />}
      {d.text}
    </span>
  );
}

// ── Chart frame ────────────────────────────────────────────────────────────

/**
 * A chart with its title and a table view of the same numbers — a tooltip is
 * a convenience, never the only way to read a value.
 */
function ChartCard({
  title,
  subtitle,
  table,
  children,
}: {
  title: string;
  subtitle: string;
  table: React.ReactNode;
  children: React.ReactNode;
}) {
  const [asTable, setAsTable] = useState(false);
  return (
    <Card>
      <CardContent className="flex flex-col gap-3 px-4 py-3">
        <div className="flex items-start gap-2">
          <div className="min-w-0 flex-1">
            <h3 className="text-sm font-medium">{title}</h3>
            <p className="text-[11px] text-muted-foreground">{subtitle}</p>
          </div>
          <Button
            variant="ghost"
            size="xs"
            aria-pressed={asTable}
            onClick={() => setAsTable((t) => !t)}
          >
            {asTable ? "Chart" : "Table"}
          </Button>
        </div>
        {asTable ? (
          <div className="max-h-56 overflow-y-auto">{table}</div>
        ) : (
          children
        )}
      </CardContent>
    </Card>
  );
}

function DataTable({
  head,
  rows,
}: {
  head: string[];
  rows: (string | number)[][];
}) {
  return (
    <table className="w-full text-[12px]">
      <thead className="sticky top-0 bg-card text-muted-foreground">
        <tr>
          {head.map((h, i) => (
            <th
              key={h}
              className={`py-1 font-medium ${i === 0 ? "text-left" : "text-right"}`}
            >
              {h}
            </th>
          ))}
        </tr>
      </thead>
      <tbody className="tabular-nums">
        {rows.map((r) => (
          <tr key={String(r[0])} className="border-t border-border/50">
            {r.map((c, i) => (
              <td
                key={i}
                className={`py-1 ${i === 0 ? "text-left" : "text-right"}`}
              >
                {c}
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

// ── Stacked segments ───────────────────────────────────────────────────────

interface SegmentProps {
  x?: number;
  y?: number;
  width?: number;
  height?: number;
  fill?: string;
  payload?: { failed: number };
}

/**
 * One segment of a stacked bar, drawn by hand so it follows the mark spec:
 * the data end is rounded (4px) only on the segment that *is* the end — the
 * failed one, or the completed one on a day with no failures — and a 2px gap
 * in the surface colour separates the two, rather than a stroke.
 */
function segment(key: "completed" | "failed", horizontal: boolean) {
  return function Segment(raw: unknown) {
    const {
      x = 0,
      y = 0,
      width = 0,
      height = 0,
      fill,
      payload,
    } = raw as SegmentProps;
    const failed = payload?.failed ?? 0;
    const isEnd = key === "failed" || failed === 0;
    const gap = key === "completed" && failed > 0 ? 2 : 0;
    if (horizontal) {
      const w = Math.max(0, width - gap);
      if (w <= 0 || height <= 0) return <g />;
      const r = isEnd ? Math.min(4, w, height / 2) : 0;
      const d = `M${x},${y} H${x + w - r} Q${x + w},${y} ${x + w},${y + r} V${
        y + height - r
      } Q${x + w},${y + height} ${x + w - r},${y + height} H${x} Z`;
      return <path d={d} fill={fill} />;
    }
    const h = Math.max(0, height - gap);
    const top = y + gap;
    if (h <= 0 || width <= 0) return <g />;
    const r = isEnd ? Math.min(4, h, width / 2) : 0;
    const d = `M${x},${top + h} V${top + r} Q${x},${top} ${x + r},${top} H${
      x + width - r
    } Q${x + width},${top} ${x + width},${top + r} V${top + h} Z`;
    return <path d={d} fill={fill} />;
  };
}

const VERTICAL_COMPLETED = segment("completed", false);
const VERTICAL_FAILED = segment("failed", false);
const HORIZONTAL_COMPLETED = segment("completed", true);
const HORIZONTAL_FAILED = segment("failed", true);

// ── The charts ─────────────────────────────────────────────────────────────

const axisTick = { fontSize: 11 };

function RunsPerDay({ m }: { m: Metrics }) {
  const data = m.daily.map((d) => ({
    day: d.day,
    completed: d.completed,
    failed: d.failed + d.cancelled,
  }));
  return (
    <ChartCard
      title="Runs per day"
      subtitle="Finished runs, by how they ended"
      table={
        <DataTable
          head={["Day", "Completed", "Failed", "Cancelled"]}
          rows={[...m.daily]
            .reverse()
            .map((d) => [dayLabel(d.day), d.completed, d.failed, d.cancelled])}
        />
      }
    >
      <ChartContainer
        config={outcomeConfig}
        className="aspect-auto h-56 w-full"
      >
        <BarChart data={data} margin={{ left: -16, right: 4, top: 8 }}>
          <CartesianGrid vertical={false} stroke="var(--border)" />
          <XAxis
            dataKey="day"
            tickFormatter={dayLabel}
            tickLine={false}
            axisLine={false}
            minTickGap={24}
            tick={axisTick}
          />
          <YAxis
            allowDecimals={false}
            tickLine={false}
            axisLine={false}
            tick={axisTick}
            width={40}
          />
          <ChartTooltip
            cursor={false}
            content={
              <ChartTooltipContent
                indicator="line"
                labelFormatter={(v) => dayLabel(String(v))}
              />
            }
          />
          <ChartLegend content={<ChartLegendContent />} />
          <Bar
            isAnimationActive={false}
            dataKey="completed"
            stackId="runs"
            fill="var(--color-completed)"
            maxBarSize={24}
            shape={VERTICAL_COMPLETED}
          />
          <Bar
            isAnimationActive={false}
            dataKey="failed"
            stackId="runs"
            fill="var(--color-failed)"
            maxBarSize={24}
            shape={VERTICAL_FAILED}
          />
        </BarChart>
      </ChartContainer>
    </ChartCard>
  );
}

function RunTimePerDay({ m }: { m: Metrics }) {
  // A dot per day reads at a week or a month; at 90 days it is just noise.
  const dots = m.days <= 31;
  return (
    <ChartCard
      title="Median run time"
      subtitle="Per day, for runs that completed or failed"
      table={
        <DataTable
          head={["Day", "Median run time"]}
          rows={[...m.daily]
            .reverse()
            .map((d) => [dayLabel(d.day), formatDuration(d.median_secs)])}
        />
      }
    >
      <ChartContainer
        config={runTimeConfig}
        className="aspect-auto h-56 w-full"
      >
        <LineChart data={m.daily} margin={{ left: -8, right: 8, top: 8 }}>
          <CartesianGrid vertical={false} stroke="var(--border)" />
          <XAxis
            dataKey="day"
            tickFormatter={dayLabel}
            tickLine={false}
            axisLine={false}
            minTickGap={24}
            tick={axisTick}
            padding={{ left: 12, right: 12 }}
          />
          <YAxis
            tickFormatter={(v: number) => formatDuration(v)}
            tickLine={false}
            axisLine={false}
            tick={axisTick}
            width={44}
          />
          <ChartTooltip
            content={
              <ChartTooltipContent
                indicator="line"
                labelFormatter={(v) => dayLabel(String(v))}
                formatter={(value) => (
                  <span className="flex w-full justify-between gap-4">
                    <span className="text-muted-foreground">
                      Median run time
                    </span>
                    <span className="font-medium text-foreground">
                      {formatDuration(Number(value))}
                    </span>
                  </span>
                )}
              />
            }
          />
          <Line
            isAnimationActive={false}
            dataKey="median_secs"
            type="linear"
            stroke="var(--color-median_secs)"
            strokeWidth={2}
            strokeLinecap="round"
            strokeLinejoin="round"
            connectNulls
            dot={
              dots
                ? {
                    r: 4,
                    fill: "var(--color-median_secs)",
                    stroke: "var(--card)",
                    strokeWidth: 2,
                  }
                : false
            }
            activeDot={{
              r: 5,
              fill: "var(--color-median_secs)",
              stroke: "var(--card)",
              strokeWidth: 2,
            }}
          />
        </LineChart>
      </ChartContainer>
    </ChartCard>
  );
}

function CostPerDay({ m }: { m: Metrics }) {
  return (
    <ChartCard
      title="Notional cost per day"
      subtitle="Tokens priced at API rates — a comparison, not a bill"
      table={
        <DataTable
          head={["Day", "Notional cost"]}
          rows={[...m.daily]
            .reverse()
            .map((d) => [dayLabel(d.day), formatUsd(d.cost_usd)])}
        />
      }
    >
      <ChartContainer config={costConfig} className="aspect-auto h-56 w-full">
        <BarChart data={m.daily} margin={{ left: -8, right: 4, top: 8 }}>
          <CartesianGrid vertical={false} stroke="var(--border)" />
          <XAxis
            dataKey="day"
            tickFormatter={dayLabel}
            tickLine={false}
            axisLine={false}
            minTickGap={24}
            tick={axisTick}
          />
          <YAxis
            tickFormatter={(v: number) => `$${Math.round(v)}`}
            tickLine={false}
            axisLine={false}
            tick={axisTick}
            width={48}
          />
          <ChartTooltip
            cursor={false}
            content={
              <ChartTooltipContent
                indicator="line"
                labelFormatter={(v) => dayLabel(String(v))}
                formatter={(value) => (
                  <span className="flex w-full justify-between gap-4">
                    <span className="text-muted-foreground">Notional cost</span>
                    <span className="font-medium text-foreground">
                      {formatUsd(Number(value))}
                    </span>
                  </span>
                )}
              />
            }
          />
          <Bar
            isAnimationActive={false}
            dataKey="cost_usd"
            fill="var(--color-cost_usd)"
            radius={[4, 4, 0, 0]}
            maxBarSize={24}
          />
        </BarChart>
      </ChartContainer>
    </ChartCard>
  );
}

function ByWorkflow({ m }: { m: Metrics }) {
  const named = m.workflows.slice(0, WORKFLOW_ROWS);
  const rest = m.workflows.slice(WORKFLOW_ROWS);
  const rows = [
    ...named,
    ...(rest.length
      ? [
          rest.reduce(
            (acc, w) => ({
              ...acc,
              completed: acc.completed + w.completed,
              failed: acc.failed + w.failed,
              cancelled: acc.cancelled + w.cancelled,
              cost_usd: acc.cost_usd + w.cost_usd,
            }),
            {
              workflow: `Other (${rest.length})`,
              completed: 0,
              failed: 0,
              cancelled: 0,
              median_secs: null as number | null,
              cost_usd: 0,
            },
          ),
        ]
      : []),
  ];
  const data = rows.map((w) => ({
    workflow: w.workflow,
    completed: w.completed,
    failed: w.failed + w.cancelled,
  }));
  return (
    <ChartCard
      title="By workflow"
      subtitle="Finished runs per workflow, by how they ended"
      table={
        <DataTable
          head={[
            "Workflow",
            "Completed",
            "Failed",
            "Cancelled",
            "Median",
            "Cost",
          ]}
          rows={rows.map((w) => [
            w.workflow,
            w.completed,
            w.failed,
            w.cancelled,
            formatDuration(w.median_secs),
            formatUsd(w.cost_usd),
          ])}
        />
      }
    >
      <ChartContainer
        config={outcomeConfig}
        className="aspect-auto w-full"
        style={{ height: Math.max(160, data.length * 32 + 48) }}
      >
        <BarChart data={data} layout="vertical" margin={{ left: 0, right: 8 }}>
          <CartesianGrid horizontal={false} stroke="var(--border)" />
          <XAxis
            type="number"
            allowDecimals={false}
            tickLine={false}
            axisLine={false}
            tick={axisTick}
          />
          <YAxis
            type="category"
            dataKey="workflow"
            tickLine={false}
            axisLine={false}
            tick={axisTick}
            width={130}
          />
          <ChartTooltip
            cursor={false}
            content={<ChartTooltipContent indicator="line" />}
          />
          <ChartLegend content={<ChartLegendContent />} />
          <Bar
            isAnimationActive={false}
            dataKey="completed"
            stackId="runs"
            fill="var(--color-completed)"
            maxBarSize={20}
            shape={HORIZONTAL_COMPLETED}
          />
          <Bar
            isAnimationActive={false}
            dataKey="failed"
            stackId="runs"
            fill="var(--color-failed)"
            maxBarSize={20}
            shape={HORIZONTAL_FAILED}
          />
        </BarChart>
      </ChartContainer>
    </ChartCard>
  );
}
