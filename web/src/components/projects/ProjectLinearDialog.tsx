import { useEffect, useState } from "react";
import { IconBolt, IconPlus, IconTrash } from "@tabler/icons-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { useProjectWorkflows } from "@/lib/authoring";
import { useProjectCredentials } from "@/lib/credentials";
import {
  useDeleteLinearSource,
  useLinearDiscovery,
  useLinearSources,
  useSaveLinearSource,
} from "@/lib/linear";
import type { LinearSource, LinearState, LinearTeam } from "@/types/linear";
import type { WorkflowSummary } from "@/types/authoring";

const inputCls =
  "h-8 rounded-md border border-input bg-transparent px-2 text-[13px] outline-none focus:ring-2 focus:ring-ring";

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[11px] font-medium text-muted-foreground">
        {label}
      </span>
      {children}
    </label>
  );
}

function StateSelect({
  label,
  value,
  onChange,
  options,
  disabled,
  placeholder = "(none)",
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  options: LinearState[];
  disabled?: boolean;
  placeholder?: string;
}) {
  return (
    <Field label={label}>
      <select
        className={inputCls}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        disabled={disabled}
      >
        <option value="">{placeholder}</option>
        {options.map((s) => (
          <option key={s.id} value={s.id}>
            {s.name}
          </option>
        ))}
      </select>
    </Field>
  );
}

/**
 * A per-project "Linear" dialog: lists existing Linear trigger bindings (one
 * per workflow) and lets the user add/edit/delete them. Renders nothing unless
 * the project has a Linear credential configured.
 */
export function ProjectLinearDialog({ project }: { project: string }) {
  const creds = useProjectCredentials(project);
  const hasLinearKey =
    creds.data?.some((c) => c.provider === "linear" && c.configured) ?? false;

  if (!hasLinearKey) return null;

  return (
    <Dialog>
      <DialogTrigger
        render={<Button variant="ghost" size="sm" title="Linear triggers" />}
      >
        <IconBolt className="size-3.5" /> Linear
      </DialogTrigger>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="font-mono text-base">{project}</DialogTitle>
          <DialogDescription>
            Linear trigger bindings for this project. Each binding watches a team
            and fires a workflow for matching issues. One binding per workflow.
          </DialogDescription>
        </DialogHeader>
        <DialogBody project={project} />
      </DialogContent>
    </Dialog>
  );
}

function DialogBody({ project }: { project: string }) {
  const discovery = useLinearDiscovery(project);
  const sources = useLinearSources(project);
  const workflows = useProjectWorkflows(project);
  const del = useDeleteLinearSource(project);

  // `null` = closed, "" = new binding, otherwise the workflow being edited.
  const [editing, setEditing] = useState<string | null>(null);

  const teams = discovery.data?.teams ?? [];
  const bound = new Set((sources.data ?? []).map((s) => s.workflow));
  const availableWorkflows = (workflows.data ?? []).filter(
    (w) => !bound.has(w.name),
  );

  const resolveTeam = (id: string): LinearTeam | undefined =>
    teams.find((t) => t.id === id);
  const resolveStateName = (teamId: string, stateId: string): string => {
    const team = resolveTeam(teamId);
    return team?.states.find((s) => s.id === stateId)?.name ?? stateId;
  };

  if (discovery.isError) {
    return (
      <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive">
        {discovery.error.message}
      </div>
    );
  }

  if (editing !== null) {
    const source =
      editing === ""
        ? undefined
        : (sources.data ?? []).find((s) => s.workflow === editing);
    return (
      <BindingForm
        project={project}
        workflow={editing}
        fixedWorkflow={editing !== ""}
        source={source}
        availableWorkflows={editing === "" ? availableWorkflows : undefined}
        teams={teams}
        onDone={() => setEditing(null)}
      />
    );
  }

  return (
    <div className="flex flex-col gap-3">
      {sources.isLoading && (
        <p className="text-xs text-muted-foreground">Loading…</p>
      )}
      {sources.data?.length === 0 && (
        <p className="text-xs text-muted-foreground">
          No bindings yet. Add one below.
        </p>
      )}
      {sources.data && sources.data.length > 0 && (
        <div className="flex flex-col gap-2">
          {sources.data.map((s) => (
            <div
              key={s.workflow}
              className="flex items-center gap-2 rounded-md border border-border px-3 py-2"
            >
              <div className="min-w-0 flex-1 flex flex-col gap-1">
                <div className="flex items-center gap-2">
                  <span className="truncate font-mono text-[13px] font-medium">
                    {s.workflow}
                  </span>
                  <Badge
                    variant={s.enabled ? "success" : "outline"}
                    className="text-[10px]"
                  >
                    {s.enabled ? "enabled" : "disabled"}
                  </Badge>
                  <Badge
                    variant={s.live ? "success" : "outline"}
                    className="text-[10px]"
                  >
                    {s.live ? "live" : "dry-run"}
                  </Badge>
                </div>
                <span className="truncate text-[11px] text-muted-foreground">
                  {s.team_name} · {resolveStateName(s.team_id, s.source_state_id)}
                </span>
              </div>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setEditing(s.workflow)}
              >
                Edit
              </Button>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => del.mutate(s.workflow)}
                disabled={del.isPending}
                title="Delete binding"
              >
                <IconTrash className="size-3.5" />
              </Button>
            </div>
          ))}
        </div>
      )}
      {del.isError && (
        <span className="text-[10px] text-destructive">
          {del.error.message}
        </span>
      )}
      <Button
        size="sm"
        variant="outline"
        onClick={() => setEditing("")}
        disabled={availableWorkflows.length === 0}
        title={
          availableWorkflows.length === 0
            ? "All workflows already have a binding"
            : "Add a Linear trigger binding"
        }
      >
        <IconPlus className="size-4" /> Add binding
      </Button>
    </div>
  );
}

