import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Badge } from "@/components/ui/badge";
import type { NodeView } from "@/types/run";
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
      <DialogContent className="max-h-[85vh] gap-0 overflow-hidden sm:max-w-2xl">
        {view && (
          <>
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <span className="truncate font-mono">{view.id}</span>
                <Badge variant={STATUS_VARIANT[view.status] ?? "default"}>
                  {statusLabel(view.status)}
                </Badge>
              </DialogTitle>
              <DialogDescription className="sr-only">
                Step output and details
              </DialogDescription>
            </DialogHeader>

            <dl className="grid grid-cols-2 gap-x-6 gap-y-1.5 border-y py-3 text-xs sm:grid-cols-3">
              <Meta label="Provider" value={view.provider ?? "—"} />
              <Meta label="Model" value={view.model ?? "—"} />
              <Meta
                label="Duration"
                value={formatDuration(
                  elapsedMs(view.started_at, view.ended_at, now),
                )}
              />
              <Meta label="Iterations" value={String(view.iterations)} />
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

            <div className="min-h-0 flex-1 overflow-auto pt-3">
              <div className="mb-1 text-xs font-medium text-muted-foreground">
                Output
              </div>
              <TextOrEmpty
                text={view.output}
                empty="No textual output for this step."
              />
              {view.artifact && (
                <div className="mt-4">
                  <div className="mb-1 font-mono text-xs font-medium text-muted-foreground">
                    {view.artifact}
                  </div>
                  <TextOrEmpty
                    text={view.artifact_content}
                    empty="No artifact produced for this step."
                  />
                </div>
              )}
            </div>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

function Meta({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex flex-col">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="truncate tabular-nums">{value}</dd>
    </div>
  );
}
function TextOrEmpty({ text, empty }: { text: string | null; empty: string }) {
  return text ? (
    <pre className="max-h-[50vh] overflow-auto whitespace-pre-wrap break-words rounded-md bg-muted p-3 font-mono text-xs leading-relaxed">
      {text}
    </pre>
  ) : (
    <p className="rounded-md bg-muted p-3 text-xs text-muted-foreground">
      {empty}
    </p>
  );
}
