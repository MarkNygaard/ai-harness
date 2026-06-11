import { useMemo } from "react";
import type { NodeView } from "@/types/run";
import { Badge } from "@/components/ui/badge";
import {
  elapsedMs,
  formatDuration,
  formatTokens,
  statusLabel,
  sumUsage,
  totalTokens,
} from "./format";
import {
  nodeColor,
  timeByCategory,
  timeByStep,
  tokensByStep,
  TOKEN_INPUT_COLOR,
  TOKEN_OUTPUT_COLOR,
  usageByModel,
  usageByType,
  waterfall,
  type WaterfallRow,
} from "./overview";
import { categoryColorMap, useCategories } from "@/lib/categories";
import { formatCost, usageCost } from "@/lib/cost";

export { usageByModel };

const statusVariant: Record<
  string,
  "running" | "success" | "failed" | "skipped" | "default"
> = {
  running: "running",
  success: "success",
  failed: "failed",
  cancelled: "failed",
  skipped: "skipped",
  pending: "default",
};

/**
 * Factory-style task overview: full-width, square-edged dashboard — headline
 * metrics, a milestone waterfall, time + token breakdowns, and detail tables.
 *
 * `stacked` renders the time and token breakdowns in one column (time first,
 * tokens under it) instead of side-by-side — used by the A/B comparison, where
 * each arm's overview already sits in a half-width column.
 */
