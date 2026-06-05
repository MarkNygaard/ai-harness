import { useMemo } from "react";
import type { NodeView } from "@/types/run";
import { Badge } from "@/components/ui/badge";
import {
  elapsedMs,
  formatDuration,
  formatTokens,
  statusColor,
  statusLabel,
  sumUsage,
  totalTokens,
} from "./format";
import {
  timeByStep,
  usageByModel,
  usageByType,
  waterfall,
  type WaterfallRow,
} from "./overview";

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

/** Factory-style task overview: headline metrics, waterfall, time + token breakdowns. */
export function TaskOverview({ nodes }: { nodes: NodeView[] }) {
  const now = Date.now();
  const byModel = useMemo(() => usageByModel(nodes), [nodes]);
  const totals = useMemo(() => sumUsage(nodes.map((n) => n.usage)), [nodes]);
  const bars = useMemo(() => waterfall(nodes, now), [nodes, now]);
  const steps = useMemo(() => timeByStep(nodes, now), [nodes, now]);
  const segments = useMemo(() => usageByType(totals), [totals]);

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
    <div className="flex flex-col gap-6">
      {/* Headline metrics */}
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <Metric label="Total time" value={formatDuration(wallMs)} />
        <Metric label="Total tokens" value={formatTokens(totalTok)} />
        <Metric
          label="Steps"
          value={`${done}/${nodes.length}`}
          hint={failed ? `${failed} failed` : undefined}
        />
        <Metric label="Models" value={String(byModel.length || "—")} />
      </div>

      {/* Milestone waterfall */}
      <section>
        <SectionTitle>Milestone waterfall</SectionTitle>
        {bars.length === 0 ? (
          <Empty>No timing recorded yet.</Empty>
        ) : (
          <div className="flex flex-col gap-1 rounded-lg border border-border p-3">
            {bars.map((b) => (
              <WaterfallBar key={b.id} row={b} />
            ))}
          </div>
        )}
      </section>

      {/* Time by step */}
      <section>
        <SectionTitle>Time by step</SectionTitle>
        {steps.length === 0 ? (
          <Empty>No timing recorded yet.</Empty>
        ) : (
          <div className="flex flex-col gap-2 rounded-lg border border-border p-3">
            {steps.map((s) => (
              <div key={s.id} className="flex items-center gap-3 text-[12px]">
                <div
                  className="w-40 shrink-0 truncate font-medium"
                  title={s.id}
                >
                  {s.id}
                </div>
                <div className="h-3 flex-1 overflow-hidden rounded-full bg-secondary/50">
                  <div
                    className="h-full rounded-full"
                    style={{
                      width: `${longest > 0 ? (s.durationMs / longest) * 100 : 0}%`,
                      backgroundColor: statusColor(s.status),
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
      </section>

      {/* Tokens by type */}
      <section>
        <SectionTitle>Tokens by type</SectionTitle>
        {segTotal === 0 ? (
          <Empty>No token usage reported yet.</Empty>
        ) : (
          <div className="rounded-lg border border-border p-3">
            <div className="flex h-4 w-full overflow-hidden rounded-full bg-secondary/50">
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
                    className="size-2.5 rounded-sm"
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
      </section>

      {/* By step (detail table) */}
      <section>
        <SectionTitle>By step</SectionTitle>
        <div className="overflow-hidden rounded-lg border border-border">
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
              </tr>
            </tfoot>
          </table>
        </div>
      </section>

      {/* By model */}
      <section>
        <SectionTitle>By model</SectionTitle>
        {byModel.length === 0 ? (
          <Empty>No token usage reported yet.</Empty>
        ) : (
          <div className="overflow-hidden rounded-lg border border-border">
            <table className="w-full text-[12px]">
              <thead className="bg-secondary/50 text-muted-foreground">
                <tr>
                  <Th className="text-left">Model</Th>
                  <Th className="text-right">Steps</Th>
                  <Th className="text-right">In</Th>
                  <Th className="text-right">Out</Th>
                  <Th className="text-right">Cache</Th>
                  <Th className="text-right">Total</Th>
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
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
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

function WaterfallBar({ row }: { row: WaterfallRow }) {
  return (
    <div className="flex items-center gap-3 text-[12px]">
      <div className="w-40 shrink-0 truncate font-medium" title={row.id}>
        {row.id}
      </div>
      <div className="relative h-4 flex-1">
        <div
          className="absolute top-0 h-full min-w-0.75 rounded-sm"
          style={{
            left: `${row.offset * 100}%`,
            width: `${row.width * 100}%`,
            backgroundColor: statusColor(row.status),
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
    <div className="rounded-lg border border-border p-3">
      <div className="text-[11px] uppercase tracking-wide text-muted-foreground">
        {label}
      </div>
      <div className="mt-1 text-xl font-semibold tabular-nums">{value}</div>
      {hint && <div className="text-[11px] text-status-failed">{hint}</div>}
    </div>
  );
}

function SectionTitle({ children }: { children: React.ReactNode }) {
  return (
    <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
      {children}
    </h3>
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
