import { useMemo } from "react";
import type { NodeView, Usage } from "@/types/run";
import { Badge } from "@/components/ui/badge";
import {
  elapsedMs,
  formatDuration,
  formatTokens,
  statusLabel,
  sumUsage,
  totalTokens,
} from "./format";

/** Aggregate token usage grouped by model (falling back to provider). */
export function usageByModel(
  nodes: NodeView[],
): Array<{ key: string; steps: number; usage: Usage; total: number }> {
  const groups = new Map<string, NodeView[]>();
  for (const n of nodes) {
    if (totalTokens(n.usage) === 0 && n.usage.cache_read == null) continue;
    const key = n.model ?? n.provider ?? "unknown";
    const list = groups.get(key) ?? [];
    list.push(n);
    groups.set(key, list);
  }
  return [...groups.entries()]
    .map(([key, list]) => {
      const usage = sumUsage(list.map((n) => n.usage));
      return { key, steps: list.length, usage, total: totalTokens(usage) };
    })
    .sort((a, b) => b.total - a.total);
}

const statusVariant: Record<string, "running" | "success" | "failed" | "skipped" | "default"> = {
  running: "running",
  success: "success",
  failed: "failed",
  cancelled: "failed",
  skipped: "skipped",
  pending: "default",
};

/** Factory-style task overview: per-step + per-model token accounting. */
export function TaskOverview({ nodes }: { nodes: NodeView[] }) {
  const byModel = useMemo(() => usageByModel(nodes), [nodes]);
  const totals = useMemo(() => sumUsage(nodes.map((n) => n.usage)), [nodes]);
  const now = Date.now();

  return (
    <div className="flex flex-col gap-5">
      <section>
        <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          By step
        </h3>
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
                  <Td className="text-muted-foreground">{n.model ?? n.provider ?? "—"}</Td>
                  <Td className="text-center">
                    <Badge variant={statusVariant[n.status] ?? "default"}>
                      {statusLabel(n.status)}
                    </Badge>
                  </Td>
                  <Td className="text-right tabular-nums text-muted-foreground">
                    {formatDuration(elapsedMs(n.started_at, n.ended_at, now))}
                  </Td>
                  <Td className="text-right tabular-nums">{formatTokens(n.usage.input)}</Td>
                  <Td className="text-right tabular-nums">{formatTokens(n.usage.output)}</Td>
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

      <section>
        <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          By model
        </h3>
        {byModel.length === 0 ? (
          <p className="text-xs text-muted-foreground">No token usage reported yet.</p>
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
                    <Td className="text-right tabular-nums text-muted-foreground">{g.steps}</Td>
                    <Td className="text-right tabular-nums">{formatTokens(g.usage.input)}</Td>
                    <Td className="text-right tabular-nums">{formatTokens(g.usage.output)}</Td>
                    <Td className="text-right tabular-nums text-muted-foreground">
                      {formatTokens(g.usage.cache_read)}
                    </Td>
                    <Td className="text-right font-medium tabular-nums">{formatTokens(g.total)}</Td>
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

function Th({ className = "", children }: { className?: string; children: React.ReactNode }) {
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
