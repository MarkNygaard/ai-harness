import { useState } from "react";
import { Handle, Position, type NodeProps } from "@xyflow/react";
import { Cpu, Loader2 } from "lucide-react";
import { cn } from "@/lib/utils";
import { useNow } from "@/lib/useNow";
import type { RunNodeData } from "./layout";
import {
  elapsedMs,
  formatDuration,
  formatTokens,
  statusColor,
  statusLabel,
  totalTokens,
} from "./format";

/**
 * A single workflow step. Shows status, provider/model, a live-ticking elapsed
 * timer while running, and token count; hovering reveals a details panel.
 */
export function RunNode({ data }: NodeProps) {
  const { view } = data as RunNodeData;
  const [hover, setHover] = useState(false);
  const running = view.status === "running";
  const now = useNow(running);
  const elapsed = elapsedMs(view.started_at, view.ended_at, now);
  const color = statusColor(view.status);
  const tokens = totalTokens(view.usage);

  return (
    <div
      className="relative"
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
    >
      <Handle type="target" position={Position.Left} className="!h-2 !w-2 !border-0 !bg-border" />
      <div
        className={cn(
          "flex w-56 flex-col gap-1.5 rounded-lg border bg-card px-3 py-2 shadow-sm transition-shadow",
          running && "shadow-[0_0_0_3px_var(--status-running)]/20",
        )}
        style={{ borderColor: color, borderWidth: running ? 2 : 1 }}
      >
        <div className="flex items-center gap-1.5">
          {running ? (
            <Loader2 className="h-3 w-3 shrink-0 animate-spin" style={{ color }} />
          ) : (
            <span
              className="h-2 w-2 shrink-0 rounded-full"
              style={{ background: color }}
              aria-hidden
            />
          )}
          <span className="truncate text-[13px] font-semibold text-card-foreground" title={view.id}>
            {view.id}
          </span>
        </div>

        <div className="flex items-center justify-between gap-2 text-[11px] text-muted-foreground">
          <span className="truncate" style={{ color }}>
            {statusLabel(view.status)}
          </span>
          {view.started_at && <span className="tabular-nums">{formatDuration(elapsed)}</span>}
        </div>

        <div className="flex items-center justify-between gap-2 text-[10px] text-muted-foreground">
          <span className="flex min-w-0 items-center gap-1 truncate">
            {view.provider && <Cpu className="h-2.5 w-2.5 shrink-0" />}
            <span className="truncate">{view.model ?? view.provider ?? "—"}</span>
          </span>
          {tokens > 0 && (
            <span className="shrink-0 whitespace-nowrap tabular-nums">
              {formatTokens(tokens)} tok
            </span>
          )}
        </div>
      </div>
      <Handle type="source" position={Position.Right} className="!h-2 !w-2 !border-0 !bg-border" />

      {hover && <NodeHoverCard view={view} elapsed={elapsed} />}
    </div>
  );
}

function NodeHoverCard({
  view,
  elapsed,
}: {
  view: RunNodeData["view"];
  elapsed: number | null;
}) {
  const rows: Array<[string, string]> = [
    ["Status", statusLabel(view.status)],
    ["Provider", view.provider ?? "—"],
    ["Model", view.model ?? "—"],
    ["Elapsed", formatDuration(elapsed)],
    ["Iterations", String(view.iterations)],
    ["Input tokens", formatTokens(view.usage.input)],
    ["Output tokens", formatTokens(view.usage.output)],
    ["Cache read", formatTokens(view.usage.cache_read)],
  ];
  return (
    <div className="absolute left-1/2 top-full z-50 mt-2 w-64 -translate-x-1/2 rounded-lg border border-border bg-popover p-3 text-popover-foreground shadow-xl">
      <div className="mb-2 text-[13px] font-semibold">{view.id}</div>
      <dl className="grid grid-cols-2 gap-x-3 gap-y-1 text-[11px]">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-muted-foreground">{k}</dt>
            <dd className="text-right tabular-nums">{v}</dd>
          </div>
        ))}
      </dl>
      {view.note && (
        <p className="mt-2 border-t border-border pt-2 text-[11px] text-muted-foreground">
          {view.note}
        </p>
      )}
      {view.output && (
        <pre className="mt-2 max-h-28 overflow-auto whitespace-pre-wrap break-words border-t border-border pt-2 font-mono text-[10px] leading-snug text-muted-foreground">
          {view.output.slice(0, 600)}
          {view.output.length > 600 ? "…" : ""}
        </pre>
      )}
    </div>
  );
}
