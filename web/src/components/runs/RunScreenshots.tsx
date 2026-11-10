import { useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { Skeleton } from "@/components/ui/skeleton";
import {
  type RunScreenshots as Data,
  type Screenshot,
  groupByPage,
  useScreenshotUrl,
} from "@/lib/screenshots";

/** The run's screenshots, grouped by page; click one to see it full size. */
export function RunScreenshots({ runId, data }: { runId: string; data: Data }) {
  const [open, setOpen] = useState<Screenshot | null>(null);

  if (data.expired != null) {
    return (
      <p className="text-sm text-muted-foreground">
        This run took {data.expired} screenshot{data.expired === 1 ? "" : "s"}.
        They were deleted after {data.ttl_days} days.
      </p>
    );
  }

  return (
    <div className="flex flex-col gap-6">
      <p className="text-xs text-muted-foreground">
        Taken by the run&rsquo;s agents of the pages its change touches. Kept
        for {data.ttl_days} days after the run.
      </p>
      {groupByPage(data.screenshots).map(([page, shots]) => (
        <section key={page} className="flex flex-col gap-2">
          <h3 className="truncate font-mono text-sm">{page}</h3>
          <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
            {shots.map((s) => (
              <Shot
                key={s.file}
                runId={runId}
                shot={s}
                onOpen={() => setOpen(s)}
              />
            ))}
          </div>
        </section>
      ))}
      <Dialog open={!!open} onOpenChange={(o) => !o && setOpen(null)}>
        <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-5xl">
          {open && (
            <>
              <DialogTitle className="truncate font-mono text-sm">
                {open.page ?? open.file}
                {open.viewport ? ` · ${open.viewport}` : ""}
              </DialogTitle>
              {open.note && <DialogDescription>{open.note}</DialogDescription>}
              <ShotImage runId={runId} file={open.file} alt={open.note} />
            </>
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}

function Shot({
  runId,
  shot,
  onOpen,
}: {
  runId: string;
  shot: Screenshot;
  onOpen: () => void;
}) {
  return (
    <Card size="sm" className="overflow-hidden">
      <button
        type="button"
        onClick={onOpen}
        className="block w-full cursor-zoom-in border-b border-border bg-muted/30"
        title="Show full size"
      >
        <ShotImage
          runId={runId}
          file={shot.file}
          alt={shot.note}
          className="max-h-72 w-full object-contain object-top"
        />
      </button>
      <CardContent className="flex flex-col gap-1 px-3 py-2">
        <div className="flex items-center gap-2">
          {shot.viewport && (
            <Badge variant="outline" className="text-[10px]">
              {shot.viewport}
            </Badge>
          )}
          <span className="truncate font-mono text-[11px] text-muted-foreground">
            {shot.file}
          </span>
        </div>
        {shot.note && <p className="text-[12px]">{shot.note}</p>}
      </CardContent>
    </Card>
  );
}

function ShotImage({
  runId,
  file,
  alt,
  className,
}: {
  runId: string;
  file: string;
  alt?: string;
  className?: string;
}) {
  const url = useScreenshotUrl(runId, file);
  if (!url) return <Skeleton className="h-48 w-full" />;
  return <img src={url} alt={alt ?? file} className={className} />;
}
