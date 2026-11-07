import { useEffect, useState } from "react";
import { IconClockPlay } from "@tabler/icons-react";
import { Button } from "@/components/ui/button";
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
import { useWorkflowList } from "@/lib/authoring";
import { useProjects } from "@/lib/projects";
import {
  type Schedule,
  type ScheduleInput,
  PRESETS,
  browserTimezone,
  describeCron,
  useSaveSchedule,
  useSchedulePreview,
} from "@/lib/schedules";

/** Base UI's Select needs a concrete value for "write my own". */
const CUSTOM = "__custom__";

export function formatWhen(iso: string | null): string {
  if (!iso) return "—";
  return new Date(iso).toLocaleString(undefined, {
    weekday: "short",
    day: "numeric",
    month: "short",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/**
 * A schedule trigger's settings. Blank = a new one; `readOnly` for anyone who
 * isn't an administrator, since only they may change schedules.
 */
export function ScheduleForm({
  schedule,
  initialProject,
  readOnly,
  onSaved,
  onCancel,
}: {
  schedule: Schedule | null;
  /** Prefill for a new schedule, e.g. from the list's project filter. */
  initialProject?: string;
  readOnly: boolean;
  onSaved: (saved: Schedule) => void;
  onCancel: () => void;
}) {
  const projects = useProjects();
  const workflows = useWorkflowList();
  const save = useSaveSchedule();
  const [form, setForm] = useState<ScheduleInput>(() => ({
    name: schedule?.name ?? "",
    project: schedule?.project ?? initialProject ?? "",
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
    !readOnly &&
    form.name.trim() &&
    form.project &&
    form.workflow &&
    preview.isSuccess &&
    !save.isPending;

  return (
    <form
      className="flex flex-col gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        if (!canSave) return;
        save.mutate({ id: schedule?.id, input: form }, { onSuccess: onSaved });
      }}
    >
      {/* One switch for every control, so a read-only view can't half-edit. */}
      <fieldset disabled={readOnly} className="flex flex-col gap-4">
        <Field label="Name" hint="Also titles each run, with the date.">
          <Input
            value={form.name}
            onChange={(e) => set("name", e.target.value)}
            placeholder="Weekly INP report"
            autoFocus={!schedule}
          />
        </Field>

        <div className="grid gap-4 sm:grid-cols-2">
          <Field label="Workflow">
            <Select
              value={form.workflow || null}
              onValueChange={(v) => v != null && set("workflow", v)}
              disabled={readOnly}
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
              disabled={readOnly}
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

        <div className="grid gap-4 sm:grid-cols-2">
          <Field label="When">
            <Select
              value={custom ? CUSTOM : preset}
              disabled={readOnly}
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
            disabled={readOnly}
          />
          Enabled
        </label>
      </fieldset>

      {save.isError && (
        <p className="text-[12px] text-destructive">{save.error.message}</p>
      )}

      {!readOnly && (
        <div className="flex items-center gap-2">
          <Button type="submit" size="sm" disabled={!canSave}>
            <IconClockPlay />
            {save.isPending ? "Saving…" : "Save schedule"}
          </Button>
          <Button type="button" size="sm" variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
        </div>
      )}
    </form>
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