export function TaskOverview({
  nodes,
  stacked = false,
}: {
  nodes: NodeView[];
  stacked?: boolean;
}) {
  const now = Date.now();
  const byModel = useMemo(() => usageByModel(nodes), [nodes]);
  const totals = useMemo(() => sumUsage(nodes.map((n) => n.usage)), [nodes]);
  const segments = useMemo(() => usageByType(totals), [totals]);
  // Notional cost is summed PER NODE (each priced at its own model's rate) — not
  // priced off the aggregate, which would mis-price a multi-model run.
  const totalCost = useMemo(
    () =>
      nodes.reduce(
        (acc, n) => acc + usageCost(n.model ?? n.provider, n.usage),
        0,
      ),
    [nodes],
  );
  const tokenSteps = useMemo(() => tokensByStep(nodes), [nodes]);
  const heaviest = tokenSteps[0]?.total ?? 0;
  const bars = useMemo(() => waterfall(nodes, now), [nodes, now]);
  const steps = useMemo(() => timeByStep(nodes, now), [nodes, now]);

  // Category colours + time-by-category (uncategorized steps keep status colour).
  const cats = useCategories();
  const colors = useMemo(() => categoryColorMap(cats.data), [cats.data]);
  const catBar = useMemo(
    () => timeByCategory(nodes, now, cats.data ?? []),
    [nodes, now, cats.data],
  );
  const catTotal = catBar.reduce((s, c) => s + c.ms, 0);

  const done = nodes.filter((n) =>
    ["success", "failed", "skipped", "cancelled"].includes(n.status),
  ).length;
  const failed = nodes.filter(
    (n) => n.status === "failed" || n.status === "cancelled",
  ).length;
  const wallMs = useMemo(() => runDuration(nodes, now), [nodes, now]);
  const totalTok = totalTokens(totals);
  const segTotal = segments.reduce((s, x) => s + x.value, 0);
  const longest = steps[0]?.durationMs ?? 0;

  return (
    <div className="flex w-full flex-col gap-6">
      {/* Headline metrics */}
      <div className="grid grid-cols-2 gap-px border border-border bg-border sm:grid-cols-5">
        <Metric label="Total time" value={formatDuration(wallMs)} />
        <Metric label="Total tokens" value={formatTokens(totalTok)} />
        <Metric
          label="Notional cost"
          value={formatCost(totalCost)}
          hint="at API list prices"
        />
        <Metric
          label="Steps"
          value={`${done}/${nodes.length}`}
          hint={failed ? `${failed} failed` : undefined}
        />
        <Metric label="Models" value={String(byModel.length || "—")} />
      </div>

      {/* Body: two columns (time | tokens) by default; one stacked column when
          `stacked` (A/B view) so tokens sit under the time breakdowns. */}
      <div className={`grid gap-6 ${stacked ? "" : "lg:grid-cols-2"}`}>
        <div className="flex flex-col gap-6">
          {catTotal > 0 && (
            <Section title="Time by category">
              <div className="border border-border p-3">
                <div className="flex h-4 w-full overflow-hidden bg-secondary/50">
                  {catBar.map((c) => (
                    <div
                      key={c.id}
                      style={{
                        width: `${(c.ms / catTotal) * 100}%`,
                        backgroundColor: c.color,
                      }}
                      title={`${c.label}: ${formatDuration(c.ms)}`}
                    />
                  ))}
                </div>
                <div className="mt-3 flex flex-wrap gap-x-5 gap-y-1.5 text-[12px]">
                  {catBar.map((c) => (
                    <div key={c.id} className="flex items-center gap-1.5">
                      <span
                        className="size-2.5 rounded-full"
                        style={{ backgroundColor: c.color }}
                      />
                      <span className="text-muted-foreground">{c.label}</span>
                      <span className="font-medium tabular-nums">
                        {formatDuration(c.ms)}
                      </span>
                    </div>
                  ))}
                </div>
              </div>
            </Section>
          )}

          <Section title="Milestone waterfall">
            {bars.length === 0 ? (
              <Empty>No timing recorded yet.</Empty>
            ) : (
              <div className="flex flex-col gap-1 border border-border p-3">
                {bars.map((b) => (
                  <WaterfallBar
                    key={b.id}
                    row={b}
                    color={nodeColor(b.status, b.category, colors)}
                  />
                ))}
              </div>
            )}
          </Section>

          <Section title="Time by step">
            {steps.length === 0 ? (
              <Empty>No timing recorded yet.</Empty>
            ) : (
              <div className="flex flex-col gap-2 border border-border p-3">
                {steps.map((s) => (
                  <div
                    key={s.id}
                    className="flex items-center gap-3 text-[12px]"
                  >
                    <div
                      className="w-40 shrink-0 truncate font-medium"
                      title={s.id}
                    >
                      {s.id}
                    </div>
                    <div className="h-3 flex-1 overflow-hidden bg-secondary/50">
                      <div
                        className="h-full"
                        style={{
                          width: `${longest > 0 ? (s.durationMs / longest) * 100 : 0}%`,
                          backgroundColor: nodeColor(
                            s.status,
                            s.category,
                            colors,
                          ),
                        }}
                      />
                    </div>
                    <div className="w-16 shrink-0 text-right tabular-nums text-muted-foreground">
                      {formatDuration(s.durationMs)}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </Section>
        </div>

        <div className="flex flex-col gap-6">
          <Section title="Tokens by type">
            {segTotal === 0 ? (
              <Empty>No token usage reported yet.</Empty>
            ) : (
              <div className="border border-border p-3">
                <div className="flex h-4 w-full overflow-hidden bg-secondary/50">
                  {segments.map((s) => (
                    <div
                      key={s.key}
                      style={{
                        width: `${(s.value / segTotal) * 100}%`,
                        backgroundColor: s.color,
                      }}
                      title={`${s.label}: ${formatTokens(s.value)}`}
                    />
                  ))}
                </div>
                <div className="mt-3 flex flex-wrap gap-x-5 gap-y-1.5 text-[12px]">
                  {segments.map((s) => (
                    <div key={s.key} className="flex items-center gap-1.5">
                      <span
                        className="size-2.5 rounded-full"
                        style={{ backgroundColor: s.color }}
                      />
                      <span className="text-muted-foreground">{s.label}</span>
                      <span className="font-medium tabular-nums">
                        {formatTokens(s.value)}
                      </span>
                    </div>
                  ))}
                </div>
              </div>
            )}
          </Section>

          <Section title="Tokens by step">
            {tokenSteps.length === 0 ? (
              <Empty>No token usage reported yet.</Empty>
            ) : (
              <div className="border border-border p-3">
                <div className="flex flex-col gap-2">
                  {tokenSteps.map((s) => (
                    <div
                      key={s.id}
                      className="flex items-center gap-3 text-[12px]"
                    >
                      <div
                        className="w-40 shrink-0 truncate font-medium"
                        title={s.id}
                      >
                        {s.id}
                      </div>
                      <div className="h-3 flex-1 overflow-hidden bg-secondary/50">
                        <div
                          className="flex h-full"
                          style={{
                            width: `${heaviest > 0 ? (s.total / heaviest) * 100 : 0}%`,
                          }}
                        >
                          <div
                            style={{
                              width: `${s.total > 0 ? (s.input / s.total) * 100 : 0}%`,
                              backgroundColor: TOKEN_INPUT_COLOR,
                            }}
                            title={`Input: ${formatTokens(s.input)}`}
                          />
                          <div
                            style={{
                              width: `${s.total > 0 ? (s.output / s.total) * 100 : 0}%`,
                              backgroundColor: TOKEN_OUTPUT_COLOR,
                            }}
                            title={`Output: ${formatTokens(s.output)}`}
                          />
                        </div>
                      </div>
                      <div className="w-16 shrink-0 text-right tabular-nums text-muted-foreground">
                        {formatTokens(s.total)}
                      </div>
                    </div>
                  ))}
                </div>
                <div className="mt-3 flex flex-wrap gap-x-5 gap-y-1.5 text-[12px]">
                  <div className="flex items-center gap-1.5">
                    <span
                      className="size-2.5 rounded-full"
                      style={{ backgroundColor: TOKEN_INPUT_COLOR }}
                    />
                    <span className="text-muted-foreground">Input</span>
                  </div>
                  <div className="flex items-center gap-1.5">
                    <span
                      className="size-2.5 rounded-full"
                      style={{ backgroundColor: TOKEN_OUTPUT_COLOR }}
                    />
                    <span className="text-muted-foreground">Output</span>
                  </div>
                </div>
              </div>
            )}
          </Section>

          <Section title="By model">
            {byModel.length === 0 ? (
              <Empty>No token usage reported yet.</Empty>
            ) : (
              <div className="overflow-hidden border border-border">
                <table className="w-full text-[12px]">
                  <thead className="bg-secondary/50 text-muted-foreground">
                    <tr>
                      <Th className="text-left">Model</Th>
                      <Th className="text-right">Steps</Th>
                      <Th className="text-right">In</Th>
                      <Th className="text-right">Out</Th>
                      <Th className="text-right">Cache</Th>
                      <Th className="text-right">Total</Th>
                      <Th className="text-right">Cost</Th>
                    </tr>
                  </thead>
                  <tbody>
                    {byModel.map((g) => (
                      <tr key={g.key} className="border-t border-border">
                        <Td className="font-medium text-foreground">{g.key}</Td>
                        <Td className="text-right tabular-nums text-muted-foreground">
                          {g.steps}
                        </Td>
                        <Td className="text-right tabular-nums">
                          {formatTokens(g.usage.input)}
                        </Td>
                        <Td className="text-right tabular-nums">
                          {formatTokens(g.usage.output)}
                        </Td>
                        <Td className="text-right tabular-nums text-muted-foreground">
                          {formatTokens(g.usage.cache_read)}
                        </Td>
                        <Td className="text-right font-medium tabular-nums">
                          {formatTokens(g.total)}
                        </Td>
                        <Td className="text-right font-medium tabular-nums">
                          {formatCost(usageCost(g.key, g.usage))}
                        </Td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </Section>
        </div>
      </div>

      {/* By step (full-width detail table) */}
      <Section title="By step">
        <div className="overflow-hidden border border-border">
          <table className="w-full text-[12px]">
            <thead className="bg-secondary/50 text-muted-foreground">
              <tr>
                <Th className="text-left">Step</Th>
                <Th className="text-left">Model</Th>
                <Th>Status</Th>
                <Th className="text-right">Time</Th>
                <Th className="text-right">In</Th>
                <Th className="text-right">Out</Th>
                <Th className="text-right">Total</Th>
                <Th className="text-right">Cost</Th>
              </tr>
            </thead>
            <tbody>
              {nodes.map((n) => (
                <tr key={n.id} className="border-t border-border">
                  <Td className="font-medium text-foreground">{n.id}</Td>
                  <Td className="text-muted-foreground">
                    {n.model ?? n.provider ?? "—"}
                  </Td>
                  <Td className="text-center">
                    <Badge variant={statusVariant[n.status] ?? "default"}>
                      {statusLabel(n.status)}
                    </Badge>
                  </Td>
                  <Td className="text-right tabular-nums text-muted-foreground">
                    {formatDuration(elapsedMs(n.started_at, n.ended_at, now))}
                  </Td>
                  <Td className="text-right tabular-nums">
                    {formatTokens(n.usage.input)}
                  </Td>
                  <Td className="text-right tabular-nums">
                    {formatTokens(n.usage.output)}
                  </Td>
                  <Td className="text-right font-medium tabular-nums">
                    {formatTokens(totalTokens(n.usage))}
                  </Td>
                  <Td className="text-right font-medium tabular-nums">
                    {formatCost(usageCost(n.model ?? n.provider, n.usage))}
                  </Td>
                </tr>
              ))}
            </tbody>
            <tfoot className="border-t-2 border-border bg-secondary/30">
              <tr>
                <Td className="font-semibold text-foreground" colSpan={4}>
                  Total
                </Td>
                <Td className="text-right font-semibold tabular-nums">
                  {formatTokens(totals.input)}
                </Td>
                <Td className="text-right font-semibold tabular-nums">
                  {formatTokens(totals.output)}
                </Td>
                <Td className="text-right font-semibold tabular-nums">
                  {formatTokens(totalTokens(totals))}
                </Td>
                <Td className="text-right font-semibold tabular-nums">
                  {formatCost(totalCost)}
                </Td>
              </tr>
            </tfoot>
          </table>
        </div>
      </Section>
    </div>
  );
}

/** Wall-clock span of the run = latest end − earliest start across timed nodes. */
function runDuration(nodes: NodeView[], now: number): number | null {
  let start = Infinity;
  let end = -Infinity;
  for (const n of nodes) {
    if (!n.started_at) continue;
    const s = Date.parse(n.started_at);
    if (Number.isNaN(s)) continue;
    const e = n.ended_at ? Date.parse(n.ended_at) : now;
    start = Math.min(start, s);
    end = Math.max(end, Number.isNaN(e) ? s : e);
  }
  return Number.isFinite(start) && Number.isFinite(end)
    ? Math.max(0, end - start)
    : null;
}

function WaterfallBar({ row, color }: { row: WaterfallRow; color: string }) {
  return (
    <div className="flex items-center gap-3 text-[12px]">
      <div className="w-40 shrink-0 truncate font-medium" title={row.id}>
        {row.id}
      </div>
      <div className="relative h-4 flex-1">
        <div
          className="absolute top-0 h-full min-w-0.75"
          style={{
            left: `${row.offset * 100}%`,
            width: `${row.width * 100}%`,
            backgroundColor: color,
          }}
          title={`${statusLabel(row.status)} · ${formatDuration(row.durationMs)}`}
        />
      </div>
      <div className="w-16 shrink-0 text-right tabular-nums text-muted-foreground">
        {formatDuration(row.durationMs)}
      </div>
    </div>
  );
}

function Metric({
  label,
  value,
  hint,
}: {
  label: string;
  value: string;
  hint?: string;
}) {
  return (
    <div className="bg-card p-3">
      <div className="text-[11px] uppercase tracking-wide text-muted-foreground">
        {label}
      </div>
      <div className="mt-1 text-xl font-semibold tabular-nums">{value}</div>
      {hint && <div className="text-[11px] text-status-failed">{hint}</div>}
    </div>
  );
}

/** A titled section with a Factory-style accent tick before the label. */
function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section>
      <h3 className="mb-2 flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        <span className="h-3 w-0.5 bg-status-running" aria-hidden />
        {title}
      </h3>
      {children}
    </section>
  );
}

function Empty({ children }: { children: React.ReactNode }) {
  return <p className="text-xs text-muted-foreground">{children}</p>;
}

function Th({
  className = "",
  children,
}: {
  className?: string;
  children: React.ReactNode;
}) {
  return <th className={`px-3 py-1.5 font-medium ${className}`}>{children}</th>;
}
function Td({
  className = "",
  children,
  colSpan,
}: {
  className?: string;
  children: React.ReactNode;
  colSpan?: number;
}) {
  return (
    <td className={`px-3 py-1.5 ${className}`} colSpan={colSpan}>
      {children}
    </td>
  );
}
