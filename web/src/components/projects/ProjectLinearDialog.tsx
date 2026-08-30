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
import { useWorkflowList } from "@/lib/authoring";
import {
  useDeleteLinearSource,
  useLinearConnections,
  useLinearDiscovery,
  useLinearSources,
  useSaveLinearSource,
  useSetProjectLinearConnection,
} from "@/lib/linear";
import { connectionName } from "@/types/linear";
import type {
  LinearConnection,
  LinearSource,
  LinearState,
  LinearTeam,
} from "@/types/linear";
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
  help,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  options: LinearState[];
  disabled?: boolean;
  placeholder?: string;
  /** Shown under the control, for a status whose purpose is not obvious. */
  help?: React.ReactNode;
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
      {help && (
        <span className="text-[10px] text-muted-foreground">{help}</span>
      )}
    </Field>
  );
}

/**
 * A per-project "Linear" dialog: lists existing Linear trigger bindings (one
 * per workflow) and lets the user add/edit/delete them. Renders nothing unless
 * the project has a Linear credential configured.
 */
export function ProjectLinearDialog({ project }: { project: string }) {
  // Gate on an account that can actually authenticate (an app install or a
  // legacy key) — one holding only OAuth client details isn't one yet. Any
  // usable account is enough to open this: which one this project uses is
  // chosen inside.
  const connections = useLinearConnections();
  const accounts = connections.data ?? [];
  if (!accounts.some((c) => c.mode !== "none")) return null;

  return (
    <Dialog>
      <DialogTrigger
        render={
          <Button variant="ghost" size="icon-sm" title="Linear triggers" />
        }
      >
        <IconBolt className="size-3.5" />
      </DialogTrigger>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="font-mono text-base">{project}</DialogTitle>
          <DialogDescription>
            Maps a Linear team to this project: which workflow runs, the source
            status that triggers it, the rest of the status map, and the base
            branch. Work starts only when an issue is both{" "}
            <strong>delegated to the harness in Linear</strong> and sitting in
            that source status. Delegating is the fast path; enabling{" "}
            <em>live</em> also lets the poller catch delegated issues it missed.
            One binding per workflow.
          </DialogDescription>
        </DialogHeader>
        <DialogBody project={project} accounts={accounts} />
      </DialogContent>
    </Dialog>
  );
}

/**
 * Which Linear account this project's issues come from.
 *
 * Hidden while there is only one account — there is nothing to choose, and the
 * harness resolves to it automatically.
 */
function AccountSelect({
  project,
  accounts,
}: {
  project: string;
  accounts: LinearConnection[];
}) {
  const pin = useSetProjectLinearConnection(project);
  if (accounts.length < 2) return null;

  // Which account claims this project. The connections list already carries it,
  // so this needs no separate lookup.
  const current = accounts.find((c) => c.projects.includes(project))?.id ?? "";

  return (
    <div className="flex flex-col gap-1.5 rounded-md border border-border bg-muted/30 p-2">
      <Field label="Linear account">
        <select
          className={inputCls}
          value={current}
          disabled={pin.isPending}
          onChange={(e) => pin.mutate(e.target.value || null)}
        >
          <option value="">(choose an account)</option>
          {accounts.map((c) => (
            <option key={c.id} value={c.id}>
              {connectionName(c)}
            </option>
          ))}
        </select>
      </Field>
      <span className="text-[10px] text-muted-foreground">
        Teams belong to an account, so the bindings below are set up against
        this one. Switching accounts means picking their teams and statuses
        again.
      </span>
      {pin.isError && (
        <span className="text-[10px] text-destructive">
          {pin.error.message}
        </span>
      )}
    </div>
  );
}

