/**
 * Read-only report for any workflow that declares `ui.report`: an optional
 * score (for `scored` reports), a summary, and the findings list. The bespoke
 * GEO / Review reports (with triage + Linear actions) stay separate until the
 * finding-stores are unified; this is the generic, declaration-driven view.
 */
import { Markdown } from "@/components/Markdown";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import {
  SEVERITY_RANK,
  type WorkflowFinding,
  type WorkflowVerdict,
} from "@/lib/report";

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
}: {
  verdict: WorkflowVerdict;
  scored: boolean;
}) {
  const findings = [...verdict.findings].sort(
    (a, b) =>
      (SEVERITY_RANK[a.severity ?? ""] ?? 9) -
      (SEVERITY_RANK[b.severity ?? ""] ?? 9),
  );

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
          Findings ({findings.length})
        </h2>
        {findings.length === 0 ? (
          <p className="text-sm text-muted-foreground">No findings.</p>
        ) : (
          <div className="flex flex-col gap-2">
            {findings.map((f, i) => (
              <FindingCard key={i} finding={f} />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

function FindingCard({ finding }: { finding: WorkflowFinding }) {
  const title = finding.title ?? finding.summary ?? "(untitled finding)";
  return (
    <Card>
      <CardContent className="flex flex-col gap-1.5 py-3">
        <div className="flex flex-wrap items-center gap-1.5">
          {finding.severity && (
            <Badge variant={SEVERITY_VARIANT[finding.severity] ?? "secondary"}>
              {finding.severity}
            </Badge>
          )}
          {finding.category && (
            <Badge variant="outline">{finding.category}</Badge>
          )}
          <span className="text-sm font-medium">{title}</span>
        </div>
        {finding.location && (
          <code className="text-[11px] text-muted-foreground">
            {finding.location}
          </code>
        )}
        {finding.detail && (
          <p className="text-sm text-muted-foreground">{finding.detail}</p>
        )}
        {finding.fix && (
          <p className="text-sm">
            <span className="font-medium text-muted-foreground">Fix: </span>
            {finding.fix}
          </p>
        )}
      </CardContent>
    </Card>
  );
}
