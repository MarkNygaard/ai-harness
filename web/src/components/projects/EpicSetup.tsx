import { useState } from "react";
import { IconChevronRight } from "@tabler/icons-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  epicBindingWrites,
  epicColumnsProblem,
  isBuilder,
  MERGE_WORKFLOW,
  SUPERVISOR_WORKFLOW,
} from "@/lib/epic-setup";
import { useLinearCheck, useSaveLinearSource } from "@/lib/linear";
import type { LinearSource, LinearState, LinearTeam } from "@/types/linear";

/**
 * The wiring check, under the binding list: whether each binding's columns hand
 * work on to another binding. Problems are shown; what is fine folds away, so a
 * healthy project reads as one line.
 */
export function WiringCheck({ project }: { project: string }) {
  const check = useLinearCheck(project);
  if (check.isLoading) return null;
  if (check.isError) {
    return (
      <p className="text-[11px] text-muted-foreground">
        Could not check the wiring: {check.error.message}
      </p>
    );
  }
  const findings = check.data ?? [];
  const problems = findings.filter((f) => f.level !== "ok");
  const fine = findings.filter((f) => f.level === "ok");

  return (
    <div className="flex flex-col gap-1.5 rounded-md border border-border bg-muted/30 p-2">
      <span className="text-[11px] font-medium text-muted-foreground">
        Wiring
      </span>
      {problems.length === 0 && (
        <span className="text-[12px]">
          Every column hands its work on to another binding.
        </span>
      )}
      {problems.map((f, i) => (
        <div key={i} className="flex items-start gap-2 text-[12px]">
          <Badge
            variant={f.level === "error" ? "destructive" : "outline"}
            className="shrink-0 text-[10px]"
          >
            {f.level}
          </Badge>
          <span className="min-w-0">
            {f.workflow && <span className="font-mono">{f.workflow}: </span>}
            {f.message}
          </span>
        </div>
      ))}
      {fine.length > 0 && (
        <Collapsible>
          <CollapsibleTrigger className="group flex items-center gap-1 text-[11px] text-muted-foreground hover:text-foreground">
            <IconChevronRight className="size-3 transition-transform group-data-[panel-open]:rotate-90" />
            {fine.length} {fine.length === 1 ? "link" : "links"} join up
          </CollapsibleTrigger>
          <CollapsibleContent className="flex flex-col gap-1 pt-1 pl-4">
            {fine.map((f, i) => (
              <span key={i} className="text-[11px] text-muted-foreground">
                {f.workflow && (
                  <span className="font-mono">{f.workflow}: </span>
                )}
                {f.message}
              </span>
            ))}
          </CollapsibleContent>
        </Collapsible>
      )}
    </div>
  );
}

