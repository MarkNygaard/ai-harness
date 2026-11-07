import { useState } from "react";
import {
  Link,
  useLocation,
  useNavigate,
  useSearchParams,
} from "react-router-dom";
import { IconChevronRight, IconPlus, IconX } from "@tabler/icons-react";
import { AppShell } from "@/components/AppShell";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { TriggerKindIcon } from "@/components/triggers/TriggerKindIcon";
import { formatWhen } from "@/components/triggers/ScheduleForm";
import { useLinearDiscovery } from "@/lib/linear";
import { useRestoreTrash } from "@/lib/projects";
import { useLinearTriggerHome } from "@/lib/trigger-prefs";
import {
  TRIGGER_KINDS,
  type Trigger,
  type TriggerKind,
  type TriggerState,
  describeLinearWhen,
  describeScheduleWhen,
  filterTriggers,
  kindMeta,
  triggerHref,
  useTriggers,
} from "@/lib/triggers";
import type { LinearSource } from "@/types/linear";

/** Base UI's Select needs a concrete value for "no filter". */
const ALL = "__all__";

/** Set by a trigger's page after a delete, so the list can offer Undo. */
export interface DeletedState {
  deleted?: { label: string; trashId: string | null };
}

/**
 * Every trigger, of every kind, in one flat list.
 *
 * Not grouped by project: a project is one column and one filter among
 * several. The filters live in the URL, so `/schedules` can redirect to
 * `?kind=schedule` and a project page can link to its own triggers.
 */
export function TriggersPage() {
  const triggers = useTriggers();
  const home = useLinearTriggerHome();
  const [params, setParams] = useSearchParams();
  const [picking, setPicking] = useState(false);
  const location = useLocation();
  const navigate = useNavigate();
  const deleted = (location.state as DeletedState | null)?.deleted;

  const kind = (params.get("kind") as TriggerKind | null) ?? null;
  const project = params.get("project");
  const workflow = params.get("workflow");
  const q = params.get("q") ?? "";
  const setParam = (key: string, value: string | null) => {
    const next = new URLSearchParams(params);
    if (value) next.set(key, value);
    else next.delete(key);
    setParams(next, { replace: true });
  };

  const all = triggers.data ?? [];
  const rows = filterTriggers(all, { kind, project, workflow, q });
  const projects = [...new Set(all.map((t) => t.project))].sort();
  const workflows = [...new Set(all.map((t) => t.workflow))].sort();
  const filtered = !!(kind || project || workflow || q);

  return (
    <AppShell
      title="Triggers"
      viewActions={
        <Button size="sm" onClick={() => setPicking(true)}>
          <IconPlus /> New trigger
        </Button>
      }
    >
      <div className="mx-auto flex max-w-5xl flex-col gap-5 p-6">
        <div className="sm:pr-64">
          <h1 className="text-lg font-semibold">Triggers</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            What starts runs: Linear issues and schedules. Each trigger starts
            one workflow in one project.
          </p>
        </div>

        {deleted && (
          <UndoDelete
            deleted={deleted}
            onDone={() => navigate(".", { replace: true, state: null })}
          />
        )}

        <div className="flex flex-wrap items-center gap-2">
          <FilterSelect
            value={kind}
            placeholder="All kinds"
            options={TRIGGER_KINDS.map((k) => ({
              value: k.kind,
              label: k.label,
            }))}
            onChange={(v) => setParam("kind", v)}
          />
          <FilterSelect
            value={project}
            placeholder="All projects"
            options={projects.map((p) => ({ value: p, label: p }))}
            onChange={(v) => setParam("project", v)}
          />
          <FilterSelect
            value={workflow}
            placeholder="All workflows"
            options={workflows.map((w) => ({ value: w, label: w }))}
            onChange={(v) => setParam("workflow", v)}
          />
          <Input
            value={q}
            onChange={(e) => setParam("q", e.target.value || null)}
            placeholder="Search"
            className="w-48"
          />
          {filtered && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() =>
                setParams(new URLSearchParams(), { replace: true })
              }
            >
              <IconX /> Clear
            </Button>
          )}
        </div>

        {triggers.errors.map((e) => (
          <p key={e.message} className="text-sm text-destructive">
            {e.message}
          </p>
        ))}
        {triggers.isLoading && (
          <p className="text-sm text-muted-foreground">Loading…</p>
        )}
        {triggers.data && rows.length === 0 && (
          <Card>
            <CardContent className="py-8 text-center text-sm text-muted-foreground">
              {filtered
                ? "No trigger matches these filters."
                : "No triggers yet. Add one to start runs from Linear or on a schedule."}
            </CardContent>
          </Card>
        )}

        <div className="flex flex-col gap-2">
          {rows.map((t) => (
            <TriggerRow key={t.key} trigger={t} href={triggerHref(t, home)} />
          ))}
        </div>
      </div>

      <KindPicker open={picking} onOpenChange={setPicking} project={project} />
    </AppShell>
  );
}

