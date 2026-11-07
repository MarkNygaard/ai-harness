import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import {
  AccountSelect,
  LinearProjectField,
} from "@/components/projects/ProjectLinear";
import { useLinearConnections, useLinearDiscovery } from "@/lib/linear";
import { useGeneralSettings } from "@/lib/settings";
import {
  type LinearTriggerConfig,
  type RunTrigger,
  useSaveTrigger,
} from "@/lib/triggers";
import { Field, OptionSelect, ProjectSelect, WorkflowSelect } from "./fields";

type LinearTrigger = Extract<RunTrigger, { kind: "linear" }>;

/**
 * A Linear trigger's settings: which issues start which workflow.
 *
 * Deliberately no status map. The trigger starts the run; moving the issue
 * through In Progress, Review and so on is the workflow's job, with `linear:`
 * steps, so it lives next to the work that justifies each move.
 */
export function LinearTriggerForm({
  trigger,
  initialProject,
  onSaved,
  onCancel,
}: {
  trigger?: LinearTrigger;
  initialProject?: string;
  onSaved: (saved: RunTrigger) => void;
  onCancel: () => void;
}) {
  const c = trigger?.config;
  const [name, setName] = useState(trigger?.name ?? "");
  const [project, setProject] = useState(
    trigger?.project ?? initialProject ?? "",
  );
  const [workflow, setWorkflow] = useState(trigger?.workflow ?? "");
  const [teamId, setTeamId] = useState(c?.team_id ?? "");
  const [stateId, setStateId] = useState(c?.state_id ?? "");
  const [label, setLabel] = useState(c?.label ?? "");
  const [linearProject, setLinearProject] = useState<{
    id: string;
    name: string;
  } | null>(
    c?.linear_project_id
      ? {
          id: c.linear_project_id,
          name: c.linear_project_name ?? c.linear_project_id,
        }
      : null,
  );
  const [baseBranch, setBaseBranch] = useState(c?.base_branch ?? "");
  const [maxRuns, setMaxRuns] = useState(c?.max_concurrent_runs ?? 1);
  const [enabled, setEnabled] = useState(trigger?.enabled ?? true);

  const connections = useLinearConnections();
  const discovery = useLinearDiscovery(project || null);
  const save = useSaveTrigger();
  // Off by default (Settings → Integrations → Linear). A trigger that already
  // names a Linear project shows the field regardless, so it can be cleared.
  const settings = useGeneralSettings(true);
  const showLinearProject =
    !!settings.data?.linear_project_bindings || !!c?.linear_project_id;

  const teams = discovery.data?.teams ?? [];
  const team = teams.find((t) => t.id === teamId);
  const states = [...(team?.states ?? [])].sort(
    (a, b) => a.position - b.position,
  );
  const state = states.find((s) => s.id === stateId);
  // A label that is no longer on the team stays choosable, so opening an old
  // trigger doesn't silently drop it.
  const labels = (team?.labels ?? []).map((l) => l.name);
  if (label && !labels.includes(label)) labels.unshift(label);

  const canSave =
    !!project &&
    !!workflow &&
    !!team &&
    !!state &&
    maxRuns >= 1 &&
    maxRuns <= 20 &&
    !save.isPending;

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!canSave || !team || !state) return;
    const config: LinearTriggerConfig = {
      team_id: team.id,
      team_name: team.name,
      state_id: state.id,
      state_name: state.name,
      linear_project_id: linearProject?.id ?? null,
      linear_project_name: linearProject?.name ?? null,
      label: label || null,
      base_branch: baseBranch.trim() || null,
      max_concurrent_runs: maxRuns,
    };
    save.mutate(
      {
        id: trigger?.id,
        body: {
          name: name.trim() || `${team.name} · ${state.name}`,
          kind: "linear",
          project,
          workflow,
          enabled,
          config,
        },
      },
      { onSuccess: (res) => onSaved(res.trigger) },
    );
  };

  return (
    <form className="flex flex-col gap-4" onSubmit={submit}>
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Project">
          <ProjectSelect
            value={project}
            onChange={(p) => {
              setProject(p);
              // Teams belong to the project's Linear account.
              setTeamId("");
              setStateId("");
              setLabel("");
              setLinearProject(null);
            }}
          />
        </Field>
        <Field label="Workflow">
          <WorkflowSelect value={workflow} onChange={setWorkflow} />
        </Field>
      </div>

      {project && (
        <AccountSelect project={project} accounts={connections.data ?? []} />
      )}

      {project && discovery.isError && (
        <p className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive">
          {discovery.error.message}
        </p>
      )}

      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Team">
          <OptionSelect
            value={teamId}
            onChange={(t) => {
              setTeamId(t);
              setStateId("");
              setLabel("");
              setLinearProject(null);
            }}
            disabled={!project || !discovery.data}
            placeholder={
              project && discovery.isLoading ? "Loading…" : "Choose a team"
            }
            options={teams.map((t) => ({
              value: t.id,
              label: `${t.name} (${t.key})`,
            }))}
          />
        </Field>
        <Field
          label="When an issue enters"
          hint="Fires once each time an issue comes into this status. To run it again, move the issue out and back."
        >
          <OptionSelect
            value={stateId}
            onChange={setStateId}
            disabled={!team}
            placeholder="Choose a status"
            options={states.map((s) => ({ value: s.id, label: s.name }))}
          />
        </Field>
      </div>

      {showLinearProject && teamId && (
        <LinearProjectField
          project={project}
          teamId={teamId}
          value={linearProject}
          onChange={setLinearProject}
        />
      )}

      <Field
        label="Also start on label"
        hint={
          label
            ? `Besides issues delegated to the harness, issues in the status carrying ${label} start it too, unless they already have a pull request.`
            : "Only issues delegated to the harness start it. Pick a label to also start issues nobody delegated — for instance ones a triage workflow labels."
        }
      >
        <OptionSelect
          value={label}
          onChange={setLabel}
          disabled={!team}
          none="None — delegation only"
          placeholder="None — delegation only"
          options={labels.map((l) => ({ value: l, label: l }))}
        />
      </Field>

      <div className="grid gap-4 sm:grid-cols-3">
        <Field label="Name" hint="Defaults to the team and status.">
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={team && state ? `${team.name} · ${state.name}` : ""}
          />
        </Field>
        <Field label="Base branch" hint="Empty: the project's own.">
          <Input
            value={baseBranch}
            onChange={(e) => setBaseBranch(e.target.value)}
            placeholder="main"
            className="font-mono"
          />
        </Field>
        <Field label="Runs at once">
          <Input
            type="number"
            min={1}
            max={20}
            value={maxRuns}
            onChange={(e) =>
              setMaxRuns(
                Math.min(20, Math.max(1, parseInt(e.target.value, 10) || 1)),
              )
            }
          />
        </Field>
      </div>

      <p className="rounded-md border border-border bg-muted/30 px-3 py-2 text-[12px] text-muted-foreground">
        This trigger only starts the run. To move the issue — to In Progress
        when work begins, to In Review once the pull request is open — give the
        workflow <code className="font-mono">linear:</code> steps. To label it
        when the run fails, add a last step with{" "}
        <code className="font-mono">trigger_rule: one_failed</code>.
      </p>

      <label className="flex items-center gap-2 text-sm">
        <Switch checked={enabled} onCheckedChange={setEnabled} />
        Enabled
      </label>

      {save.isError && (
        <p className="text-[12px] text-destructive">{save.error.message}</p>
      )}
      <div className="flex items-center gap-2">
        <Button type="submit" size="sm" disabled={!canSave}>
          {save.isPending ? "Saving…" : trigger ? "Save" : "Create trigger"}
        </Button>
        <Button type="button" size="sm" variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </form>
  );
}