function ColumnSelect({
  label,
  help,
  value,
  onChange,
  states,
  optional,
}: {
  label: string;
  help: React.ReactNode;
  value: string;
  onChange: (v: string) => void;
  states: LinearState[];
  optional?: boolean;
}) {
  const names: Record<string, string> = Object.fromEntries(
    states.map((s) => [s.id, s.name]),
  );
  return (
    <div className="flex flex-col gap-1">
      <span className="text-[11px] font-medium text-muted-foreground">
        {label}
      </span>
      <Select
        value={value}
        onValueChange={(v) => onChange(v == null ? "" : String(v))}
      >
        <SelectTrigger className="h-8 w-full text-[13px]">
          <SelectValue>
            {(v: string | null) =>
              (v && names[v]) ||
              (optional ? "(leave it where it is)" : "Select a column…")
            }
          </SelectValue>
        </SelectTrigger>
        <SelectContent>
          {optional && <SelectItem value="">(leave it where it is)</SelectItem>}
          {states.map((s) => (
            <SelectItem key={s.id} value={s.id}>
              {s.name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <span className="text-[10px] text-muted-foreground">{help}</span>
    </div>
  );
}

/**
 * The bindings that make epics work, written from three column choices. Set
 * by hand, that is three forms whose columns must line up with nothing saying
 * so; see `epicBindingWrites` for what is written.
 */
export function EpicSetupForm({
  project,
  sources,
  teams,
  onDone,
}: {
  project: string;
  sources: LinearSource[];
  teams: LinearTeam[];
  onDone: () => void;
}) {
  const save = useSaveLinearSource(project);
  const byWorkflow = (w: string) => sources.find((s) => s.workflow === w);
  const merge = byWorkflow(MERGE_WORKFLOW);
  const supervisor = byWorkflow(SUPERVISOR_WORKFLOW);
  const builders = sources.filter(isBuilder);

  const [buildWorkflow, setBuildWorkflow] = useState(
    (builders.find((b) => b.piece_ready_state_id) ?? builders[0])?.workflow ??
      "",
  );
  const build = builders.find((b) => b.workflow === buildWorkflow);
  const team = teams.find((t) => t.id === build?.team_id);
  const states = [...(team?.states ?? [])].sort(
    (a, b) => a.position - b.position,
  );
  const named = (re: RegExp) => states.find((s) => re.test(s.name))?.id;

  // Start from what is already wired, then from the usual column names, so a
  // half-done setup opens showing itself rather than blank.
  const [mergeColumn, setMergeColumn] = useState(
    build?.piece_ready_state_id ??
      merge?.source_state_id ??
      named(/ready for merge/i) ??
      "",
  );
  const [doneColumn, setDoneColumn] = useState(
    merge?.ready_state_id ??
      supervisor?.source_state_id ??
      states.find((s) => s.kind === "completed")?.id ??
      "",
  );
  const [epicColumn, setEpicColumn] = useState(
    supervisor?.epic_review_state_id ?? "",
  );
  const [error, setError] = useState<string | null>(null);

  if (builders.length === 0) {
    return (
      <div className="flex flex-col gap-3">
        <p className="text-[13px]">
          Add the binding that builds issues first (for example{" "}
          <span className="font-mono">idea-to-pr</span> from Todo). Epics build
          their pieces with it.
        </p>
        <div>
          <Button size="sm" variant="ghost" onClick={onDone}>
            Back
          </Button>
        </div>
      </div>
    );
  }

  const columns = {
    merge: mergeColumn,
    merged: doneColumn,
    finished: epicColumn,
  };
  const problem = !build
    ? "Choose the binding that builds pieces."
    : epicColumnsProblem(build, columns);
  const canSave = !!build && !!mergeColumn && !!doneColumn && !problem;

  const handleSave = async () => {
    if (!build || !canSave) return;
    setError(null);
    try {
      for (const w of epicBindingWrites(sources, build, columns)) {
        await save.mutateAsync(w);
      }
      onDone();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const buildNames: Record<string, string> = Object.fromEntries(
    builders.map((b) => [b.workflow, `${b.workflow} (${b.team_name})`]),
  );
  const buildColumn = states.find((s) => s.id === build?.source_state_id)?.name;

  return (
    <div className="flex min-w-0 flex-col gap-3">
      <p className="text-[12px] text-muted-foreground">
        An epic is an issue with sub-issues. Each sub-issue is built, merged
        into a shared <span className="font-mono">epic/&lt;ID&gt;</span> branch,
        and graded against its own acceptance criteria before the next one
        starts. The finished feature reaches your default branch as one pull
        request. Choose the columns and this writes the bindings.
      </p>

      {builders.length > 1 && (
        <div className="flex flex-col gap-1">
          <span className="text-[11px] font-medium text-muted-foreground">
            Builds each piece
          </span>
          <Select
            value={buildWorkflow}
            onValueChange={(v) => v != null && setBuildWorkflow(String(v))}
          >
            <SelectTrigger className="h-8 w-full text-[13px]">
              <SelectValue>
                {(v: string | null) => (v && buildNames[v]) || ""}
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              {builders.map((b) => (
                <SelectItem key={b.workflow} value={b.workflow}>
                  {buildNames[b.workflow]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      )}
      {build && (
        <span className="text-[12px]">
          Pieces are built by{" "}
          <span className="font-mono">{build.workflow}</span> from{" "}
          <strong>{buildColumn ?? "its source column"}</strong>.
        </span>
      )}

      <ColumnSelect
        label="Waiting to merge"
        value={mergeColumn}
        onChange={setMergeColumn}
        states={states}
        help={
          <>
            Where a built piece goes.{" "}
            <span className="font-mono">merge-pr</span> picks it up here and
            merges it into the epic branch, once its CI checks pass (or right
            away if the repository has none). Standalone issues keep their own
            Ready column.
          </>
        }
      />
      <ColumnSelect
        label="Merged"
        value={doneColumn}
        onChange={setDoneColumn}
        states={states}
        help={
          <>
            Where a merged piece goes. The supervisor grades it here, then
            starts the next piece or files a fix.
          </>
        }
      />
      <ColumnSelect
        label="Finished epic"
        value={epicColumn}
        onChange={setEpicColumn}
        states={states}
        optional
        help={
          <>
            Where the epic itself goes once every piece is in and its pull
            request is open, for a person to review.
          </>
        }
      />

      <p className="text-[11px] text-muted-foreground">
        To run an epic: create its sub-issues in reverse build order, delegate
        the epic and every sub-issue to the harness, then move the epic to{" "}
        <strong>{buildColumn ?? "the build column"}</strong>.
      </p>

      <div className="flex items-center gap-2 pt-1">
        <Button
          size="sm"
          onClick={handleSave}
          disabled={!canSave || save.isPending}
        >
          {save.isPending ? "Saving…" : "Save"}
        </Button>
        <Button size="sm" variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        {(problem || error) && (
          <span className="text-xs text-destructive">{problem ?? error}</span>
        )}
      </div>
    </div>
  );
}
