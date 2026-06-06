import { useEffect, useRef, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { useProjects } from "@/lib/projects";
import {
  useDeleteLinearSource,
  useLinearDiscovery,
  useLinearSource,
  useSaveLinearSource,
} from "@/lib/linear";
import type { LinearState, LinearTeam } from "@/types/linear";

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

export function LinearTriggerPanel({ workflow }: { workflow: string }) {
  const projects = useProjects();
  const [project, setProject] = useState<string>("");
  const discovery = useLinearDiscovery(project || null);
  const source = useLinearSource(project || null, workflow || null);
  const save = useSaveLinearSource(project || null);
  const del = useDeleteLinearSource(project || null);

  // Form state.
  const [teamId, setTeamId] = useState("");
  const [sourceStateId, setSourceStateId] = useState("");
  const [label, setLabel] = useState("");
  const [inProgressStateId, setInProgressStateId] = useState("");
  const [reviewStateId, setReviewStateId] = useState("");
  const [readyStateId, setReadyStateId] = useState("");
  const [baseBranch, setBaseBranch] = useState("");
  const [pollIntervalSecs, setPollIntervalSecs] = useState(60);
  const [enabled, setEnabled] = useState(false);
  const [live, setLive] = useState(false);

  const loadedFor = useRef<string | null>(null);
  const sourceKey = `${project}:${workflow}`;

  const resetForm = () => {
    setTeamId("");
    setSourceStateId("");
    setLabel("");
    setInProgressStateId("");
    setReviewStateId("");
    setReadyStateId("");
    setBaseBranch("");
    setPollIntervalSecs(60);
    setEnabled(false);
    setLive(false);
  };

  // Seed form state once when source data arrives (or reset when none exists).
  useEffect(() => {
    if (!project || !workflow) {
      resetForm();
      loadedFor.current = null;
      return;
    }

    if (loadedFor.current === sourceKey) return;

    if (source.data === undefined) {
      if (loadedFor.current !== null) {
        resetForm();
        loadedFor.current = null;
      }
      return;
    }

    if (source.data === null) {
      resetForm();
      loadedFor.current = sourceKey;
      return;
    }

    const s = source.data;
    setTeamId(s.team_id);
    setSourceStateId(s.source_state_id);
    setLabel(s.label ?? "");
    setInProgressStateId(s.in_progress_state_id ?? "");
    setReviewStateId(s.review_state_id ?? "");
    setReadyStateId(s.ready_state_id ?? "");
    setBaseBranch(s.base_branch ?? "");
    setPollIntervalSecs(s.poll_interval_secs);
    setEnabled(s.enabled);
    setLive(s.live);
    loadedFor.current = sourceKey;
  }, [source.data, project, workflow, sourceKey]);
  // Clear stale mutation state when the context changes.
  useEffect(() => {
    save.reset();
    del.reset();
  }, [project, workflow]);

  // Default project to first available project.
  useEffect(() => {
    if (!project && projects.data && projects.data.length > 0) {
      setProject(projects.data[0].name);
    }
  }, [projects.data, project]);

  const selectedTeam: LinearTeam | undefined = discovery.data?.teams.find(
    (t) => t.id === teamId,
  );

  const teamName = selectedTeam?.name ?? "";
  const states = selectedTeam?.states ?? [];
  const labels = selectedTeam?.labels ?? [];
  const sortedStates = [...states].sort((a, b) => a.position - b.position);
  const sourceLoaded = loadedFor.current === sourceKey;

  const canSave =
    sourceLoaded &&
    !!project &&
    !!teamId &&
    !!teamName &&
    !!sourceStateId &&
    !save.isPending;

  const handleSave = () => {
    if (!canSave) return;
    save.mutate({
      workflow,
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
    });
  };

  const handleDelete = () => {
    if (!source.data || del.isPending) return;
    del.mutate(workflow, {
      onSuccess: () => {
        loadedFor.current = null;
      },
    });
  };

  const isMissingCredential = discovery.isError;
  const hasNoTeams = !discovery.data || discovery.data.teams.length === 0;
  const hasNoProjects = !projects.data || projects.data.length === 0;

  const projectSelect = (
    <Field label="Project">
      <select
        className={inputCls}
        value={project}
        onChange={(e) => setProject(e.target.value)}
      >
        {(projects.data ?? []).map((p) => (
          <option key={p.name} value={p.name}>
            {p.name}
          </option>
        ))}
      </select>
    </Field>
  );

  return (
    <Collapsible
      defaultOpen={false}
      className="flex-none border-b border-border"
    >
      <CollapsibleTrigger className="flex w-full items-center justify-between px-4 py-2 text-sm hover:bg-muted/50">
        <div className="flex items-center gap-2">
          <span className="font-medium">Linear trigger</span>
          {enabled && source.data && (
            <Badge variant="success" className="text-[10px]">
              enabled
            </Badge>
          )}
          {save.isSuccess && (
            <span className="text-[11px] text-muted-foreground">Saved</span>
          )}
        </div>
        <span className="text-muted-foreground">▼</span>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="flex flex-col gap-3 px-4 py-3">
          {hasNoProjects ? (
            <div className="text-xs text-muted-foreground">
              Register a project first to configure a Linear trigger.
            </div>
          ) : isMissingCredential || hasNoTeams ? (
            <>
              {projectSelect}
              <div className="text-xs text-muted-foreground">
                No Linear key for this project. Set a Linear API key on the{" "}
                <span className="font-medium">Projects</span> page (or a global
                key in Settings → Credentials) to load teams.
              </div>
            </>
          ) : (
            <>
              {projectSelect}

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
                  {(discovery.data?.teams ?? []).map((t) => (
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
                  {save.isPending
                    ? "Saving…"
                    : save.isSuccess
                      ? "Saved"
                      : "Save"}
                </Button>
                {source.data && (
                  <Button
                    size="sm"
                    variant="destructive"
                    onClick={handleDelete}
                    disabled={del.isPending}
                  >
                    {del.isPending ? "Deleting…" : "Delete"}
                  </Button>
                )}
                {(save.isError || del.isError) && (
                  <span className="text-xs text-destructive">
                    {save.error?.message ?? del.error?.message}
                  </span>
                )}
              </div>
            </>
          )}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}