function DialogBody({
  project,
  accounts,
}: {
  project: string;
  accounts: LinearConnection[];
}) {
  const discovery = useLinearDiscovery(project);
  const sources = useLinearSources(project);
  const workflows = useWorkflowList();
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
    // A project with several accounts and none chosen lands here: discovery
    // can't say which workspace to read. The selector above is the fix, so it
    // has to render alongside the error rather than instead of it.
    return (
      <div className="flex flex-col gap-2">
        <AccountSelect project={project} accounts={accounts} />
        <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive">
          {discovery.error.message}
        </div>
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
      <AccountSelect project={project} accounts={accounts} />
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
                  {/* Team in the heading, not just the detail line: bindings in
                      one project can watch different teams, and the team is what
                      distinguishes them at a glance. */}
                  <span className="truncate text-[13px] font-medium">
                    <span className="font-mono">{s.workflow}</span>
                    <span className="text-muted-foreground">
                      {" "}
                      ({s.team_name})
                    </span>
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
                {/* Team moved to the heading above, so this is the status it
                    triggers from — the other thing that identifies a binding. */}
                <span className="truncate text-[11px] text-muted-foreground">
                  starts from {resolveStateName(s.team_id, s.source_state_id)}
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

  const [workflowName, setWorkflowName] = useState(
    fixedWorkflow ? workflow : "",
  );
  const [teamId, setTeamId] = useState(source?.team_id ?? "");
  const [sourceStateId, setSourceStateId] = useState(
    source?.source_state_id ?? "",
  );
  const [failedLabel, setFailedLabel] = useState(source?.failed_label ?? "");
  const [inProgressStateId, setInProgressStateId] = useState(
    source?.in_progress_state_id ?? "",
  );
  const [reviewStateId, setReviewStateId] = useState(
    source?.review_state_id ?? "",
  );
  const [readyStateId, setReadyStateId] = useState(
    source?.ready_state_id ?? "",
  );
  const [pieceReadyStateId, setPieceReadyStateId] = useState(
    source?.piece_ready_state_id ?? "",
  );
  const [epicReviewStateId, setEpicReviewStateId] = useState(
    source?.epic_review_state_id ?? "",
  );
  const [baseBranch, setBaseBranch] = useState(source?.base_branch ?? "");
  const [pollIntervalSecs, setPollIntervalSecs] = useState(
    source?.poll_interval_secs ?? 60,
  );
  const [maxConcurrentRuns, setMaxConcurrentRuns] = useState(
    source?.max_concurrent_runs ?? 1,
  );
  const [maxAttempts, setMaxAttempts] = useState(source?.max_attempts ?? 1);
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
        failed_label: failedLabel.trim() || undefined,
        in_progress_state_id: inProgressStateId.trim() || undefined,
        review_state_id: reviewStateId.trim() || undefined,
        ready_state_id: readyStateId.trim() || undefined,
        piece_ready_state_id: pieceReadyStateId.trim() || undefined,
        epic_review_state_id: epicReviewStateId.trim() || undefined,
        base_branch: baseBranch.trim() || undefined,
        poll_interval_secs: pollIntervalSecs,
        max_concurrent_runs: maxConcurrentRuns,
        max_attempts: maxAttempts,
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

      {/* Failed label — applied when the binding gives up (optional). */}
      <Field label="Failed label">
        <select
          className={inputCls}
          value={failedLabel}
          onChange={(e) => setFailedLabel(e.target.value)}
          disabled={!teamId}
        >
          <option value="">(none — feature off)</option>
          {labels.map((l) => (
            <option key={l.id} value={l.name}>
              {l.name}
            </option>
          ))}
        </select>
        <span className="text-[10px] text-muted-foreground">
          Applied after the attempt budget is spent; while set, the issue is
          skipped. Remove it (or hit Rerun) to re-arm for one more try. An issue
          is hard-stopped after 10 total attempts.
        </span>
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

      {/* The exception, on its own row: it overrides one cell of the map above
          and only for a sub-issue of an epic. */}
      <StateSelect
        label="Ready (finished epic)"
        value={epicReviewStateId}
        onChange={setEpicReviewStateId}
        options={sortedStates}
        disabled={!teamId}
        help={
          <>
            Where the <em>epic itself</em> goes once every piece is in and its
            pull request is open — the point a person picks it up. Set this on
            the <code>linear-epic-supervise</code> binding. Leave empty to leave
            the epic where it is.
          </>
        }
      />
      <StateSelect
        label="Ready (epic piece)"
        value={pieceReadyStateId}
        onChange={setPieceReadyStateId}
        options={sortedStates}
        disabled={!teamId}
        help={
          <>
            Where a sub-issue of an epic goes instead of <em>Ready</em>, so
            pieces merge straight into the epic branch while standalone issues
            stop at a human gate. The feature is still reviewed once, when the
            finished epic becomes a single pull request. Leave empty to treat
            both the same.
          </>
        }
      />

      {/* Base branch, poll interval & concurrency cap */}
      <div className="grid grid-cols-3 gap-2">
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
                Math.min(86400, Math.max(1, parseInt(e.target.value, 10) || 1)),
              )
            }
          />
        </Field>
        <Field label="Max simultaneous tasks">
          <input
            className={inputCls}
            type="number"
            min={1}
            max={20}
            value={maxConcurrentRuns}
            onChange={(e) =>
              setMaxConcurrentRuns(
                Math.min(20, Math.max(1, parseInt(e.target.value, 10) || 1)),
              )
            }
          />
        </Field>
      </div>

      {/* Attempt budget — whole-workflow retries before the poller gives up. */}
      <Field label="Max attempts (retries before giving up)">
        <input
          className={inputCls}
          type="number"
          min={1}
          max={10}
          value={maxAttempts}
          onChange={(e) =>
            setMaxAttempts(
              Math.min(10, Math.max(1, parseInt(e.target.value, 10) || 1)),
            )
          }
        />
      </Field>

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
