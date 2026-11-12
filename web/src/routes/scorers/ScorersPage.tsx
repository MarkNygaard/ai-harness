/**
 * Every scorer that has graded a run: a workflow step marked `scorer:` that
 * judges the run it is in. Each links to its own page with the trend and the
 * runs behind it.
 */
import { Link } from "react-router-dom";
import { AppShell } from "@/components/AppShell";
import { Badge } from "@/components/ui/badge";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { formatCost } from "@/lib/cost";
import { passRate, useScorers } from "@/lib/scorers";

export function ScorersPage() {
  const scorers = useScorers();
  return (
    <AppShell title="Scorers">
      <div className="mx-auto flex max-w-5xl flex-col gap-4 p-6">
        <p className="max-w-3xl text-sm text-muted-foreground">
          A scorer is a workflow step that grades the run it is in, such as
          whether a pull request adds tests nothing needs. Mark a step with{" "}
          <code className="font-mono text-xs">scorer:</code> and its grades show
          up here.
        </p>
        {scorers.isLoading && (
          <p className="text-sm text-muted-foreground">Loading…</p>
        )}
        {scorers.isError && (
          <p className="text-sm text-destructive">{scorers.error.message}</p>
        )}
        {scorers.data && scorers.data.length === 0 && (
          <p className="text-sm text-muted-foreground">
            No scorer has graded a run yet.
          </p>
        )}
        {scorers.data && scorers.data.length > 0 && (
          <div className="rounded-md border border-border">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Scorer</TableHead>
                  <TableHead className="text-right">Runs</TableHead>
                  <TableHead className="text-right">Pass rate</TableHead>
                  <TableHead className="text-right">All-time cost</TableHead>
                  <TableHead>Last graded</TableHead>
                  <TableHead>Workflows</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {scorers.data.map((s) => (
                  <TableRow key={s.scorer}>
                    <TableCell className="max-w-80">
                      <Link
                        to={`/scorers/${encodeURIComponent(s.scorer)}`}
                        className="font-medium hover:underline"
                      >
                        {s.scorer}
                      </Link>
                      {s.description && (
                        <div
                          className="truncate text-xs text-muted-foreground"
                          title={s.description}
                        >
                          {s.description}
                        </div>
                      )}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {s.runs}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {passRate(s.passed, s.runs)}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {formatCost(s.total_cost_usd)}
                    </TableCell>
                    <TableCell className="text-muted-foreground">
                      {new Date(s.last_at).toLocaleDateString()}
                    </TableCell>
                    <TableCell>
                      <div className="flex flex-wrap gap-1">
                        {s.workflows.map((w) => (
                          <Badge key={w} variant="outline">
                            {w}
                          </Badge>
                        ))}
                      </div>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}
      </div>
    </AppShell>
  );
}