function FilterSelect({
  value,
  placeholder,
  options,
  onChange,
}: {
  value: string | null;
  placeholder: string;
  options: { value: string; label: string }[];
  onChange: (v: string | null) => void;
}) {
  return (
    <Select
      value={value ?? ALL}
      onValueChange={(v) => onChange(v == null || v === ALL ? null : v)}
    >
      <SelectTrigger className="w-44">
        <SelectValue>
          {(v: string | null) =>
            !v || v === ALL
              ? placeholder
              : (options.find((o) => o.value === v)?.label ?? v)
          }
        </SelectValue>
      </SelectTrigger>
      <SelectContent>
        <SelectItem value={ALL}>{placeholder}</SelectItem>
        {options.map((o) => (
          <SelectItem key={o.value} value={o.value}>
            {o.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

const STATE_BADGE: Record<
  TriggerState,
  { label: string; variant: "success" | "outline" | "secondary" }
> = {
  on: { label: "on", variant: "success" },
  "dry-run": { label: "dry-run", variant: "secondary" },
  off: { label: "off", variant: "outline" },
};

function TriggerRow({ trigger: t, href }: { trigger: Trigger; href: string }) {
  const state = STATE_BADGE[t.state];
  return (
    <Link
      to={href}
      className="group rounded-xl outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
    >
      <Card size="sm" className="transition-colors group-hover:bg-muted/40">
        <CardContent className="flex items-center gap-3 px-4 py-3">
          <span title={kindMeta(t.kind).label}>
            <TriggerKindIcon kind={t.kind} className="size-5" />
          </span>
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="truncate text-sm font-medium">
                {t.kind === "schedule" ? (
                  t.schedule.name
                ) : (
                  <LinearWhen binding={t.binding} />
                )}
              </span>
              <Badge variant={state.variant} className="shrink-0 text-[10px]">
                {state.label}
              </Badge>
            </div>
            <div className="truncate text-[12px] text-muted-foreground">
              starts <span className="font-mono">{t.workflow}</span> in{" "}
              <span className="font-mono">{t.project}</span>
              {t.kind === "schedule" && (
                <> · {describeScheduleWhen(t.schedule)}</>
              )}
            </div>
          </div>
          <div className="hidden shrink-0 text-right text-[11px] text-muted-foreground sm:block">
            {t.kind === "schedule" ? (
              t.schedule.enabled ? (
                <>Next {formatWhen(t.schedule.next_run_at)}</>
              ) : (
                "paused"
              )
            ) : (
              <>polls every {t.binding.poll_interval_secs}s</>
            )}
          </div>
          <IconChevronRight className="size-4 shrink-0 text-muted-foreground transition-transform group-hover:translate-x-0.5" />
        </CardContent>
      </Card>
    </Link>
  );
}

/**
 * A Linear trigger's "when", with its status named. One discovery per
 * project, shared by every row of it through the query cache, and not polled.
 */
function LinearWhen({ binding }: { binding: LinearSource }) {
  const discovery = useLinearDiscovery(binding.project);
  return <>{describeLinearWhen(binding, discovery.data?.teams)}</>;
}

/** Choose what kind of trigger to add. Each kind's own page does the rest. */
function KindPicker({
  open,
  onOpenChange,
  project,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  project: string | null;
}) {
  const navigate = useNavigate();
  const suffix = project ? `?project=${encodeURIComponent(project)}` : "";
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>New trigger</DialogTitle>
          <DialogDescription>What should start the run?</DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-2">
          {TRIGGER_KINDS.map((k) => (
            <button
              key={k.kind}
              type="button"
              onClick={() => {
                onOpenChange(false);
                navigate(`/triggers/new/${k.kind}${suffix}`);
              }}
              className="flex items-start gap-3 rounded-lg border border-border p-3 text-left transition-colors outline-none hover:bg-muted/50 focus-visible:ring-3 focus-visible:ring-ring/50"
            >
              <TriggerKindIcon kind={k.kind} className="mt-0.5 size-5" />
              <span className="flex flex-col gap-0.5">
                <span className="text-sm font-medium">{k.label}</span>
                <span className="text-[12px] text-muted-foreground">
                  {k.description}
                </span>
              </span>
            </button>
          ))}
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** The trigger just deleted, with an Undo while it is still in the bin. */
function UndoDelete({
  deleted,
  onDone,
}: {
  deleted: NonNullable<DeletedState["deleted"]>;
  onDone: () => void;
}) {
  const restore = useRestoreTrash();
  return (
    <div className="flex items-center gap-2 rounded-md border border-border bg-muted/30 px-3 py-2 text-[12px]">
      <span className="min-w-0 flex-1">Deleted {deleted.label}.</span>
      {deleted.trashId && (
        <Button
          variant="outline"
          size="sm"
          disabled={restore.isPending}
          onClick={() =>
            restore.mutate(deleted.trashId!, { onSuccess: onDone })
          }
        >
          Undo
        </Button>
      )}
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label="Dismiss"
        onClick={onDone}
      >
        <IconX />
      </Button>
      {restore.isError && (
        <span className="text-destructive">{restore.error.message}</span>
      )}
    </div>
  );
}
