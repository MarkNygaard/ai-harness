import { useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
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
import { OptionSelect } from "@/components/triggers/fields";
import { TriggerKindIcon } from "@/components/triggers/TriggerKindIcon";
import { formatWhen } from "@/components/triggers/ScheduleForm";
import {
  TRIGGER_KINDS,
  type Trigger,
  type TriggerKind,
  type TriggerState,
  type TriggerSystem,
  describeWhen,
  filterTriggers,
  kindMeta,
  triggerHref,
  triggerName,
  triggerProject,
  triggerState,
  triggerWorkflow,
  useTriggerSystem,
  useTriggers,
} from "@/lib/triggers";

/**
 * Every trigger, of every kind, in one flat list.
 *
 * Not grouped by project: a project is one column and one filter among
 * several. The filters live in the URL, so `/schedules` can redirect to
 * `?kind=schedule` and a project page can link to its own triggers.
 */
export function TriggersPage() {
  const triggers = useTriggers();
  const system = useTriggerSystem();
  const [params, setParams] = useSearchParams();
  const [picking, setPicking] = useState(false);

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
  const projects = [...new Set(all.map(triggerProject))].sort();
  const workflows = [...new Set(all.map(triggerWorkflow))].sort();
  const filtered = !!(kind || project || workflow || q);
  const hasLinear = all.some((t) => t.kind === "linear");

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
            What starts runs by itself: Linear issues, webhooks and schedules.
            Each trigger starts one workflow in one project; what happens after
            that is the workflow&rsquo;s job.
          </p>
        </div>

        {system.data === "classic" && (hasLinear || kind === "linear") && (
          <div className="rounded-md border border-border bg-muted/40 px-3 py-2 text-[12px]">
            <span className="font-medium">Linear triggers here are idle.</span>{" "}
            This harness still starts Linear runs from each project&rsquo;s
            classic bindings. Set up the triggers you need, then switch under{" "}
            <Link to="/settings/general" className="underline">
              Settings → General
            </Link>
            .
          </div>
        )}

        <div className="flex flex-wrap items-center gap-2">
          <div className="w-40">
            <OptionSelect
              value={kind ?? ""}
              onChange={(v) => setParam("kind", v || null)}
              none="All kinds"
              placeholder="All kinds"
              options={TRIGGER_KINDS.map((k) => ({
                value: k.kind,
                label: k.label,
              }))}
            />
          </div>
          <div className="w-44">
            <OptionSelect
              value={project ?? ""}
              onChange={(v) => setParam("project", v || null)}
              none="All projects"
              placeholder="All projects"
              options={projects.map((p) => ({ value: p, label: p }))}
            />
          </div>
          <div className="w-44">
            <OptionSelect
              value={workflow ?? ""}
              onChange={(v) => setParam("workflow", v || null)}
              none="All workflows"
              placeholder="All workflows"
              options={workflows.map((w) => ({ value: w, label: w }))}
            />
          </div>
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
                : "No triggers yet. Add one to start runs from Linear, a webhook or a schedule."}
            </CardContent>
          </Card>
        )}

        <div className="flex flex-col gap-2">
          {rows.map((t) => (
            <TriggerRow
              key={t.key}
              trigger={t}
              system={system.data ?? "classic"}
            />
          ))}
        </div>
      </div>

      <KindPicker open={picking} onOpenChange={setPicking} project={project} />
    </AppShell>
  );
}

const STATE_BADGE: Record<
  TriggerState,
  {
    label: string;
    variant: "success" | "outline" | "secondary";
    title?: string;
  }
> = {
  on: { label: "on", variant: "success" },
  idle: {
    label: "idle",
    variant: "secondary",
    title: "Classic bindings start Linear runs on this harness",
  },
  off: { label: "off", variant: "outline" },
};

function TriggerRow({
  trigger: t,
  system,
}: {
  trigger: Trigger;
  system: TriggerSystem;
}) {
  const state = STATE_BADGE[triggerState(t, system)];
  const lastError = t.kind === "schedule" ? null : t.trigger.last_error;
  const shadowedBy = t.kind === "schedule" ? null : t.trigger.shadowed_by;
  return (
    <Link
      to={triggerHref(t)}
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
                {triggerName(t)}
              </span>
              <Badge
                variant={state.variant}
                title={state.title}
                className="shrink-0 text-[10px]"
              >
                {state.label}
              </Badge>
              {shadowedBy && (
                <Badge
                  variant="outline"
                  title={`"${shadowedBy.name}" starts runs from the same Linear status, so this one doesn't fire. Remove one, and let a route node in the other's workflow choose.`}
                  className="shrink-0 border-destructive/50 text-[10px] text-destructive"
                >
                  held back
                </Badge>
              )}
              {lastError && (
                <Badge
                  variant="outline"
                  title={lastError}
                  className="shrink-0 border-destructive/50 text-[10px] text-destructive"
                >
                  failed to start
                </Badge>
              )}
            </div>
            <div className="truncate text-[12px] text-muted-foreground">
              {describeWhen(t)} →{" "}
              <span className="font-mono">{triggerWorkflow(t)}</span> in{" "}
              <span className="font-mono">{triggerProject(t)}</span>
            </div>
          </div>
          <div className="hidden shrink-0 text-right text-[11px] text-muted-foreground sm:block">
            <LastOrNext trigger={t} />
          </div>
          <IconChevronRight className="size-4 shrink-0 text-muted-foreground transition-transform group-hover:translate-x-0.5" />
        </CardContent>
      </Card>
    </Link>
  );
}

function LastOrNext({ trigger: t }: { trigger: Trigger }) {
  if (t.kind === "schedule") {
    return t.schedule.enabled ? (
      <>Next {formatWhen(t.schedule.next_run_at)}</>
    ) : (
      <>paused</>
    );
  }
  return t.trigger.last_fired_at ? (
    <>Last {formatWhen(t.trigger.last_fired_at)}</>
  ) : (
    <>never fired</>
  );
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
