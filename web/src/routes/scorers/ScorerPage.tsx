/**
 * One scorer across runs: what it judges, what it has cost, its pass rate
 * or daily cost over time, and the runs it graded. Reached from /scorers.
 */
import { useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { CartesianGrid, Line, LineChart, XAxis, YAxis } from "recharts";
import { AppShell } from "@/components/AppShell";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  type ChartConfig,
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
} from "@/components/ui/chart";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { formatDuration } from "@/components/runflow/format";
import { formatCost } from "@/lib/cost";
import { dayLabel } from "@/lib/dashboard-metrics";
import { passRate, trendPoints, useScorer } from "@/lib/scorers";

const DAYS = 30;

/** The series blue the dashboard's single-series charts use. */
const SERIES = { light: "#2a78d6", dark: "#3987e5" };

const chartConfig = {
  passRate: { label: "Pass rate", theme: SERIES },
  cost: { label: "Daily cost", theme: SERIES },
} satisfies ChartConfig;

type Metric = "passRate" | "cost";

export function ScorerPage() {
  const { name = "" } = useParams();
  const detail = useScorer(name, DAYS);
  const [metric, setMetric] = useState<Metric>("passRate");
  const today = new Date().toISOString().slice(0, 10);
  const points = useMemo(
    () => trendPoints(detail.data?.daily ?? [], DAYS, today),
    [detail.data, today],
  );

  const s = detail.data?.summary;
  return (
    <AppShell title={name}>
      <div className="mx-auto flex max-w-5xl flex-col gap-6 p-6">
        <div className="flex flex-col gap-1">
          <Link
            to="/scorers"
            className="text-xs text-muted-foreground hover:underline"
          >
            Scorers
          </Link>
          <h1 className="text-xl font-semibold">{name}</h1>
          {s?.description && (
            <p className="max-w-3xl text-sm text-muted-foreground">
              {s.description}
            </p>
          )}
          {s && (
            <div className="mt-1 flex flex-wrap gap-1.5">
              {s.workflows.map((w) => (
                <Badge key={w} variant="outline">
                  {w}
                </Badge>
              ))}
            </div>
          )}
        </div>

        {detail.isLoading && (
          <p className="text-sm text-muted-foreground">Loading…</p>
        )}
        {detail.isError && (
          <p className="text-sm text-destructive">{detail.error.message}</p>
        )}

        {s && detail.data && (
          <>
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
              <Stat
                label="All-time cost"
                value={formatCost(s.total_cost_usd)}
              />
              <Stat label="Runs graded" value={String(s.runs)} />
              <Stat label="Pass rate" value={passRate(s.passed, s.runs)} />
            </div>

            <Card>
              <CardHeader className="flex flex-row items-center justify-between gap-2">
                <CardTitle className="text-sm">Last {DAYS} days</CardTitle>
                <Tabs
                  value={metric}
                  onValueChange={(v) => setMetric(v as Metric)}
                >
                  <TabsList>
                    <TabsTrigger value="passRate">Pass rate</TabsTrigger>
                    <TabsTrigger value="cost">Daily cost</TabsTrigger>
                  </TabsList>
                </Tabs>
              </CardHeader>
              <CardContent>
                <ChartContainer
                  config={chartConfig}
                  className="aspect-auto h-56 w-full"
                >
                  <LineChart
                    data={points}
                    margin={{ left: -8, right: 8, top: 8 }}
                  >
                    <CartesianGrid vertical={false} stroke="var(--border)" />
                    <XAxis
                      dataKey="day"
                      tickFormatter={dayLabel}
                      tickLine={false}
                      axisLine={false}
                      minTickGap={24}
                      tick={{ fontSize: 11 }}
                    />
                    <YAxis
                      tickLine={false}
                      axisLine={false}
                      tick={{ fontSize: 11 }}
                      width={48}
                      domain={metric === "passRate" ? [0, 100] : [0, "auto"]}
                      tickFormatter={(v: number) =>
                        metric === "passRate" ? `${v}%` : formatCost(v)
                      }
                    />
                    <ChartTooltip
                      content={
                        <ChartTooltipContent
                          indicator="line"
                          labelFormatter={(v) => dayLabel(String(v))}
                          formatter={(value) =>
                            metric === "passRate"
                              ? `${value}%`
                              : formatCost(Number(value))
                          }
                        />
                      }
                    />
                    {/* A day without a grade is a gap, not a zero. */}
                    <Line
                      dataKey={metric}
                      type="linear"
                      stroke={`var(--color-${metric})`}
                      strokeWidth={2}
                      dot={{ r: 2.5 }}
                      connectNulls={false}
                      isAnimationActive={false}
                    />
                  </LineChart>
                </ChartContainer>
              </CardContent>
            </Card>

            <div className="rounded-md border border-border">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Run</TableHead>
                    <TableHead>Result</TableHead>
                    <TableHead className="text-right">Score</TableHead>
                    <TableHead>Classification</TableHead>
                    <TableHead className="text-right">Run time</TableHead>
                    <TableHead className="text-right">Cost</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {detail.data.runs.map((r) => (
                    <TableRow key={`${r.run_id}/${r.node_id}`}>
                      <TableCell className="max-w-80">
                        <Link
                          to={`/runs/${r.run_id}`}
                          className="font-medium hover:underline"
                        >
                          {new Date(r.created_at).toLocaleString(undefined, {
                            month: "numeric",
                            day: "numeric",
                            hour: "numeric",
                            minute: "2-digit",
                          })}
                        </Link>
                        <div
                          className="truncate text-xs text-muted-foreground"
                          title={r.run_title ?? r.workflow}
                        >
                          {r.run_title ?? r.workflow}
                        </div>
                      </TableCell>
                      <TableCell>
                        <Badge variant={r.passed ? "success" : "failed"}>
                          {r.passed ? "Pass" : "Fail"}
                        </Badge>
                      </TableCell>
                      <TableCell className="text-right tabular-nums">
                        {r.score.toFixed(2)}
                      </TableCell>
                      <TableCell
                        className="max-w-72 truncate"
                        title={r.summary ?? undefined}
                      >
                        {r.classification ?? "—"}
                      </TableCell>
                      <TableCell className="text-right tabular-nums">
                        {formatDuration(r.duration_ms)}
                      </TableCell>
                      <TableCell className="text-right tabular-nums">
                        {formatCost(r.cost_usd)}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          </>
        )}
      </div>
    </AppShell>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <Card size="sm">
      <CardContent className="flex flex-col gap-0.5">
        <span className="text-xs text-muted-foreground">{label}</span>
        <span className="text-2xl font-semibold tabular-nums">{value}</span>
      </CardContent>
    </Card>
  );
}
