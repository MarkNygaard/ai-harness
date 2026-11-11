/**
 * The single report view behind every `ui.report` workflow (GEO audit, review,
 * and any custom one). Rendered from a run's verdict:
 * - `scored` reports show a score dashboard, a per-dimension breakdown, and a
 *   score-history sparkline (GEO-style);
 * - all reports show the summary + findings list, where each finding can be
 *   acted on: "Build this" fires `task`, "Create issue" files it into
 *   Linear (when configured), "Ignore" dims it. All three persist per run via
 *   the unified finding-state store, so state survives reloads.
 */
import { useState } from "react";
import { Check } from "lucide-react";
import { Link } from "react-router-dom";
import { Markdown } from "@/components/Markdown";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useCreateRun } from "@/lib/runs";
import { useProjects } from "@/lib/projects";
import {
  useCreateLinearIssue,
  useLinearOauthStatus,
  useLinearSources,
} from "@/lib/linear";
import {
  SEVERITY_RANK,
  findingKey,
  findingTaskDescription,
  issueActionBlocked,
  ratingColor,
  applyDescription,
  useBatchFindingState,
  useClearFindingState,
  useFindingStates,
  useReportHistory,
  useSetFindingState,
  type FindingAction,
  type FindingState,
  type WorkflowFinding,
  type WorkflowVerdict,
} from "@/lib/report";
import type {
  ReportAction,
  ReportApply,
  ReportLayout,
  ReportStatus,
} from "@/types/authoring";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  DECISION_FILTERS,
  DECISION_FILTER_LABEL,
  categoryOf,
  decisionState,
  type Decision,
  type DecisionFilter,
} from "@/lib/report-table";
import { FindingsTable } from "./FindingsTable";

/** The workflow findings are filed/built against. */
const BUILD_WORKFLOW = "task";

const SEVERITY_VARIANT: Record<string, "failed" | "running" | "secondary"> = {
  critical: "failed",
  high: "failed",
  medium: "running",
  low: "secondary",
  info: "secondary",
};

