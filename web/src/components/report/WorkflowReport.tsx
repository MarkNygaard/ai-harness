/**
 * Report for any workflow that declares `ui.report`: an optional score (for
 * `scored` reports), a summary, and the findings list. Each finding can be
 * acted on — "Build this" fires `idea-to-pr` from the finding's fix, and (when
 * the project has Linear configured) "Create issue" files it into Linear.
 * Acted-on findings get a green check + a "Rebuild"; "Ignore" dims them. All
 * three persist per run via the unified finding-state store, so the report
 * shows the same state next visit. The generic counterpart to GeoReport /
 * ReviewReport.
 */
import { useState } from "react";
import { Check } from "lucide-react";
import { Link } from "react-router-dom";
import { Markdown } from "@/components/Markdown";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useCreateRun } from "@/lib/runs";
import { useProjectCredentials } from "@/lib/credentials";
import { useCreateLinearIssue, useLinearSources } from "@/lib/linear";
import {
  SEVERITY_RANK,
  findingKey,
  findingTaskDescription,
  useClearFindingState,
  useFindingStates,
  useSetFindingState,
  type FindingState,
  type WorkflowFinding,
  type WorkflowVerdict,
} from "@/lib/report";

/** The workflow findings are filed/built against. */
const IDEA_WORKFLOW = "idea-to-pr";

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
}: {
  verdict: WorkflowVerdict;
  scored: boolean;
  project: string | null;
  runId: string | null;
}) {
  const findings = [...verdict.findings].sort(
    (a, b) =>
      (SEVERITY_RANK[a.severity ?? ""] ?? 9) -
      (SEVERITY_RANK[b.severity ?? ""] ?? 9),
  );

  // "Create issue" needs a Linear API key AND an idea-to-pr binding to file into.
  const projectCreds = useProjectCredentials(project);
  const linearSources = useLinearSources(project);
  const hasLinearKey = !!projectCreds.data?.some(
    (c) => c.provider === "linear" && c.configured,
  );
  const hasBinding = !!linearSources.data?.some(
    (s) => s.workflow === IDEA_WORKFLOW,
  );
  const linearEnabled = hasLinearKey && hasBinding;

  // Persisted per-finding triage state, keyed by finding_key, scoped to this run.
  const states = useFindingStates(runId);
  const stateByKey: Record<string, FindingState> = {};
  for (const s of states.data ?? []) stateByKey[s.finding_key] = s;

  const createRun = useCreateRun();
  const createIssue = useCreateLinearIssue(project);
  const setState = useSetFindingState(runId);
  const clearState = useClearFindingState(runId);
  const [busy, setBusy] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  async function build(f: WorkflowFinding) {
    setActionError(null);
    setBusy(findingKey(f));
    try {
      const res = await createRun.mutateAsync({
        workflow: IDEA_WORKFLOW,
        project: project ?? undefined,
        real: true,
        title: f.title ?? f.summary ?? "Finding",
        description: findingTaskDescription(f),
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
      const created = await createIssue.mutateAsync({
        title: f.title ?? f.summary ?? "Finding",
        description: findingTaskDescription(f),
      });
      await setState.mutateAsync({
        finding_key: findingKey(f),
        action: "issued",
        issue_identifier: created.identifier,
        issue_url: created.url,
      });
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

  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-6">
      {scored && verdict.score != null && (
        <div className="flex items-baseline gap-3">
          <span className="text-4xl font-bold tabular-nums">
            {verdict.score}
          </span>
          {verdict.rating && (
            <Badge variant="secondary">{verdict.rating}</Badge>
          )}
        </div>
      )}

      {verdict.summary && (
        <div className="rounded-md bg-muted p-4 text-sm">
          <Markdown>{verdict.summary}</Markdown>
        </div>
      )}

      <section className="flex flex-col gap-2">
        <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          Findings ({findings.length}) — “Build this” fixes it via idea-to-pr
        </h2>
        {actionError && (
          <p className="text-[11px] text-destructive">{actionError}</p>
        )}
        {findings.length === 0 ? (
          <p className="text-sm text-muted-foreground">No findings.</p>
        ) : (
          <div className="flex flex-col gap-2">
            {findings.map((f, i) => (
              <FindingRow
                key={i}
                finding={f}
                linearEnabled={linearEnabled}
                buildable={!!project}
                state={stateByKey[findingKey(f)]}
                busy={busy === findingKey(f)}
                onBuild={() => build(f)}
                onCreateIssue={() => createIssueOne(f)}
                onIgnore={() => ignore(f)}
                onReset={() => reset(f)}
              />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

function FindingRow({
  finding,
  linearEnabled,
  buildable,
  state,
  busy,
  onBuild,
  onCreateIssue,
  onIgnore,
  onReset,
}: {
  finding: WorkflowFinding;
  linearEnabled: boolean;
  buildable: boolean;
  state: FindingState | undefined;
  busy: boolean;
  onBuild: () => void;
  onCreateIssue: () => void;
  onIgnore: () => void;
  onReset: () => void;
}) {
  const action = state?.action;
  const ignored = action === "ignored";
  const done = action === "built" || action === "issued";
  const title = finding.title ?? finding.summary ?? "(untitled finding)";
  return (
    <div
      className={cn(
        "border-l-2 border-border bg-card p-3",
        done && "border-l-status-success",
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
          {done ? (
            <>
              <Check className="size-4 text-status-success" aria-label="done" />
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
              <Button type="button" size="sm" variant="ghost" onClick={onReset}>
                Rebuild
              </Button>
            </>
          ) : ignored ? (
            <Button type="button" size="sm" variant="ghost" onClick={onReset}>
              Unignore
            </Button>
          ) : (
            <>
              {linearEnabled && (
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  title="Create a Linear issue (AI Eligible) for this finding"
                  onClick={onCreateIssue}
                >
                  {busy ? "Working…" : "Create issue"}
                </Button>
              )}
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={busy || !buildable}
                title={
                  buildable
                    ? "Build a fix via idea-to-pr"
                    : "Run has no project to open a PR against"
                }
                onClick={onBuild}
              >
                {busy ? "Working…" : "Build this"}
              </Button>
              <Button
                type="button"
                size="sm"
                variant="ghost"
                onClick={onIgnore}
              >
                Ignore
              </Button>
            </>
          )}
        </div>
      </div>
      {finding.location && (
        <p className="mt-1 font-mono text-[11px] text-muted-foreground">
          {finding.location}
        </p>
      )}
      {finding.detail && (
        <p className="mt-1 text-[13px] text-muted-foreground">
          {finding.detail}
        </p>
      )}
      {finding.fix && (
        <p className="mt-1 text-[13px]">
          <span className="text-muted-foreground">Fix: </span>
          {finding.fix}
        </p>
      )}
    </div>
  );
}
