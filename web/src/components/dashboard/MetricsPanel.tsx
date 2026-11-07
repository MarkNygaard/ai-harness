import { useState } from "react";
import {
  IconArrowDownRight,
  IconArrowUpRight,
  IconHelpCircle,
} from "@tabler/icons-react";
import {
  Area,
  AreaChart,
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
import { Checkbox } from "@/components/ui/checkbox";
import {
  type ChartConfig,
  ChartContainer,
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
import { useTheme } from "@/lib/theme";

/**
 * Data colours, validated with the dataviz palette checker in both modes: the
 * series blue for runs that finished and for every single-series chart, the
 * status red for runs that didn't, and categorical slot 2 (orange) beside the
 * blue for merged against opened PRs. Green-for-done was the obvious pairing
 * and fails red-green colour blindness (ΔE 4 against the red).
 */
const SERIES = { light: "#2a78d6", dark: "#3987e5" };
const SERIES_2 = { light: "#eb6834", dark: "#d95926" };
const CRITICAL = { light: "#d03b3b", dark: "#d03b3b" };

type Tone = typeof SERIES;

const outcomeConfig = {
  completed: { label: "Completed", theme: SERIES },
  failed: { label: "Failed", theme: CRITICAL },
} satisfies ChartConfig;

const prConfig = {
  prs_opened: { label: "Opened", theme: SERIES },
  prs_merged: { label: "Merged", theme: SERIES_2 },
} satisfies ChartConfig;

const runTimeConfig = {
  median_secs: { label: "Median run time", theme: SERIES },
} satisfies ChartConfig;

const costConfig = {
  cost_usd: { label: "Notional cost", theme: SERIES },
} satisfies ChartConfig;

/** Workflows shown by name; the rest fold into one "Other" bar. */
const WORKFLOW_ROWS = 8;

const CHART_HEIGHT = "aspect-auto h-48 w-full";
const axisTick = { fontSize: 11 };

/**
 * How the harness has been doing over `days`, for `project` or all of them.
 * Each card leads with its headline number and the change from the period
 * before; the chart under it shows the days.
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

  const nothing =
    m.totals.runs === 0 && m.totals.prs_opened + m.totals.prs_merged === 0;

  // While a new filter loads, the old picture stays, dimmed — no flash.
  return (
    <div
      className={`transition-opacity ${
        metrics.isPlaceholderData ? "opacity-60" : ""
      }`}
    >
      {nothing ? (
        <Card size="sm">
          <CardContent className="py-8 text-center text-sm text-muted-foreground">
            No finished runs in the last {m.days} days
            {project ? ` in ${project}` : ""}.
          </CardContent>
        </Card>
      ) : (
        <div className="grid gap-3 lg:grid-cols-2">
          <Runs m={m} />
          <CycleTime m={m} />
          <PullRequests m={m} />
          <RunTime m={m} />
          <Cost m={m} />
          <ByWorkflow m={m} />
        </div>
      )}
    </div>
  );
}

// ── Pieces ─────────────────────────────────────────────────────────────────

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

/** What a number means, on hover — the label stays short. */
function Help({ text }: { text: string }) {
  return (
    <span title={text} aria-label={text} className="inline-flex">
      <IconHelpCircle className="size-3.5 text-muted-foreground" aria-hidden />
    </span>
  );
}

interface SeriesInfo {
  key: string;
  label: string;
  tone: Tone;
}

/**
 * The legend, as checkboxes: each names a series in its colour and shows or
 * hides it. The last one showing can't be hidden — an empty chart answers
 * nothing.
 */
function SeriesToggles({
  series,
  hidden,
  onChange,
}: {
  series: SeriesInfo[];
  hidden: Set<string>;
  onChange: (next: Set<string>) => void;
}) {
  const { resolved } = useTheme();
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
      {series.map((s) => {
        const shown = !hidden.has(s.key);
        const last = shown && series.length - hidden.size === 1;
        return (
          <label
            key={s.key}
            className="flex cursor-pointer items-center gap-1.5 text-[12px] text-muted-foreground"
          >
            <Checkbox
              checked={shown}
              disabled={last}
              onCheckedChange={(on) => {
                const next = new Set(hidden);
                if (on) next.delete(s.key);
                else next.add(s.key);
                onChange(next);
              }}
              style={{ "--series": s.tone[resolved] } as React.CSSProperties}
              className="data-checked:border-(--series) data-checked:bg-(--series) data-checked:text-white dark:data-checked:bg-(--series) disabled:opacity-100"
            />
            {s.label}
          </label>
        );
      })}
    </div>
  );
}

/**
 * A card with a headline: its title, the number that sums it up, a line of
 * context, then the chart — or the same numbers as a table, since a tooltip
 * should never be the only way to read a value.
 */
function ChartCard({
  title,
  help,
  value,
  caption,
  controls,
  table,
  children,
}: {
  title: string;
  help: string;
  value: string;
  caption: React.ReactNode;
  controls?: React.ReactNode;
  table: React.ReactNode;
  children: React.ReactNode;
}) {
  const [asTable, setAsTable] = useState(false);
  return (
    <Card size="sm">
      <CardContent className="flex flex-col gap-3">
        <div className="flex items-start gap-3">
          <div className="flex min-w-0 flex-1 flex-col gap-0.5">
            <h3 className="flex items-center gap-1.5 text-sm text-muted-foreground">
              {title}
              <Help text={help} />
            </h3>
            <span className="text-2xl font-semibold">{value}</span>
            <span className="flex flex-wrap items-center gap-x-2 text-[11px] text-muted-foreground">
              {caption}
            </span>
          </div>
          <div className="flex flex-col items-end gap-1.5">
            <Button
              variant="ghost"
              size="xs"
              aria-pressed={asTable}
              onClick={() => setAsTable((t) => !t)}
            >
              {asTable ? "Chart" : "Table"}
            </Button>
            {!asTable && controls}
          </div>
        </div>
        {asTable ? (
          <div className="max-h-48 overflow-y-auto">{table}</div>
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

/** Rows of the per-day table, newest first. */
function byDay(
  m: Metrics,
  cells: (d: Metrics["daily"][number]) => (string | number)[],
) {
  return [...m.daily].reverse().map((d) => [dayLabel(d.day), ...cells(d)]);
}

function vs(m: Metrics) {
  return `vs previous ${m.days} days`;
}

// ── Time axes ──────────────────────────────────────────────────────────────

/** The shared day axis, grid and hover crosshair of every time chart. */
function timeAxes(yFormat?: (v: number) => string, yWidth = 40) {
  return [
    <CartesianGrid key="grid" vertical={false} stroke="var(--border)" />,
    <XAxis
      key="x"
      dataKey="day"
      tickFormatter={dayLabel}
      tickLine={false}
      axisLine={false}
      minTickGap={24}
      tick={axisTick}
      padding={{ left: 8, right: 8 }}
    />,
    <YAxis
      key="y"
      allowDecimals={false}
      tickFormatter={yFormat}
      tickLine={false}
      axisLine={false}
      tick={axisTick}
      width={yWidth}
    />,
  ];
}

const crosshair = {
  stroke: "var(--muted-foreground)",
  strokeWidth: 1,
  strokeDasharray: "3 3",
};

function lineProps(key: string) {
  return {
    isAnimationActive: false,
    dataKey: key,
    type: "linear" as const,
    stroke: `var(--color-${key})`,
    strokeWidth: 2,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
    dot: false,
    activeDot: {
      r: 4,
      fill: `var(--color-${key})`,
      stroke: "var(--card)",
      strokeWidth: 2,
    },
  };
}

/** A tooltip row: the series name, and its value formatted. */
function valueRow(label: string, value: string) {
  return (
    <span className="flex w-full justify-between gap-4">
      <span className="text-muted-foreground">{label}</span>
      <span className="font-medium text-foreground tabular-nums">{value}</span>
    </span>
  );
}

// ── The cards ──────────────────────────────────────────────────────────────

function Runs({ m }: { m: Metrics }) {
  const [hidden, setHidden] = useState<Set<string>>(new Set());
  const data = m.daily.map((d) => ({
    day: d.day,
    completed: d.completed,
    failed: d.failed + d.cancelled,
  }));
  const rate = completionRate(m.totals);
  const rateDelta = delta(rate, completionRate(m.previous), {
    better: "up",
    kind: "points",
  });
  return (
    <ChartCard
      title="Runs"
      help="Finished runs, by how they ended (failed includes cancelled). A task is the runs that share a title — an issue built, reviewed and merged — with at least one completed."
      value={m.totals.runs.toLocaleString()}
      caption={
        <>
          <span>
            {formatPercent(rate)} completed · {m.totals.tasks.toLocaleString()}{" "}
            tasks finished
          </span>
          {rateDelta && <DeltaText d={rateDelta} />}
        </>
      }
      controls={
        <SeriesToggles
          series={[
            { key: "completed", label: "Completed", tone: SERIES },
            { key: "failed", label: "Failed", tone: CRITICAL },
          ]}
          hidden={hidden}
          onChange={setHidden}
        />
      }
      table={
        <DataTable
          head={["Day", "Completed", "Failed", "Cancelled"]}
          rows={byDay(m, (d) => [d.completed, d.failed, d.cancelled])}
        />
      }
    >
      <ChartContainer config={outcomeConfig} className={CHART_HEIGHT}>
        <AreaChart data={data} margin={{ left: -16, right: 4, top: 8 }}>
          {timeAxes()}
          <ChartTooltip
            cursor={crosshair}
            content={
              <ChartTooltipContent
                indicator="line"
                labelFormatter={(v) => dayLabel(String(v))}
              />
            }
          />
          {(["completed", "failed"] as const).map((key) => (
            <Area
              key={key}
              {...lineProps(key)}
              stackId="runs"
              fill={`var(--color-${key})`}
              fillOpacity={0.2}
              hide={hidden.has(key)}
            />
          ))}
        </AreaChart>
      </ChartContainer>
    </ChartCard>
  );
}

/** The cycle from kickoff to merge, as four medians in a 2×2 grid. */
function CycleTime({ m }: { m: Metrics }) {
  const t = m.totals;
  const p = m.previous;
  const cells: {
    label: string;
    help: string;
    now: number | null;
    before: number | null;
  }[] = [
    {
      label: "Kickoff → merge",
      help: "From the run starting to its pull request merging.",
      now: t.median_kickoff_to_merge_secs,
      before: p.median_kickoff_to_merge_secs,
    },
    {
      label: "Kickoff → PR",
      help: "From the run starting to its pull request opening.",
      now: t.median_to_pr_secs,
      before: p.median_to_pr_secs,
    },
    {
      label: "PR → first review",
      help: "From the pull request opening to someone other than its author first reviewing it.",
      now: t.median_to_review_secs,
      before: p.median_to_review_secs,
    },
    {
      label: "First review → merge",
      help: "From that first review to the merge.",
      now: t.median_review_to_merge_secs,
      before: p.median_review_to_merge_secs,
    },
  ];
  return (
    <Card size="sm">
      <CardContent className="flex h-full flex-col gap-3">
        <h3 className="flex items-center gap-1.5 text-sm text-muted-foreground">
          Median cycle time
          <Help text="Medians over the pull requests runs opened. A PR someone opened before the run started has no kickoff time." />
        </h3>
        <div className="grid flex-1 grid-cols-2 overflow-hidden rounded-lg border border-border">
          {cells.map((c, i) => {
            const d = delta(c.now, c.before, { better: "down" });
            return (
              <div
                key={c.label}
                className={`flex flex-col items-center justify-center gap-1 px-3 py-5 text-center ${
                  i % 2 === 0 ? "border-r border-border" : ""
                } ${i < 2 ? "border-b border-border" : ""}`}
              >
                <span className="flex items-center gap-1 text-[12px] font-medium text-muted-foreground">
                  {c.label}
                  <Help text={c.help} />
                </span>
                <span className="text-2xl font-semibold">
                  {formatDuration(c.now)}
                </span>
                <span className="flex h-4 items-center text-[11px]">
                  {d && <DeltaText d={d} />}
                </span>
              </div>
            );
          })}
        </div>
      </CardContent>
    </Card>
  );
}

function PullRequests({ m }: { m: Metrics }) {
  const [hidden, setHidden] = useState<Set<string>>(new Set());
  const d = delta(m.totals.prs_opened, m.previous.prs_opened, { better: "up" });
  return (
    <ChartCard
      title="Pull requests"
      help="Pull requests runs opened, and those that merged, by day. A run whose output links a PR is followed until it merges or closes."
      value={m.totals.prs_opened.toLocaleString()}
      caption={
        <>
          <span>{m.totals.prs_merged.toLocaleString()} merged</span>
          {d && <DeltaText d={d} />}
        </>
      }
      controls={
        <SeriesToggles
          series={[
            { key: "prs_opened", label: "Opened", tone: SERIES },
            { key: "prs_merged", label: "Merged", tone: SERIES_2 },
          ]}
          hidden={hidden}
          onChange={setHidden}
        />
      }
      table={
        <DataTable
          head={["Day", "Opened", "Merged"]}
          rows={byDay(m, (d) => [d.prs_opened, d.prs_merged])}
        />
      }
    >
      <ChartContainer config={prConfig} className={CHART_HEIGHT}>
        <LineChart data={m.daily} margin={{ left: -16, right: 4, top: 8 }}>
          {timeAxes()}
          <ChartTooltip
            cursor={crosshair}
            content={
              <ChartTooltipContent
                indicator="line"
                labelFormatter={(v) => dayLabel(String(v))}
              />
            }
          />
          {(["prs_opened", "prs_merged"] as const).map((key) => (
            <Line key={key} {...lineProps(key)} hide={hidden.has(key)} />
          ))}
        </LineChart>
      </ChartContainer>
    </ChartCard>
  );
}

function RunTime({ m }: { m: Metrics }) {
  const d = delta(m.totals.median_secs, m.previous.median_secs, {
    better: "down",
  });
  return (
    <ChartCard
      title="Median run time"
      help="First step started to last step ended, for runs that completed or failed. A cancelled run's length says when someone stopped it."
      value={formatDuration(m.totals.median_secs)}
      caption={
        <>
          {d && <DeltaText d={d} />}
          <span>{vs(m)}</span>
        </>
      }
      table={
        <DataTable
          head={["Day", "Median run time"]}
          rows={byDay(m, (d) => [formatDuration(d.median_secs)])}
        />
      }
    >
      <ChartContainer config={runTimeConfig} className={CHART_HEIGHT}>
        <LineChart data={m.daily} margin={{ left: -8, right: 4, top: 8 }}>
          {timeAxes((v) => formatDuration(v), 44)}
          <ChartTooltip
            cursor={crosshair}
            content={
              <ChartTooltipContent
                indicator="line"
                labelFormatter={(v) => dayLabel(String(v))}
                formatter={(value) =>
                  valueRow("Median run time", formatDuration(Number(value)))
                }
              />
            }
          />
          <Line {...lineProps("median_secs")} connectNulls />
        </LineChart>
      </ChartContainer>
    </ChartCard>
  );
}

function Cost({ m }: { m: Metrics }) {
  const d = delta(m.totals.cost_usd, m.previous.cost_usd, { better: "none" });
  return (
    <ChartCard
      title="Notional cost"
      help="The tokens these runs used, priced at API rates. A comparison, not a bill: subscriptions pay for most of it."
      value={formatUsd(m.totals.cost_usd)}
      caption={
        <>
          {d && <DeltaText d={d} />}
          <span>{vs(m)}</span>
        </>
      }
      table={
        <DataTable
          head={["Day", "Notional cost"]}
          rows={byDay(m, (d) => [formatUsd(d.cost_usd)])}
        />
      }
    >
      <ChartContainer config={costConfig} className={CHART_HEIGHT}>
        <LineChart data={m.daily} margin={{ left: -8, right: 4, top: 8 }}>
          {timeAxes((v) => `$${Math.round(v)}`, 44)}
          <ChartTooltip
            cursor={crosshair}
            content={
              <ChartTooltipContent
                indicator="line"
                labelFormatter={(v) => dayLabel(String(v))}
                formatter={(value) =>
                  valueRow("Notional cost", formatUsd(Number(value)))
                }
              />
            }
          />
          <Line {...lineProps("cost_usd")} />
        </LineChart>
      </ChartContainer>
    </ChartCard>
  );
}

// ── By workflow: a comparison across names, so bars ────────────────────────

interface SegmentProps {
  x?: number;
  y?: number;
  width?: number;
  height?: number;
  fill?: string;
  payload?: { failed: number };
}

/**
 * One segment of a stacked horizontal bar, drawn by hand so it follows the
 * mark spec: only the segment that *is* the end — the failed one, or the
 * completed one on a row with no failures — gets the rounded end, and a 2px
 * gap in the surface colour separates the two rather than a stroke.
 */
function segment(key: "completed" | "failed") {
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
    const w = Math.max(0, width - gap);
    if (w <= 0 || height <= 0) return <g />;
    const r = isEnd ? Math.min(4, w, height / 2) : 0;
    const d = `M${x},${y} H${x + w - r} Q${x + w},${y} ${x + w},${y + r} V${
      y + height - r
    } Q${x + w},${y + height} ${x + w - r},${y + height} H${x} Z`;
    return <path d={d} fill={fill} />;
  };
}

const COMPLETED_SEGMENT = segment("completed");
const FAILED_SEGMENT = segment("failed");

function ByWorkflow({ m }: { m: Metrics }) {
  const [hidden, setHidden] = useState<Set<string>>(new Set());
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
    completed: hidden.has("completed") ? 0 : w.completed,
    failed: hidden.has("failed") ? 0 : w.failed + w.cancelled,
  }));
  const busiest = m.workflows[0];
  return (
    <ChartCard
      title="By workflow"
      help="Finished runs per workflow, by how they ended."
      value={m.workflows.length.toLocaleString()}
      caption={
        <span>
          workflows ran
          {busiest ? ` · busiest ${busiest.workflow}` : ""}
        </span>
      }
      controls={
        <SeriesToggles
          series={[
            { key: "completed", label: "Completed", tone: SERIES },
            { key: "failed", label: "Failed", tone: CRITICAL },
          ]}
          hidden={hidden}
          onChange={setHidden}
        />
      }
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
        style={{ height: Math.max(144, data.length * 28 + 32) }}
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
          <Bar
            isAnimationActive={false}
            dataKey="completed"
            stackId="runs"
            fill="var(--color-completed)"
            maxBarSize={18}
            shape={COMPLETED_SEGMENT}
          />
          <Bar
            isAnimationActive={false}
            dataKey="failed"
            stackId="runs"
            fill="var(--color-failed)"
            maxBarSize={18}
            shape={FAILED_SEGMENT}
          />
        </BarChart>
      </ChartContainer>
    </ChartCard>
  );
}
