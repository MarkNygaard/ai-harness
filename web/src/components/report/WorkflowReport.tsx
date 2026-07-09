/**
 * The single report view behind every `ui.report` workflow (GEO audit, review,
 * and any custom one). Rendered from a run's verdict:
 * - `scored` reports show a score dashboard, a per-dimension breakdown, and a
 *   score-history sparkline (GEO-style);
 * - all reports show the summary + findings list, where each finding can be
 *   acted on: "Build this" fires `idea-to-pr`, "Create issue" files it into
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
import { useProjectCredentials } from "@/lib/credentials";
import { useCreateLinearIssue, useLinearSources } from "@/lib/linear";
import {
  SEVERITY_RANK,
  findingKey,
  findingTaskDescription,
  ratingColor,
  useClearFindingState,
  useFindingStates,
  useReportHistory,
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
  workflow,
  verdictNode,
}: {
  verdict: WorkflowVerdict;
  scored: boolean;
  project: string | null;
  runId: string | null;
  workflow: string | null;
  verdictNode: string | null;
}) {
  const projects = useProjects();
  const externalUrl =
    projects.data?.find((p) => p.name === project)?.external_url ?? "";

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
  const [busy, setBusy] = useState<string | "bulk" | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

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
        workflow: IDEA_WORKFLOW,
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
    <div className="mx-auto flex max-w-4xl flex-col gap-6">
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
            Findings ({findings.length}) — “Build this” fixes it via idea-to-pr
          </h3>
          {linearEnabled && untouched > 0 && (
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
                busy={busy === findingKey(f) || busy === "bulk"}
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