export function WorkflowReport({
  verdict,
  scored,
  project,
  runId,
  workflow,
  verdictNode,
  actions,
  status,
  layout = "list",
  apply = null,
  runTitle = null,
}: {
  verdict: WorkflowVerdict;
  scored: boolean;
  project: string | null;
  runId: string | null;
  workflow: string | null;
  verdictNode: string | null;
  /** Opt-in fix/triage buttons; empty → a clean read-only list. */
  actions: ReportAction[];
  /** Per-item status control (checkbox / pass-fail); `none` → hidden. */
  status: ReportStatus;
  /** Cards, or a table with totals and filters for many findings. */
  layout?: ReportLayout;
  /** The workflow that acts on approved findings, if the report has one. */
  apply?: ReportApply | null;
  /** The run's title, to name the apply run after. */
  runTitle?: string | null;
}) {
  const canBuild = actions.includes("build");
  const canIssue = actions.includes("issue");
  const canIgnore = actions.includes("ignore");
  const canApprove = actions.includes("approve");
  const projects = useProjects();
  const externalUrl =
    projects.data?.find((p) => p.name === project)?.external_url ?? "";

  const findings = [...verdict.findings].sort(
    (a, b) =>
      (SEVERITY_RANK[a.severity ?? ""] ?? 9) -
      (SEVERITY_RANK[b.severity ?? ""] ?? 9),
  );

  // "Create issue" needs Linear connected AND a task binding to file into.
  //
  // Connectedness is read from the OAuth status, not from per-project
  // credentials. Linear is configured globally — the identity is the *app*, the
  // same for every project — so `/api/projects/{p}/credentials` never reports a
  // `linear` row (its provider list is `["github"]`, and a leftover per-project
  // row is documented as inert). Asking it therefore always answered "not
  // configured", which hid this button on every report for every project.
  const linearStatus = useLinearOauthStatus();
  const linearSources = useLinearSources(project);
  const hasBinding = !!linearSources.data?.some(
    (s) => s.workflow === BUILD_WORKFLOW,
  );
  // A button that simply vanishes teaches nobody anything — this gate read the
  // wrong source for months and went unreported because its failure mode was an
  // absence. When the action is declared by the workflow but a prerequisite is
  // missing, show it disabled and name the prerequisite.
  const issueBlocked = issueActionBlocked(
    linearStatus.data?.mode,
    hasBinding,
    BUILD_WORKFLOW,
  );
  const linearEnabled = issueBlocked === null;

  // Persisted per-finding triage state, keyed by finding_key, scoped to this run.
  const states = useFindingStates(runId);
  const stateByKey: Record<string, FindingState> = {};
  for (const s of states.data ?? []) stateByKey[s.finding_key] = s;

  const createRun = useCreateRun();
  const createIssue = useCreateLinearIssue(project);
  const setState = useSetFindingState(runId);
  const clearState = useClearFindingState(runId);
  const batchState = useBatchFindingState(runId);
  const [busy, setBusy] = useState<string | "bulk" | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [decision, setDecision] = useState<DecisionFilter>("all");
  const [startedRun, setStartedRun] = useState<string | null>(null);

  // Only the categories the apply workflow acts on are up for a decision: a
  // "keep" row has nothing to approve.
  const decidable = (f: WorkflowFinding) =>
    canApprove &&
    (!apply?.categories?.length || apply.categories.includes(categoryOf(f)));
  const approved = findings.filter(
    (f) => decidable(f) && stateByKey[findingKey(f)]?.action === "approved",
  );

  /** Record one decision for every finding in `rows`, or clear theirs. */
  async function decideAll(rows: WorkflowFinding[], action: Decision | null) {
    // A finding an apply run already took is not up for a new decision, but
    // can be undone (its run may have failed).
    const keys = rows
      .filter(decidable)
      .map(findingKey)
      .filter((k) => action === null || stateByKey[k]?.action !== "applied");
    if (keys.length === 0) return;
    setActionError(null);
    setBusy("bulk");
    try {
      await batchState.mutateAsync({ finding_keys: keys, action });
    } catch (e) {
      setActionError((e as Error).message);
    } finally {
      setBusy(null);
    }
  }

  /**
   * Start the apply workflow with the approved findings, then mark them
   * applied, so they show which run took them and can't be sent twice.
   */
  async function applyApproved() {
    if (!apply || !runId || approved.length === 0) return;
    setActionError(null);
    setBusy("bulk");
    try {
      const res = await createRun.mutateAsync({
        workflow: apply.workflow,
        project: project ?? undefined,
        real: true,
        title: `${apply.label || "Apply"}: ${approved.length} from ${runTitle || runId}`,
        description: applyDescription(runId, approved),
      });
      await batchState.mutateAsync({
        finding_keys: approved.map(findingKey),
        action: "applied",
        ref_run_id: res.run_id,
      });
      setStartedRun(res.run_id);
    } catch (e) {
      setActionError((e as Error).message);
    } finally {
      setBusy(null);
    }
  }

  async function decideOne(f: WorkflowFinding, action: Decision) {
    setActionError(null);
    try {
      if (stateByKey[findingKey(f)]?.action === action) {
        await clearState.mutateAsync(findingKey(f));
      } else {
        await setState.mutateAsync({ finding_key: findingKey(f), action });
      }
    } catch (e) {
      setActionError((e as Error).message);
    }
  }

  const decisionOf = (f: WorkflowFinding) =>
    decisionState(stateByKey[findingKey(f)]?.action);
  const decided = canApprove
    ? findings.filter((f) => decision === "all" || decisionOf(f) === decision)
    : findings;

  async function issueOne(f: WorkflowFinding) {
    const key = findingKey(f);
    if (stateByKey[key]) return;
    const created = await createIssue.mutateAsync({
      title: f.title ?? f.summary ?? "Finding",
      description: findingTaskDescription(f, externalUrl),
    });
    await setState.mutateAsync({
      finding_key: key,
      action: "issued",
      issue_identifier: created.identifier,
      issue_url: created.url,
    });
  }

  async function build(f: WorkflowFinding) {
    setActionError(null);
    setBusy(findingKey(f));
    try {
      const res = await createRun.mutateAsync({
        workflow: BUILD_WORKFLOW,
        project: project ?? undefined,
        real: true,
        title: f.title ?? f.summary ?? "Finding",
        description: findingTaskDescription(f, externalUrl),
      });
      await setState.mutateAsync({
        finding_key: findingKey(f),
        action: "built",
        ref_run_id: res.run_id,
      });
    } catch (e) {
      setActionError((e as Error).message);
    } finally {
      setBusy(null);
    }
  }

  async function createIssueOne(f: WorkflowFinding) {
    setActionError(null);
    setBusy(findingKey(f));
    try {
      await issueOne(f);
    } catch (e) {
      setActionError((e as Error).message);
    } finally {
      setBusy(null);
    }
  }

  async function ignore(f: WorkflowFinding) {
    setActionError(null);
    try {
      await setState.mutateAsync({
        finding_key: findingKey(f),
        action: "ignored",
      });
    } catch (e) {
      setActionError((e as Error).message);
    }
  }

  async function reset(f: WorkflowFinding) {
    setActionError(null);
    try {
      await clearState.mutateAsync(findingKey(f));
    } catch (e) {
      setActionError((e as Error).message);
    }
  }

  // Set a per-item status (checked / passed / failed); re-selecting the current
  // one clears it.
  async function mark(f: WorkflowFinding, value: FindingAction) {
    setActionError(null);
    try {
      if (stateByKey[findingKey(f)]?.action === value) {
        await clearState.mutateAsync(findingKey(f));
      } else {
        await setState.mutateAsync({
          finding_key: findingKey(f),
          action: value,
        });
      }
    } catch (e) {
      setActionError((e as Error).message);
    }
  }

  async function createAll() {
    setActionError(null);
    setBusy("bulk");
    try {
      for (const f of findings) {
        if (stateByKey[findingKey(f)]) continue;
        await issueOne(f);
      }
    } catch (e) {
      setActionError((e as Error).message);
    } finally {
      setBusy(null);
    }
  }

  const untouched = findings.filter((f) => !stateByKey[findingKey(f)]).length;
  const showScore = scored && verdict.score != null;

  return (
    <div
      className={cn(
        "mx-auto flex flex-col gap-6",
        // A table needs the width for its columns.
        layout === "table" ? "max-w-6xl" : "max-w-4xl",
      )}
    >
      {showScore ? (
        <div className="flex items-center gap-4 border border-border p-4">
          <div className="flex flex-col items-center">
            <span
              className="text-4xl font-semibold tabular-nums"
              style={{ color: ratingColor(verdict.score!) }}
            >
              {verdict.score}
            </span>
            <span className="text-[11px] uppercase tracking-wide text-muted-foreground">
              score
            </span>
          </div>
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              {verdict.rating && (
                <Badge variant="secondary">{verdict.rating}</Badge>
              )}
              {externalUrl && (
                <a
                  href={externalUrl}
                  target="_blank"
                  rel="noreferrer"
                  className="truncate text-xs text-accent-orange hover:underline"
                >
                  {externalUrl}
                </a>
              )}
            </div>
            {verdict.summary && (
              <p className="mt-1 text-[13px] text-muted-foreground">
                {verdict.summary}
              </p>
            )}
          </div>
        </div>
      ) : (
        verdict.summary && (
          <div className="rounded-md bg-muted p-4 text-sm">
            <Markdown>{verdict.summary}</Markdown>
          </div>
        )
      )}

      {scored && (
        <ReportHistory
          workflow={workflow}
          project={project}
          verdictNode={verdictNode}
        />
      )}

      {scored && verdict.categories && verdict.categories.length > 0 && (
        <section className="flex flex-col gap-2">
          <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            Scores by dimension
          </h3>
          <div className="flex flex-col gap-2 border border-border p-3">
            {verdict.categories.map((c) => (
              <div key={c.key} className="flex items-center gap-3 text-[13px]">
                <div className="w-28 shrink-0 font-medium">{c.key}</div>
                <div className="h-2.5 flex-1 overflow-hidden bg-secondary/50">
                  <div
                    className="h-full"
                    style={{
                      width: `${Math.max(0, Math.min(100, c.score))}%`,
                      backgroundColor: ratingColor(c.score),
                    }}
                  />
                </div>
                <div className="w-8 shrink-0 text-right tabular-nums text-muted-foreground">
                  {c.score}
                </div>
              </div>
            ))}
          </div>
        </section>
      )}

      <section className="flex flex-col gap-2">
        <div className="flex items-center justify-between gap-2">
          <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            {verdict.findings.length === findings.length && canBuild
              ? `Findings (${findings.length}) — “Build this” runs the task workflow to fix it`
              : `Findings (${findings.length})`}
          </h3>
          {canIssue && linearEnabled && untouched > 0 && (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={busy !== null}
              onClick={createAll}
            >
              {busy === "bulk"
                ? "Filing…"
                : `Create Linear issues for all (${untouched})`}
            </Button>
          )}
        </div>
        {actionError && (
          <p className="text-[11px] text-destructive">{actionError}</p>
        )}
        {apply && canApprove && (
          <ApplyBar
            label={apply.label || "Apply"}
            workflow={apply.workflow}
            approved={approved.length}
            canStart={!!project && !!runId}
            busy={busy === "bulk"}
            startedRun={startedRun}
            onApply={applyApproved}
          />
        )}
        {layout === "table" ? (
          <FindingsTable
            findings={decided}
            dimmed={(f) => {
              const a = stateByKey[findingKey(f)]?.action;
              return a === "ignored" || a === "rejected";
            }}
            toolbar={
              canApprove
                ? (rows) => (
                    <DecisionTools
                      filter={decision}
                      onFilter={setDecision}
                      shown={rows.filter(decidable).length}
                      busy={busy !== null}
                      onApproveAll={() => decideAll(rows, "approved")}
                      onRejectAll={() => decideAll(rows, "rejected")}
                      onClearAll={() => decideAll(rows, null)}
                    />
                  )
                : undefined
            }
            actionsFor={
              (canBuild && project) || canIssue || canIgnore || canApprove
                ? (f) => (
                    <TableActions
                      finding={f}
                      state={stateByKey[findingKey(f)]}
                      canBuild={canBuild && !!project && !f.offsite}
                      canIssue={canIssue}
                      issueBlocked={issueBlocked}
                      canIgnore={canIgnore}
                      canApprove={decidable(f)}
                      busy={busy === findingKey(f) || busy === "bulk"}
                      onBuild={() => build(f)}
                      onCreateIssue={() => createIssueOne(f)}
                      onIgnore={() => ignore(f)}
                      onDecide={(d) => decideOne(f, d)}
                      onReset={() => reset(f)}
                    />
                  )
                : undefined
            }
          />
        ) : findings.length === 0 ? (
          <p className="text-sm text-muted-foreground">No findings.</p>
        ) : (
          <div className="flex flex-col gap-2">
            {findings.map((f, i) => (
              <FindingRow
                key={i}
                finding={f}
                canBuild={canBuild && !!project}
                canIssue={canIssue}
                issueBlocked={issueBlocked}
                canIgnore={canIgnore}
                canApprove={decidable(f)}
                status={status}
                state={stateByKey[findingKey(f)]}
                busy={busy === findingKey(f) || busy === "bulk"}
                onBuild={() => build(f)}
                onCreateIssue={() => createIssueOne(f)}
                onIgnore={() => ignore(f)}
                onDecide={(d) => decideOne(f, d)}
                onReset={() => reset(f)}
                onMark={(v) => mark(f, v)}
              />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

/**
 * Score over time for the report's workflow + project: a sparkline of past runs
 * + the delta since the previous one. Hidden until there are two scored runs to
 * compare. The generic counterpart to the former GeoHistory.
 */
function ReportHistory({
  workflow,
  project,
  verdictNode,
}: {
  workflow: string | null;
  project: string | null;
  verdictNode: string | null;
}) {
  const { points, loading } = useReportHistory(workflow, project, verdictNode);
  if (loading || points.length < 2) return null;
  const last = points[points.length - 1];
  const prev = points[points.length - 2];
  const delta = last.score - prev.score;
  return (
    <div className="flex items-center gap-3 border border-border p-3">
      <span className="shrink-0 text-[11px] uppercase tracking-wide text-muted-foreground">
        Score history
      </span>
      <div className="flex h-8 items-end gap-0.5">
        {points.map((p) => (
          <Link
            key={p.runId}
            to={`/runs/${p.runId}`}
            title={`${new Date(p.at).toLocaleDateString()}: ${p.score}`}
          >
            <div
              className="w-2"
              style={{
                height: `${Math.max(4, (p.score / 100) * 32)}px`,
                backgroundColor: ratingColor(p.score),
              }}
            />
          </Link>
        ))}
      </div>
      <span
        className="shrink-0 text-xs tabular-nums"
        style={{
          color: delta >= 0 ? "var(--status-success)" : "var(--status-failed)",
        }}
      >
        {delta > 0 ? `▲ +${delta}` : delta < 0 ? `▼ ${delta}` : "±0"} vs
        previous
      </span>
    </div>
  );
}

/**
 * The decision tools above a table: which decisions to show, and one decision
 * for every row in view (after the category tab, the search and this filter).
 */
function DecisionTools({
  filter,
  onFilter,
  shown,
  busy,
  onApproveAll,
  onRejectAll,
  onClearAll,
}: {
  filter: DecisionFilter;
  onFilter: (f: DecisionFilter) => void;
  shown: number;
  busy: boolean;
  onApproveAll: () => void;
  onRejectAll: () => void;
  onClearAll: () => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Select
        value={filter}
        onValueChange={(v) => v && onFilter(v as DecisionFilter)}
      >
        <SelectTrigger size="sm" aria-label="Show decisions">
          <SelectValue>
            {(v: string) => DECISION_FILTER_LABEL[v as DecisionFilter]}
          </SelectValue>
        </SelectTrigger>
        <SelectContent>
          {DECISION_FILTERS.map((d) => (
            <SelectItem key={d} value={d}>
              {DECISION_FILTER_LABEL[d]}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      {/* Applied rows can only be undone (their run may have failed). */}
      {shown > 0 && filter === "applied" && (
        <Button variant="ghost" size="sm" disabled={busy} onClick={onClearAll}>
          Undo {shown}
        </Button>
      )}
      {shown > 0 && filter !== "applied" && (
        <>
          <Button
            variant="outline"
            size="sm"
            disabled={busy}
            onClick={onApproveAll}
          >
            Approve {shown} shown
          </Button>
          <Button
            variant="ghost"
            size="sm"
            disabled={busy}
            onClick={onRejectAll}
          >
            Reject {shown}
          </Button>
          {filter !== "undecided" && (
            <Button
              variant="ghost"
              size="sm"
              disabled={busy}
              onClick={onClearAll}
            >
              Undo {shown}
            </Button>
          )}
        </>
      )}
    </div>
  );
}

/**
 * How many findings are approved, and the button that starts the report's
 * apply workflow with them. Once started, a link to that run.
 */
function ApplyBar({
  label,
  workflow,
  approved,
  canStart,
  busy,
  startedRun,
  onApply,
}: {
  label: string;
  workflow: string;
  approved: number;
  canStart: boolean;
  busy: boolean;
  startedRun: string | null;
  onApply: () => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-3 rounded-md border border-border bg-card p-3 text-sm">
      <span className="tabular-nums">
        <span className="font-semibold">{approved}</span> approved
      </span>
      <span className="text-xs text-muted-foreground">
        {label} starts <span className="font-mono">{workflow}</span> with the
        approved findings.
      </span>
      {startedRun && (
        <Button
          variant="link"
          size="sm"
          render={<Link to={`/runs/${startedRun}`} />}
          nativeButton={false}
        >
          Started: view the run
        </Button>
      )}
      <Button
        className="ml-auto"
        size="sm"
        disabled={approved === 0 || !canStart || busy}
        title={canStart ? undefined : "This run has no project to start it in"}
        onClick={onApply}
      >
        {busy ? "Starting…" : `${label} (${approved})`}
      </Button>
    </div>
  );
}

/**
 * A table row's buttons: Approve / Reject where the report decides, then
 * build / issue / ignore, as on a card. Once decided or acted on, what was
 * done, with a link to the run where there is one.
 */
function TableActions({
  finding,
  state,
  canBuild,
  canIssue,
  issueBlocked,
  canIgnore,
  canApprove,
  busy,
  onBuild,
  onCreateIssue,
  onIgnore,
  onDecide,
  onReset,
}: {
  finding: WorkflowFinding;
  state: FindingState | undefined;
  canBuild: boolean;
  canIssue: boolean;
  issueBlocked: string | null;
  canIgnore: boolean;
  canApprove: boolean;
  busy: boolean;
  onBuild: () => void;
  onCreateIssue: () => void;
  onIgnore: () => void;
  onDecide: (d: Decision) => void;
  onReset: () => void;
}) {
  const action = state?.action;
  if (action === "applied") {
    return (
      <span className="inline-flex items-center gap-1">
        {state?.ref_run_id ? (
          <Button
            variant="link"
            size="xs"
            render={<Link to={`/runs/${state.ref_run_id}`} />}
            nativeButton={false}
            title="The run that is acting on it"
          >
            Applied
          </Button>
        ) : (
          <Badge variant="success">Applied</Badge>
        )}
        {/* If that run failed, this is how the finding goes back. */}
        <Button
          variant="ghost"
          size="xs"
          disabled={busy}
          title="Undo, for instance if that run failed"
          onClick={onReset}
        >
          Undo
        </Button>
      </span>
    );
  }
  if (action === "approved" || action === "rejected") {
    return (
      <span className="inline-flex items-center gap-1">
        <Badge variant={action === "approved" ? "success" : "secondary"}>
          {action === "approved" ? "Approved" : "Rejected"}
        </Badge>
        <Button variant="ghost" size="xs" disabled={busy} onClick={onReset}>
          Undo
        </Button>
      </span>
    );
  }
  if (action === "built" || action === "issued") {
    const href =
      action === "built" && state?.ref_run_id
        ? `/runs/${state.ref_run_id}`
        : null;
    return (
      <span className="inline-flex items-center gap-1">
        {href ? (
          <Button
            variant="link"
            size="xs"
            render={<Link to={href} />}
            nativeButton={false}
          >
            Building
          </Button>
        ) : state?.issue_url ? (
          <Button
            variant="link"
            size="xs"
            render={
              <a href={state.issue_url} target="_blank" rel="noreferrer" />
            }
            nativeButton={false}
          >
            {state.issue_identifier ?? "Issue"}
          </Button>
        ) : (
          <Check className="size-4 text-status-success" aria-label="done" />
        )}
        <Button variant="ghost" size="xs" onClick={onReset}>
          Undo
        </Button>
      </span>
    );
  }
  if (action === "ignored") {
    return (
      <Button variant="ghost" size="xs" onClick={onReset}>
        Unignore
      </Button>
    );
  }
  return (
    <span className="inline-flex items-center gap-1">
      {canApprove && (
        <>
          <Button
            variant="outline"
            size="xs"
            disabled={busy}
            title="Approve what this finding proposes"
            onClick={() => onDecide("approved")}
          >
            Approve
          </Button>
          <Button
            variant="ghost"
            size="xs"
            disabled={busy}
            title="Reject it: leave things as they are"
            onClick={() => onDecide("rejected")}
          >
            Reject
          </Button>
        </>
      )}
      {canBuild && (
        <Button
          variant="outline"
          size="xs"
          disabled={busy}
          title={`Build a fix with the task workflow: ${finding.title ?? ""}`}
          onClick={onBuild}
        >
          Build
        </Button>
      )}
      {canIssue && (
        <Button
          variant="outline"
          size="xs"
          disabled={busy || !!issueBlocked}
          title={issueBlocked ?? "File a Linear issue for this finding"}
          onClick={onCreateIssue}
        >
          Issue
        </Button>
      )}
      {canIgnore && (
        <Button variant="ghost" size="xs" onClick={onIgnore}>
          Ignore
        </Button>
      )}
    </span>
  );
}

function FindingRow({
  finding,
  canBuild,
  canIssue,
  issueBlocked,
  canIgnore,
  canApprove,
  status,
  state,
  busy,
  onBuild,
  onCreateIssue,
  onIgnore,
  onDecide,
  onReset,
  onMark,
}: {
  finding: WorkflowFinding;
  canBuild: boolean;
  canIssue: boolean;
  /** Why "Create issue" cannot be used, or null when it can. */
  issueBlocked: string | null;
  canIgnore: boolean;
  canApprove: boolean;
  status: ReportStatus;
  state: FindingState | undefined;
  busy: boolean;
  onBuild: () => void;
  onCreateIssue: () => void;
  onIgnore: () => void;
  onDecide: (d: Decision) => void;
  onReset: () => void;
  onMark: (value: FindingAction) => void;
}) {
  const action = state?.action;
  const ignored = action === "ignored" || action === "rejected";
  const failed = action === "failed";
  const positive =
    action === "built" ||
    action === "issued" ||
    action === "checked" ||
    action === "passed" ||
    action === "approved" ||
    action === "applied";
  // An off-site finding ("earn a Wikipedia entity") has no code change behind it,
  // so `task` would have nothing to implement. It stays filable as an issue
  // for a human, which is what such an item actually needs.
  const buildable = canBuild && !finding.offsite;
  const hasFixActions = buildable || canIssue || canIgnore;
  const title = finding.title ?? finding.summary ?? "(untitled finding)";
  return (
    <div
      className={cn(
        "border-l-2 border-border bg-card p-3",
        positive && "border-l-status-success",
        failed && "border-l-status-failed",
        ignored && "opacity-50",
      )}
    >
      <div className="flex items-center gap-2">
        {finding.severity && (
          <Badge variant={SEVERITY_VARIANT[finding.severity] ?? "secondary"}>
            {finding.severity}
          </Badge>
        )}
        {finding.category && (
          <span className="text-[11px] text-muted-foreground">
            {finding.category}
            {finding.effort ? ` · ${finding.effort}` : ""}
          </span>
        )}
        <span
          className={cn(
            "truncate text-sm font-medium",
            ignored && "line-through",
          )}
        >
          {title}
        </span>
        <div className="ml-auto flex shrink-0 items-center gap-2">
          {/* A decision on what the finding proposes. */}
          {canApprove &&
            (action === "applied" ? (
              <>
                {state?.ref_run_id ? (
                  <Link
                    to={`/runs/${state.ref_run_id}`}
                    className="text-xs text-accent-orange hover:underline"
                  >
                    Applied →
                  </Link>
                ) : (
                  <Badge variant="success">Applied</Badge>
                )}
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  disabled={busy}
                  title="Undo, for instance if that run failed"
                  onClick={onReset}
                >
                  Undo
                </Button>
              </>
            ) : action === "approved" || action === "rejected" ? (
              <>
                <Badge
                  variant={action === "approved" ? "success" : "secondary"}
                >
                  {action === "approved" ? "Approved" : "Rejected"}
                </Badge>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  disabled={busy}
                  onClick={onReset}
                >
                  Undo
                </Button>
              </>
            ) : (
              <>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() => onDecide("approved")}
                >
                  Approve
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  disabled={busy}
                  onClick={() => onDecide("rejected")}
                >
                  Reject
                </Button>
              </>
            ))}
          {/* Per-item status control (checkbox / pass-fail). */}
          {status === "check" && (
            <label className="flex items-center gap-1.5 text-xs text-muted-foreground">
              <input
                type="checkbox"
                className="size-3.5"
                checked={action === "checked"}
                disabled={busy}
                onChange={() => onMark("checked")}
              />
              tested
            </label>
          )}
          {status === "pass_fail" &&
            (action === "passed" || action === "failed" ? (
              <>
                <Badge variant={action === "passed" ? "success" : "failed"}>
                  {action === "passed" ? "Passed" : "Failed"}
                </Badge>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  onClick={onReset}
                >
                  Clear
                </Button>
              </>
            ) : (
              <>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() => onMark("passed")}
                >
                  Passed
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() => onMark("failed")}
                >
                  Failed
                </Button>
              </>
            ))}

          {/* Opt-in fix/triage actions (build / issue / ignore). */}
          {hasFixActions &&
            (action === "built" || action === "issued" ? (
              <>
                <Check
                  className="size-4 text-status-success"
                  aria-label="done"
                />
                {action === "built" && state?.ref_run_id ? (
                  <Link
                    to={`/runs/${state.ref_run_id}`}
                    className="text-xs text-accent-orange hover:underline"
                  >
                    Building →
                  </Link>
                ) : action === "issued" && state?.issue_url ? (
                  <a
                    href={state.issue_url}
                    target="_blank"
                    rel="noreferrer"
                    className="text-xs text-accent-orange hover:underline"
                    title="Open the Linear issue"
                  >
                    {state.issue_identifier ?? "Issue"} →
                  </a>
                ) : null}
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  onClick={onReset}
                >
                  Rebuild
                </Button>
              </>
            ) : ignored ? (
              <Button type="button" size="sm" variant="ghost" onClick={onReset}>
                Unignore
              </Button>
            ) : (
              <>
                {canIssue && (
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    disabled={busy || !!issueBlocked}
                    title={
                      issueBlocked ??
                      "File a Linear issue for this finding — delegate it to the harness in Linear to start work"
                    }
                    onClick={onCreateIssue}
                  >
                    {busy ? "Working…" : "Create issue"}
                  </Button>
                )}
                {/* An off-site finding ("earn a Wikipedia entity") has no code
                    change behind it, so the task workflow would have nothing to
                    implement. It can still be filed as an issue for a human. */}
                {buildable && (
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    disabled={busy}
                    title="Build a fix with the task workflow"
                    onClick={onBuild}
                  >
                    {busy ? "Working…" : "Build this"}
                  </Button>
                )}
                {canIgnore && (
                  <Button
                    type="button"
                    size="sm"
                    variant="ghost"
                    onClick={onIgnore}
                  >
                    Ignore
                  </Button>
                )}
              </>
            ))}
        </div>
      </div>
      {finding.location && (
        <p className="mt-1 font-mono text-[11px] text-muted-foreground">
          {finding.location}
        </p>
      )}
      {finding.detail && <Markdown className="mt-1">{finding.detail}</Markdown>}
      {finding.fix && (
        <div className="mt-1">
          <span className="text-[13px] font-medium text-muted-foreground">
            Fix:
          </span>
          <Markdown>{finding.fix}</Markdown>
        </div>
      )}
    </div>
  );
}