function BindingForm({
  project,
  workflow,
  fixedWorkflow,
  source,
  availableWorkflows,
  teams,
  onDone,
}: {
  project: string;
  workflow: string;
  fixedWorkflow: boolean;
  source?: LinearSource;
  availableWorkflows?: WorkflowSummary[];
  teams: LinearTeam[];
  onDone: () => void;
}) {
  const save = useSaveLinearSource(project);

  const [workflowName, setWorkflowName] = useState(fixedWorkflow ? workflow : "");
  const [teamId, setTeamId] = useState(source?.team_id ?? "");
  const [sourceStateId, setSourceStateId] = useState(
    source?.source_state_id ?? "",
  );
  const [label, setLabel] = useState(source?.label ?? "");
  const [inProgressStateId, setInProgressStateId] = useState(
    source?.in_progress_state_id ?? "",
  );
  const [reviewStateId, setReviewStateId] = useState(
    source?.review_state_id ?? "",
  );
  const [readyStateId, setReadyStateId] = useState(source?.ready_state_id ?? "");
  const [baseBranch, setBaseBranch] = useState(source?.base_branch ?? "");
  const [pollIntervalSecs, setPollIntervalSecs] = useState(
    source?.poll_interval_secs ?? 60,
  );
  const [enabled, setEnabled] = useState(source?.enabled ?? false);
  const [live, setLive] = useState(source?.live ?? false);

  // Clear stale mutation state when the form context changes.
  useEffect(() => {
    save.reset();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workflow]);

  const selectedTeam = teams.find((t) => t.id === teamId);
  const teamName = selectedTeam?.name ?? "";
  const states = selectedTeam?.states ?? [];
  const labels = selectedTeam?.labels ?? [];
  const sortedStates = [...states].sort((a, b) => a.position - b.position);

  const canSave =
    !!project &&
    !!workflowName &&
    !!teamId &&
    !!teamName &&
    !!sourceStateId &&
    !save.isPending;

  const handleSave = () => {
    if (!canSave) return;
    save.mutate(
      {
        workflow: workflowName,
        team_id: teamId,
        team_name: teamName,
        source_state_id: sourceStateId,
        label: label.trim() || undefined,
        in_progress_state_id: inProgressStateId.trim() || undefined,
        review_state_id: reviewStateId.trim() || undefined,
        ready_state_id: readyStateId.trim() || undefined,
        base_branch: baseBranch.trim() || undefined,
        poll_interval_secs: pollIntervalSecs,
        enabled,
        live,
      },
      { onSuccess: () => onDone() },
    );
  };

  return (
    <div className="flex flex-col gap-3">
      {/* Workflow */}
      {fixedWorkflow ? (
        <Field label="Workflow">
          <div className="flex h-8 items-center font-mono text-[13px]">
            {workflow}
          </div>
        </Field>
      ) : (
        <Field label="Workflow">
          <select
            className={inputCls}
            value={workflowName}
            onChange={(e) => setWorkflowName(e.target.value)}
          >
            <option value="">Select a workflow…</option>
            {(availableWorkflows ?? []).map((w) => (
              <option key={w.name} value={w.name}>
                {w.name}
              </option>
            ))}
          </select>
        </Field>
      )}

      {/* Team */}
      <Field label="Team">
        <select
          className={inputCls}
          value={teamId}
          onChange={(e) => {
            setTeamId(e.target.value);
            setSourceStateId("");
            setLabel("");
            setInProgressStateId("");
            setReviewStateId("");
            setReadyStateId("");
          }}
        >
          <option value="">Select a team…</option>
          {teams.map((t) => (
            <option key={t.id} value={t.id}>
              {t.name} ({t.key})
            </option>
          ))}
        </select>
      </Field>

      {/* Source state */}
      <StateSelect
        label="Source status"
        value={sourceStateId}
        onChange={setSourceStateId}
        options={sortedStates}
        disabled={!teamId}
        placeholder="Select a status…"
      />

      {/* Eligibility label */}
      <Field label="Eligibility label">
        <select
          className={inputCls}
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          disabled={!teamId}
        >
          <option value="">(none)</option>
          {labels.map((l) => (
            <option key={l.id} value={l.name}>
              {l.name}
            </option>
          ))}
        </select>
      </Field>

      {/* Status map */}
      <div className="grid grid-cols-3 gap-2">
        <StateSelect
          label="In-progress"
          value={inProgressStateId}
          onChange={setInProgressStateId}
          options={sortedStates}
          disabled={!teamId}
        />
        <StateSelect
          label="Review"
          value={reviewStateId}
          onChange={setReviewStateId}
          options={sortedStates}
          disabled={!teamId}
        />
        <StateSelect
          label="Ready"
          value={readyStateId}
          onChange={setReadyStateId}
          options={sortedStates}
          disabled={!teamId}
        />
      </div>

      {/* Base branch & poll interval */}
      <div className="grid grid-cols-2 gap-2">
        <Field label="Base branch">
          <input
            className={inputCls}
            value={baseBranch}
            onChange={(e) => setBaseBranch(e.target.value)}
            placeholder="main"
          />
        </Field>
        <Field label="Poll interval (seconds)">
          <input
            className={inputCls}
            type="number"
            min={1}
            max={86400}
            value={pollIntervalSecs}
            onChange={(e) =>
              setPollIntervalSecs(
                Math.min(
                  86400,
                  Math.max(1, parseInt(e.target.value, 10) || 1),
                ),
              )
            }
          />
        </Field>
      </div>

      {/* Enabled toggle — on/off without deleting the binding. */}
      <label className="flex items-center gap-2 text-[13px]">
        <input
          type="checkbox"
          checked={enabled}
          onChange={(e) => setEnabled(e.target.checked)}
          className="h-4 w-4 rounded border-border"
        />
        <span>Enabled</span>
      </label>

      {/* Live toggle — off = dry-run (logs only); on = claim + fire. */}
      <label className="flex items-center gap-2 text-[13px]">
        <input
          type="checkbox"
          checked={live}
          onChange={(e) => setLive(e.target.checked)}
          disabled={!enabled}
          className="h-4 w-4 rounded border-border"
        />
        <span>
          Live{" "}
          <span className="text-muted-foreground">
            (off = dry-run: logs candidates without claiming or firing)
          </span>
        </span>
      </label>

      {/* Actions */}
      <div className="flex items-center gap-2 pt-1">
        <Button size="sm" onClick={handleSave} disabled={!canSave}>
          {save.isPending ? "Saving…" : "Save"}
        </Button>
        <Button size="sm" variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        {save.isError && (
          <span className="text-xs text-destructive">{save.error.message}</span>
        )}
      </div>
    </div>
  );
}
