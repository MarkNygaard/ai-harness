import { useState } from "react";
import { Check } from "lucide-react";
import { Link } from "react-router-dom";
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
  geoTaskDescription,
  ratingColor,
  useClearGeoFindingState,
  useGeoFindingStates,
  useGeoHistory,
  useSetGeoFindingState,
  type GeoFinding,
  type GeoFindingState,
  type GeoSeverity,
  type GeoVerdict,
} from "@/lib/geo";

/** The workflow GEO findings are filed/built against. */
const IDEA_WORKFLOW = "idea-to-pr";

const SEV_VARIANT: Record<GeoSeverity, "failed" | "running" | "secondary"> = {
  critical: "failed",
  high: "failed",
  medium: "running",
  low: "secondary",
};

/**
 * Renders a geo-audit verdict: score dashboard, per-dimension scores, and the
 * findings. Each finding can be acted on: "Build this" fires `idea-to-pr`, and
 * (when the project has Linear configured) "Create issue" files it into Linear
 * with the eligibility label. Acted-on findings get a green check and a
 * "Rebuild" to restore the buttons; "Ignore" dims and skips them. All three are
 * **persisted per audit run**, so the report shows the same state next visit.
 */
export function GeoReport({
  verdict,
  project,
  runId,
}: {
  verdict: GeoVerdict;
  project: string | null;
  runId: string | null;
}) {
  const projects = useProjects();
  // external_url lands with the project-external-url change; read it loosely so
  // this view doesn't hard-depend on that type at compile time.
  const url =
    (
      projects.data?.find((p) => p.name === project) as
        | { external_url?: string | null }
        | undefined
    )?.external_url ?? "";
  const findings = [...verdict.findings].sort(
    (a, b) => SEVERITY_RANK[a.severity] - SEVERITY_RANK[b.severity],
  );

  // "Create issue" is available only when the project has a Linear API key AND
  // an idea-to-pr binding (team + source status + eligibility label) to file
  // into. Without the binding the server can't resolve where the issue lands.
  const projectCreds = useProjectCredentials(project);
  const linearSources = useLinearSources(project);
  const hasLinearKey = !!projectCreds.data?.some(
    (c) => c.provider === "linear" && c.configured,
  );
  const hasBinding = !!linearSources.data?.some(
    (s) => s.workflow === IDEA_WORKFLOW,
  );
  const linearEnabled = hasLinearKey && hasBinding;

  // Persisted per-finding triage state, keyed by finding_key and scoped to this
  // audit run. Built / issued / ignored survive reloads and revisits.
  const states = useGeoFindingStates(runId);
  const stateByKey: Record<string, GeoFindingState> = {};
  for (const s of states.data ?? []) stateByKey[s.finding_key] = s;

  const createRun = useCreateRun();
  const createIssue = useCreateLinearIssue(project);
  const setState = useSetGeoFindingState(runId);
  const clearState = useClearGeoFindingState(runId);
  const [busy, setBusy] = useState<string | "bulk" | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  // File one finding into Linear and remember it (skips if already acted on).
  async function issueOne(f: GeoFinding) {
    const key = findingKey(f);
    if (stateByKey[key]) return;
    const created = await createIssue.mutateAsync({
      title: f.title,
      description: geoTaskDescription(f, url),
    });
    await setState.mutateAsync({
      finding_key: key,
      action: "issued",
      issue_identifier: created.identifier,
      issue_url: created.url,
    });
  }

  async function build(f: GeoFinding) {
    setActionError(null);
    setBusy(findingKey(f));
    try {
      const res = await createRun.mutateAsync({
        workflow: IDEA_WORKFLOW,
        project: project ?? undefined,
        real: true,
        title: f.title,
        description: geoTaskDescription(f, url),
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

  async function createIssueOne(f: GeoFinding) {
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

  async function ignore(f: GeoFinding) {
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

  // "Rebuild" / "Unignore" — forget the state, restoring the action buttons.
  async function reset(f: GeoFinding) {
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

  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-6">
      <div className="flex items-center gap-4 border border-border p-4">
        <div className="flex flex-col items-center">
          <span
            className="text-4xl font-semibold tabular-nums"
            style={{ color: ratingColor(verdict.score) }}
          >
            {verdict.score}
          </span>
          <span className="text-[11px] uppercase tracking-wide text-muted-foreground">
            GEO score
          </span>
        </div>
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <Badge variant="secondary">{verdict.rating}</Badge>
            {url && (
              <a
                href={url}
                target="_blank"
                rel="noreferrer"
                className="truncate text-xs text-accent-orange hover:underline"
              >
                {url}
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

      <GeoHistory project={project} />

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
        <div className="flex flex-col gap-2">
          {findings.map((f, i) => {
            const key = findingKey(f);
            return (
              <FindingRow
                key={i}
                finding={f}
                linearEnabled={linearEnabled}
                buildable={!!project}
                state={stateByKey[key]}
                busy={busy === key || busy === "bulk"}
                onBuild={() => build(f)}
                onCreateIssue={() => createIssueOne(f)}
                onIgnore={() => ignore(f)}
                onReset={() => reset(f)}
              />
            );
          })}
        </div>
      </section>
    </div>
  );
}

/**
 * GEO score over time for the project: a sparkline of past audits + the delta
 * since the previous one — the audit → fix → re-audit loop made visible. Hidden
 * until there are at least two audits to compare.
 */
function GeoHistory({ project }: { project: string | null }) {
  const { points, loading } = useGeoHistory(project);
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
  finding: GeoFinding;
  linearEnabled: boolean;
  buildable: boolean;
  state: GeoFindingState | undefined;
  busy: boolean;
  onBuild: () => void;
  onCreateIssue: () => void;
  onIgnore: () => void;
  onReset: () => void;
}) {
  const action = state?.action;
  const ignored = action === "ignored";
  const done = action === "built" || action === "issued";
  return (
    <div
      className={cn(
        "border-l-2 border-border bg-card p-3 pl-3",
        done && "border-l-status-success",
        ignored && "opacity-50",
      )}
    >
      <div className="flex items-center gap-2">
        <Badge variant={SEV_VARIANT[finding.severity] ?? "secondary"}>
          {finding.severity}
        </Badge>
        <span className="text-[11px] text-muted-foreground">
          {finding.category}
          {finding.effort ? ` · ${finding.effort}` : ""}
        </span>
        <span
          className={cn(
            "truncate text-sm font-medium",
            ignored && "line-through",
          )}
        >
          {finding.title}
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
              <Button
                type="button"
                size="sm"
                variant="ghost"
                onClick={onReset}
                title="Clear this and bring back the actions"
              >
                Rebuild
              </Button>
            </>
          ) : ignored ? (
            <Button
              type="button"
              size="sm"
              variant="ghost"
              onClick={onReset}
              title="Restore this finding"
            >
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
                title="Ignore this finding (won't be built or filed)"
              >
                Ignore
              </Button>
            </>
          )}
        </div>
      </div>
      {finding.detail && (
        <p className="mt-1 text-[13px] text-muted-foreground">
          {finding.detail}
        </p>
      )}
      <p className="mt-1 text-[13px]">
        <span className="text-muted-foreground">Fix: </span>
        {finding.fix}
      </p>
    </div>
  );
}
