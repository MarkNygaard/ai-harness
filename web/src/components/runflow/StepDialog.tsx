import { useEffect, useRef } from "react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Badge } from "@/components/ui/badge";
import { Markdown } from "@/components/Markdown";
import type { Activity, NodeView } from "@/types/run";
import {
  elapsedMs,
  formatDuration,
  formatTokens,
  statusLabel,
  totalTokens,
} from "./format";

const STATUS_VARIANT: Record<
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

/** Click a completed step → its full output + metadata in a dialog. */
export function StepDialog({
  view,
  onClose,
}: {
  view: NodeView | null;
  onClose: () => void;
}) {
  const now = Date.now();
  return (
    <Dialog open={!!view} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="flex max-h-[85vh] flex-col gap-0 overflow-hidden sm:max-w-3xl">
        {view && (
          <>
            <DialogHeader className="shrink-0 pb-2">
              <DialogTitle className="flex items-center gap-2">
                <span className="truncate font-mono">{view.id}</span>
                <Badge variant={STATUS_VARIANT[view.status] ?? "default"}>
                  {statusLabel(view.status)}
                </Badge>
                {view.status === "running" && view.liveProgress && (
                  <Badge
                    variant="secondary"
                    className="tabular-nums"
                    title={
                      view.liveProgress.kind === "loop"
                        ? "Loops up to this many times; stops early once the review is clean"
                        : undefined
                    }
                  >
                    {view.liveProgress.kind === "loop"
                      ? `iteration ${view.liveProgress.done}/${view.liveProgress.total}`
                      : `task ${view.liveProgress.done}/${view.liveProgress.total}`}
                  </Badge>
                )}
              </DialogTitle>
              <DialogDescription className="sr-only">
                Step output and details
              </DialogDescription>
            </DialogHeader>

            {/* One scroll region under the fixed title: the whole body scrolls
                together, so a large artifact never gets trapped in a short
                inner box on small screens. */}
            <div className="min-h-0 flex-1 overflow-auto">
              <dl className="grid grid-cols-2 gap-x-6 gap-y-1.5 border-y py-3 text-xs sm:grid-cols-3">
                <Meta label="CLI" value={view.provider ?? "—"} />
                <Meta label="Model" value={view.model ?? "—"} />
                <Meta
                  label="Duration"
                  value={formatDuration(
                    elapsedMs(view.started_at, view.ended_at, now),
                  )}
                />
                <Meta
                  label="Iterations"
                  value={
                    view.status === "running" &&
                    view.liveProgress?.kind === "loop"
                      ? `${view.liveProgress.done} / ${view.liveProgress.total} max`
                      : String(view.iterations)
                  }
                />
                <Meta
                  label="Input tokens"
                  value={formatTokens(view.usage.input)}
                />
                <Meta
                  label="Output tokens"
                  value={formatTokens(view.usage.output)}
                />
                {view.usage.cache_read != null && (
                  <Meta
                    label="Cache read"
                    value={formatTokens(view.usage.cache_read)}
                  />
                )}
                <Meta
                  label="Total tokens"
                  value={formatTokens(totalTokens(view.usage))}
                />
              </dl>

              {view.note && (
                <p className="border-b py-2 text-xs text-muted-foreground">
                  {view.note}
                </p>
              )}

              {view.status === "running" && view.activityLog.length > 0 && (
                <ActivityFeed lines={view.activityLog} />
              )}

              <div className="pt-3">
                <div className="mb-1 text-xs font-medium text-muted-foreground">
                  Output
                </div>
                <OutputView text={view.output} />
                {view.artifact && (
                  <ArtifactView
                    name={view.artifact}
                    content={view.artifact_content}
                  />
                )}
              </div>
            </div>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

/**
 * Live feed of the agent's activity while a step runs, rendered as typed cards
 * (tool calls, their results, and assistant text) rather than a flat log. Shows
 * current progress, not a full transcript — the real result lands in Output
 * when the step finishes. Auto-scrolls to the newest line as the agent works.
 */
function ActivityFeed({ lines }: { lines: Activity[] }) {
  const endRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [lines]);

  // Pair tool calls with their results by tool_id: the call card shows its
  // result inline (+ elapsed time), and a result that has a matching call isn't
  // repeated as its own line. Orphan results (no matching call) still show.
  const resultById = new Map<string, Activity>();
  const callIds = new Set<string>();
  for (const l of lines) {
    if (l.tool_id == null) continue;
    if (l.kind === "tool_result") resultById.set(l.tool_id, l);
    else if (l.kind === "tool") callIds.add(l.tool_id);
  }

  return (
    <div className="pt-3">
      <div className="mb-1 flex items-center gap-2 text-xs font-medium text-muted-foreground">
        <span className="size-1.5 animate-pulse rounded-full bg-current" />
        Activity
      </div>
      <div className="max-h-48 overflow-auto rounded-md bg-muted p-3">
        <ul className="flex flex-col gap-1 text-[11px] leading-relaxed">
          {lines.map((line, i) => {
            if (
              line.kind === "tool_result" &&
              line.tool_id != null &&
              callIds.has(line.tool_id)
            ) {
              return null; // shown inline with its call
            }
            const result =
              line.kind === "tool" && line.tool_id != null
                ? resultById.get(line.tool_id)
                : undefined;
            return <ActivityRow key={i} line={line} result={result} />;
          })}
        </ul>
        <div ref={endRef} />
      </div>
    </div>
  );
}

/** Elapsed label between a tool call and its result, or "" when not timeable. */
function pairDuration(call: Activity, result: Activity): string {
  if (!call.ts || !result.ts) return "";
  const ms = Date.parse(result.ts) - Date.parse(call.ts);
  return ms >= 0 ? ` ${formatDuration(ms)}` : "";
}

/**
 * One activity line, styled by kind. A `tool` call shows its name + input and,
 * when `result` is paired in, a ✓ + elapsed time and the result snippet below.
 */
function ActivityRow({ line, result }: { line: Activity; result?: Activity }) {
  if (line.kind === "tool") {
    return (
      <li className="font-mono text-muted-foreground">
        <div className="flex items-start gap-1.5 break-all">
          <span aria-hidden>🔧</span>
          <span className="break-words">
            <span className="font-medium text-foreground">{line.text}</span>
            {line.detail ? <span> {line.detail}</span> : null}
          </span>
          {result ? (
            <span
              className={`ml-auto whitespace-nowrap pl-2 ${
                result.is_error
                  ? "text-red-600 dark:text-red-400"
                  : "text-emerald-600 dark:text-emerald-400"
              }`}
            >
              {result.is_error ? "✗" : "✓"}
              {pairDuration(line, result)}
            </span>
          ) : null}
        </div>
        {result?.detail ? (
          <div
            className={`flex items-start gap-1.5 break-all pl-[1.1rem] ${
              result.is_error
                ? "text-red-600/90 dark:text-red-400/90"
                : "text-muted-foreground/80"
            }`}
          >
            <span aria-hidden>↳</span>
            <span className="break-words">{result.detail}</span>
          </div>
        ) : null}
      </li>
    );
  }
  if (line.kind === "tool_result") {
    return (
      <li
        className={`flex items-start gap-1.5 break-all font-mono ${
          line.is_error
            ? "text-red-600/90 dark:text-red-400/90"
            : "text-muted-foreground/80"
        }`}
      >
        <span aria-hidden>{line.is_error ? "✗" : "↳"}</span>
        <span className="break-words">{line.detail}</span>
      </li>
    );
  }
  return <li className="break-words text-muted-foreground">{line.text}</li>;
}

function Meta({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex flex-col">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="truncate tabular-nums">{value}</dd>
    </div>
  );
}
/**
 * A step's artifact (e.g. `exploration.md`): markdown files render formatted;
 * this is read-only info, so there's no raw/source toggle. Non-markdown
 * artifacts show as raw text.
 */
function ArtifactView({
  name,
  content,
}: {
  name: string;
  content: string | null;
}) {
  const isMarkdown = /\.(md|markdown)$/i.test(name);
  return (
    <div className="mt-4">
      <div className="mb-1 truncate font-mono text-xs font-medium text-muted-foreground">
        {name}
      </div>
      {!content ? (
        <p className={EMPTY}>No artifact produced for this step.</p>
      ) : isMarkdown ? (
        <div className={PROSE}>
          <Markdown>{content}</Markdown>
        </div>
      ) : (
        <pre className="whitespace-pre-wrap break-words rounded-md bg-muted p-3 font-mono text-xs leading-relaxed">
          {content}
        </pre>
      )}
    </div>
  );
}

/**
 * Where a tint belongs, now that a surface is the brightest thing on it.
 *
 * Since #318 a panel is white and the page behind it is grey, so a filled box
 * *inside* a dialog reads as content sunk below its own container — which is
 * backwards for the thing you opened the dialog to read.
 *
 * The line is prose versus machine text. An agent's summary and a rendered
 * `.md` artifact are what you came for: they sit on the surface, bounded by a
 * rule so you can see where the document starts and stops. Raw output, JSON and
 * the activity log are quoted material, and keep `bg-muted` — the same tint
 * `.markdown code` and `.markdown pre` already use, so an inline snippet inside
 * a prose block still reads as quoted.
 */
const PROSE = "rounded-md border border-border p-3";

/** Nothing to show. Outlined rather than filled, for the same reason. */
const EMPTY =
  "rounded-md border border-dashed border-border p-3 text-xs text-muted-foreground";

/**
 * A step's textual output: rendered as markdown (agents usually emit prose /
 * markdown), except JSON outputs (e.g. `output_format` verdicts) which stay a
 * formatted code block since markdown would mangle them. Read-only info, so no
 * raw/source toggle.
 */
function OutputView({ text }: { text: string | null }) {
  if (!text) {
    return <p className={EMPTY}>No textual output for this step.</p>;
  }
  const json = asPrettyJson(text);
  if (json !== null) {
    return (
      <pre className="whitespace-pre-wrap break-words rounded-md bg-muted p-3 font-mono text-xs leading-relaxed">
        {json}
      </pre>
    );
  }
  return (
    <div className={PROSE}>
      <Markdown>{text}</Markdown>
    </div>
  );
}

/** Pretty-printed JSON when `text` is a JSON object/array, else null. */
function asPrettyJson(text: string): string | null {
  const t = text.trim();
  if (!t.startsWith("{") && !t.startsWith("[")) return null;
  try {
    return JSON.stringify(JSON.parse(t), null, 2);
  } catch {
    return null;
  }
}
