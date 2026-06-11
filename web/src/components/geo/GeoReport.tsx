import { useState } from "react";
import { Link } from "react-router-dom";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useCreateRun } from "@/lib/runs";
import { useProjects } from "@/lib/projects";
import { useProjectCredentials } from "@/lib/credentials";
import { useCreateLinearIssue, useLinearSources } from "@/lib/linear";
import type { CreatedLinearIssue } from "@/types/linear";
import {
  SEVERITY_RANK,
  geoTaskDescription,
  ratingColor,
  useGeoHistory,
  type GeoFinding,
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
 * findings. Each finding can be acted on two ways: "Build this" fires
 * `idea-to-pr` immediately, and (when the project has Linear configured)
 * "Create issue" files it into Linear with the eligibility label so the poller
 * picks it up. A bulk action files every finding at once.
 */
export function GeoReport({
  verdict,
  project,
}: {
  verdict: GeoVerdict;
  project: string | null;
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

  // Created issues keyed by the finding's index in `findings` — shared between
  // the per-finding buttons and the bulk action so neither double-files.
  const [issues, setIssues] = useState<Record<number, CreatedLinearIssue>>({});
  const [filing, setFiling] = useState<number | "bulk" | null>(null);
  const [fileError, setFileError] = useState<string | null>(null);
  const createIssue = useCreateLinearIssue(project);

  // Findings the user has dismissed (by index). Ignored ones are skipped by the
  // bulk action and can't be built/filed until un-ignored. Session-scoped.
  const [ignored, setIgnored] = useState<Set<number>>(() => new Set());
  function toggleIgnore(idx: number) {
    setIgnored((prev) => {
      const next = new Set(prev);
      if (next.has(idx)) next.delete(idx);
      else next.add(idx);
      return next;
    });
  }

  async function fileFinding(idx: number, f: GeoFinding) {
    if (issues[idx]) return;
    const created = await createIssue.mutateAsync({
      title: f.title,
      description: geoTaskDescription(f, url),
    });
    setIssues((prev) => ({ ...prev, [idx]: created }));
  }

  async function fileOne(idx: number, f: GeoFinding) {
    setFileError(null);
    setFiling(idx);
    try {
      await fileFinding(idx, f);
    } catch (e) {
      setFileError((e as Error).message);
    } finally {
      setFiling(null);
    }
  }

  async function fileAll() {
    setFileError(null);
    setFiling("bulk");
    try {
      for (let i = 0; i < findings.length; i++) {
        if (ignored.has(i)) continue;
        await fileFinding(i, findings[i]);
      }
    } catch (e) {
      setFileError((e as Error).message);
    } finally {
      setFiling(null);
    }
  }

  const unfiled = findings.filter(
    (_, i) => !issues[i] && !ignored.has(i),
  ).length;

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
          {linearEnabled && unfiled > 0 && (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={filing !== null}
              onClick={fileAll}
            >
              {filing === "bulk"
                ? "Filing…"
                : `Create Linear issues for all (${unfiled})`}
            </Button>
          )}
        </div>
        {fileError && (
          <p className="text-[11px] text-destructive">{fileError}</p>
        )}
        <div className="flex flex-col gap-2">
          {findings.map((f, i) => (
            <FindingRow
              key={i}
              finding={f}
              project={project}
              url={url}
              linearEnabled={linearEnabled}
              issue={issues[i]}
              filing={filing === i}
              ignored={ignored.has(i)}
              onCreateIssue={() => fileOne(i, f)}
              onToggleIgnore={() => toggleIgnore(i)}
            />
          ))}
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
  project,
  url,
  linearEnabled,
  issue,
  filing,
  ignored,
  onCreateIssue,
  onToggleIgnore,
}: {
  finding: GeoFinding;
  project: string | null;
  url: string;
  linearEnabled: boolean;
  issue: CreatedLinearIssue | undefined;
  filing: boolean;
  ignored: boolean;
  onCreateIssue: () => void;
  onToggleIgnore: () => void;
}) {
  const create = useCreateRun();
  return (
    <div
      className={cn(
        "border-l-2 border-border bg-card p-3 pl-3",
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
          {linearEnabled &&
            (issue ? (
              <a
                href={issue.url}
                target="_blank"
                rel="noreferrer"
                className="text-xs text-accent-orange hover:underline"
                title="Open the Linear issue"
              >
                {issue.identifier} →
              </a>
            ) : (
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={filing || !project || ignored}
                title="Create a Linear issue (AI Eligible) for this finding"
                onClick={onCreateIssue}
              >
                {filing ? "Filing…" : "Create issue"}
              </Button>
            ))}
          {create.data ? (
            <Link
              to={`/runs/${create.data.run_id}`}
              className="text-xs text-accent-orange hover:underline"
            >
              Building →
            </Link>
          ) : (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={create.isPending || !project || ignored}
              title={!project ? "Run has no project to open a PR against" : ""}
              onClick={() =>
                create.mutate({
                  workflow: IDEA_WORKFLOW,
                  project: project ?? undefined,
                  real: true,
                  title: finding.title,
                  description: geoTaskDescription(finding, url),
                })
              }
            >
              {create.isPending ? "Starting…" : "Build this"}
            </Button>
          )}
          <Button
            type="button"
            size="sm"
            variant="ghost"
            onClick={onToggleIgnore}
            title={
              ignored
                ? "Restore this finding"
                : "Ignore this finding (won't be built or filed)"
            }
          >
            {ignored ? "Unignore" : "Ignore"}
          </Button>
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
      {create.isError && (
        <p className="mt-1 text-[11px] text-destructive">
          {create.error.message}
        </p>
      )}
    </div>
  );
}
