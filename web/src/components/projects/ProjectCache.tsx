import { useState } from "react";
import { IconTrash } from "@tabler/icons-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  useClearProjectCache,
  useProjectCacheSize,
  useSetProjectCacheCap,
} from "@/lib/projects";
import type { Project } from "@/types/project";

const gb = (n: number) => (n / 1_073_741_824).toFixed(1);

/** A project's Cache tab: cache sizes, the build-cache cap, and a clear. */
export function ProjectCachePanel({ project }: { project: Project }) {
  const size = useProjectCacheSize(project.name);
  const setCap = useSetProjectCacheCap();
  const clear = useClearProjectCache();
  const [raw, setRaw] = useState(project.cargo_target_cap_gb?.toString() ?? "");
  const capGb = raw.trim() === "" ? null : Number(raw);
  const isValid =
    raw.trim() === "" ||
    (Number.isFinite(capGb) && Number.isInteger(capGb) && (capGb ?? 0) > 0);
  const hasChanged =
    (project.cargo_target_cap_gb == null && raw.trim() !== "") ||
    (project.cargo_target_cap_gb != null &&
      Number(project.cargo_target_cap_gb) !== capGb);

  const shown = (f: (d: NonNullable<typeof size.data>) => string) =>
    size.isLoading || size.data == null ? "—" : f(size.data);

  const rows: [string, string][] = [
    [
      "Build cache (this project)",
      shown((d) => `${gb(d.bytes)} GB / ${d.cap_gb} GB`),
    ],
    [
      "Workflow cache (this project)",
      shown((d) => `${gb(d.workflow_bytes)} GB / ${d.workflow_cap_gb} GB`),
    ],
    [
      "Dependencies (shared)",
      shown((d) => `${gb(d.deps_bytes)} GB / ${d.deps_cap_gb} GB`),
    ],
    ["Git mirrors (shared)", shown((d) => `${gb(d.git_bytes)} GB`)],
  ];

  return (
    <div className="flex flex-col gap-5">
      <p className="text-sm text-muted-foreground">
        View size, set a per-project cap, or clear the build cache. A blank cap
        falls back to the server default. The dependency and git caches are
        shared by every project — a pnpm store or NuGet folder is
        content-addressed, so no project owns an entry in it.
      </p>

      <dl className="flex flex-col divide-y divide-border rounded-md border border-border text-sm">
        {rows.map(([label, value]) => (
          <div key={label} className="flex justify-between gap-4 px-3 py-2">
            <dt className="text-muted-foreground">{label}</dt>
            <dd className="font-medium tabular-nums">{value}</dd>
          </div>
        ))}
      </dl>
      {size.isError && (
        <p className="text-xs text-destructive">{size.error.message}</p>
      )}

      <div className="flex flex-col gap-1">
        <label
          className="text-xs font-medium text-muted-foreground"
          htmlFor={`cap-${project.name}`}
        >
          Build cache cap (GiB)
        </label>
        <div className="flex items-center gap-2">
          <Input
            id={`cap-${project.name}`}
            type="number"
            min={1}
            placeholder={`default (${size.data?.cap_gb ?? 50} GB)`}
            value={raw}
            onChange={(e) => setRaw(e.target.value)}
            className="w-40"
          />
          <Button
            size="sm"
            disabled={!hasChanged || !isValid || setCap.isPending}
            onClick={() => setCap.mutate({ name: project.name, cap_gb: capGb })}
          >
            Save
          </Button>
        </div>
        {!isValid && raw.trim() !== "" && (
          <div className="text-xs text-destructive">
            Cap must be a positive whole number.
          </div>
        )}
        {setCap.error && (
          <div className="text-xs text-destructive">{setCap.error.message}</div>
        )}
      </div>

      <div className="flex flex-col items-start gap-1">
        <Button
          variant="destructive"
          size="sm"
          disabled={clear.isPending}
          onClick={() => {
            if (
              confirm(
                `Clear build cache for "${project.name}"? The next run rebuilds from scratch (cold).`,
              )
            ) {
              clear.mutate(project.name);
            }
          }}
        >
          <IconTrash className="size-3.5" /> Clear build cache
        </Button>
        {clear.error && (
          <div className="text-xs text-destructive">{clear.error.message}</div>
        )}
      </div>
    </div>
  );
}
