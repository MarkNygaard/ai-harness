import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import {
  IconClockPlay,
  IconPencil,
  IconPlayerPlay,
  IconPlus,
  IconTrash,
} from "@tabler/icons-react";
import { AppShell } from "@/components/AppShell";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
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
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { isAdminOf, useAuthStatus } from "@/lib/auth";
import { useWorkflowList } from "@/lib/authoring";
import { useProjects } from "@/lib/projects";
import {
  type Schedule,
  type ScheduleInput,
  PRESETS,
  browserTimezone,
  describeCron,
  useDeleteSchedule,
  useRunScheduleNow,
  useSaveSchedule,
  useSchedulePreview,
  useSchedules,
} from "@/lib/schedules";

/** Base UI's Select needs a concrete value for "write my own". */
const CUSTOM = "__custom__";

function formatWhen(iso: string | null): string {
  if (!iso) return "—";
  return new Date(iso).toLocaleString(undefined, {
    weekday: "short",
    day: "numeric",
    month: "short",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** Workflows on a schedule: no Linear issue, just a time. */
export function SchedulesPage() {
  const schedules = useSchedules();
  const admin = isAdminOf(useAuthStatus().data);
  const [editing, setEditing] = useState<Schedule | "new" | null>(null);
  const list = schedules.data?.schedules ?? [];

  return (
    <AppShell
      title="Schedules"
      viewActions={
        admin && (
          <Button size="sm" onClick={() => setEditing("new")}>
            <IconPlus />
            New schedule
          </Button>
        )
      }
    >
      <div className="mx-auto flex max-w-4xl flex-col gap-5 p-6">
        <div className="sm:pr-64">
          <h1 className="text-lg font-semibold">Schedules</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Run a workflow at set times, with no Linear issue involved, such as
            a weekly report from Grafana. Each run shows up under Runs with its
            report like any other.
            {!admin && " Administrators create and change schedules."}
          </p>
        </div>

        {schedules.isError && (
          <p className="text-sm text-destructive">{schedules.error.message}</p>
        )}
        {schedules.isSuccess && list.length === 0 && (
          <Card>
            <CardContent className="py-8 text-center text-sm text-muted-foreground">
              No schedules yet.
              {admin &&
                " Create one to run a workflow every week, day or hour."}
            </CardContent>
          </Card>
        )}

        <div className="flex flex-col gap-2">
          {list.map((s) => (
            <ScheduleRow
              key={s.id}
              schedule={s}
              admin={admin}
              onEdit={() => setEditing(s)}
            />
          ))}
        </div>
      </div>

      {editing && (
        <ScheduleDialog
          schedule={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
        />
      )}
    </AppShell>
  );
}

function ScheduleRow({
  schedule: s,
  admin,
  onEdit,
}: {
  schedule: Schedule;
  admin: boolean;
  onEdit: () => void;
}) {
  const run = useRunScheduleNow();
  const remove = useDeleteSchedule();
  const words = describeCron(s.cron);
  return (
    <Card size="sm">
      <CardContent className="flex flex-col gap-2 px-4 py-3 sm:flex-row sm:items-center">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="truncate text-sm font-medium">{s.name}</span>
            {!s.enabled && <Badge variant="outline">paused</Badge>}
          </div>
          <div className="text-[12px] text-muted-foreground">
            <span className="font-mono">{s.workflow}</span> in{" "}
            <span className="font-mono">{s.project}</span> ·{" "}
            {words ?? <code className="font-mono">{s.cron}</code>} ·{" "}
            {s.timezone}
          </div>
          <div className="text-[11px] text-muted-foreground">
            Next: {s.enabled ? formatWhen(s.next_run_at) : "paused"}
            {s.last_run_id && (
              <>
                {" · Last: "}
                <Link to={`/runs/${s.last_run_id}`} className="underline">
                  {formatWhen(s.last_run_at)}
                </Link>
              </>
            )}
          </div>
          {(run.isError || remove.isError) && (
            <div className="text-[11px] text-destructive">
              {(run.error ?? remove.error)?.message}
            </div>
          )}
        </div>
        {admin && (
          <div className="flex shrink-0 gap-1">
            <Button
              size="sm"
              variant="outline"
              disabled={run.isPending}
              onClick={() => run.mutate(s.id)}
              title="Start a run now; the schedule itself is unchanged"
            >
              <IconPlayerPlay />
              Run now
            </Button>
            <Button
              size="icon-sm"
              variant="ghost"
              aria-label="Edit"
              onClick={onEdit}
            >
              <IconPencil />
            </Button>
            <Button
              size="icon-sm"
              variant="ghost"
              aria-label="Delete"
              disabled={remove.isPending}
              onClick={() => {
                if (window.confirm(`Delete the schedule “${s.name}”?`)) {
                  remove.mutate(s.id);
                }
              }}
            >
              <IconTrash />
            </Button>
          </div>
        )}
        {run.data && (
          <Link
            to={`/runs/${run.data.run_id}`}
            className="text-[11px] underline sm:ml-2"
          >
            Started — open the run
          </Link>
        )}
      </CardContent>
    </Card>
  );
}

function ScheduleDialog({
  schedule,
  onClose,
}: {
  schedule: Schedule | null;
  onClose: () => void;
}) {
  const projects = useProjects();
  const workflows = useWorkflowList();
  const save = useSaveSchedule();
  const [form, setForm] = useState<ScheduleInput>(() => ({
    name: schedule?.name ?? "",
    project: schedule?.project ?? "",
    workflow: schedule?.workflow ?? "",
    cron: schedule?.cron ?? PRESETS[0].cron,
    timezone: schedule?.timezone ?? browserTimezone(),
    task: schedule?.task ?? "",
    enabled: schedule?.enabled ?? true,
  }));
  const set = <K extends keyof ScheduleInput>(k: K, v: ScheduleInput[K]) =>
    setForm((f) => ({ ...f, [k]: v }));
  const preset = describeCron(form.cron) ? form.cron.trim() : CUSTOM;
  const [custom, setCustom] = useState(preset === CUSTOM);
  const preview = useSchedulePreview(form.cron, form.timezone);

  // A single project is the obvious choice.
  useEffect(() => {
    const list = projects.data ?? [];
    if (!form.project && list.length === 1) set("project", list[0].name);
  }, [projects.data, form.project]);

  const canSave =
    form.name.trim() &&
    form.project &&
    form.workflow &&
    preview.isSuccess &&
    !save.isPending;

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>
            {schedule ? "Edit schedule" : "New schedule"}
          </DialogTitle>
          <DialogDescription>
            Runs the workflow at these times, as if someone started it with the
            task below.
          </DialogDescription>
        </DialogHeader>

        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (!canSave) return;
            save.mutate(
              { id: schedule?.id, input: form },
              { onSuccess: onClose },
            );
          }}
        >
          <Field label="Name">
            <Input
              value={form.name}
              onChange={(e) => set("name", e.target.value)}
              placeholder="Weekly INP report"
              autoFocus
            />
          </Field>

          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Workflow">
              <Select
                value={form.workflow || null}
                onValueChange={(v) => v != null && set("workflow", v)}
              >
                <SelectTrigger className="w-full">
                  <SelectValue>
                    {(v: string | null) => v || "Choose a workflow"}
                  </SelectValue>
                </SelectTrigger>
                <SelectContent>
                  {(workflows.data ?? []).map((w) => (
                    <SelectItem key={w.name} value={w.name}>
                      {w.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
            <Field label="Project">
              <Select
                value={form.project || null}
                onValueChange={(v) => v != null && set("project", v)}
              >
                <SelectTrigger className="w-full">
                  <SelectValue>
                    {(v: string | null) => v || "Choose a project"}
                  </SelectValue>
                </SelectTrigger>
                <SelectContent>
                  {(projects.data ?? []).map((p) => (
                    <SelectItem key={p.name} value={p.name}>
                      {p.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
          </div>

          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="When">
              <Select
                value={custom ? CUSTOM : preset}
                onValueChange={(v) => {
                  if (v == null) return;
                  if (v === CUSTOM) {
                    setCustom(true);
                  } else {
                    setCustom(false);
                    set("cron", v);
                  }
                }}
              >
                <SelectTrigger className="w-full">
                  <SelectValue>
                    {(v: string | null) =>
                      v === CUSTOM || !v
                        ? "Custom (cron)"
                        : (describeCron(v) ?? v)
                    }
                  </SelectValue>
                </SelectTrigger>
                <SelectContent>
                  {PRESETS.map((p) => (
                    <SelectItem key={p.cron} value={p.cron}>
                      {p.label}
                    </SelectItem>
                  ))}
                  <SelectItem value={CUSTOM}>Custom (cron)</SelectItem>
                </SelectContent>
              </Select>
            </Field>
            <Field label="Timezone">
              <Input
                value={form.timezone}
                onChange={(e) => set("timezone", e.target.value)}
                placeholder="Europe/Copenhagen"
              />
            </Field>
          </div>

          {custom && (
            <Field
              label="Cron expression"
              hint="minute hour day-of-month month day-of-week, e.g. 0 8 * * 1"
            >
              <Input
                value={form.cron}
                onChange={(e) => set("cron", e.target.value)}
                className="font-mono"
              />
            </Field>
          )}

          <div className="rounded-md border border-border px-3 py-2 text-[12px]">
            {preview.isError ? (
              <span className="text-destructive">{preview.error.message}</span>
            ) : preview.data ? (
              <span className="text-muted-foreground">
                Next runs:{" "}
                {preview.data.next
                  .slice(0, 3)
                  .map((t) => formatWhen(t))
                  .join(" · ")}
              </span>
            ) : (
              <span className="text-muted-foreground">
                Enter a five-field cron expression.
              </span>
            )}
          </div>

          <Field
            label="Task"
            hint="What the run is asked to do. The workflow receives it as its task text."
          >
            <Textarea
              value={form.task}
              onChange={(e) => set("task", e.target.value)}
              placeholder="Report on last week's INP from Grafana, by page type, compared with the four weeks before."
              className="min-h-24"
            />
          </Field>

          <label className="flex items-center gap-2 text-sm">
            <Switch
              checked={form.enabled}
              onCheckedChange={(v) => set("enabled", v)}
            />
            Enabled
          </label>

          {save.isError && (
            <p className="text-[12px] text-destructive">{save.error.message}</p>
          )}

          <DialogFooter>
            <Button type="button" variant="ghost" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" disabled={!canSave}>
              <IconClockPlay />
              {save.isPending ? "Saving…" : "Save schedule"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1">
      <span className="text-[11px] font-medium text-muted-foreground">
        {label}
      </span>
      {children}
      {hint && (
        <span className="text-[11px] text-muted-foreground">{hint}</span>
      )}
    </div>
  );
}
